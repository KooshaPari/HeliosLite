//! CLI surface for `forge share` — the consumer entry point for `ShareHub`.
//!
//! The `share` group owns a single process-wide [`crate::ShareHub`] and
//! exposes it on the wire. Each subcommand constructs its own hub (matching
//! the `agileplus` / `lsp` precedent where the binary dispatches before the
//! full UI startup so no existing runtime is disturbed).

use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::signal;
use tracing::{debug, info, warn};

use crate::{ShareHub, ShareMessage};

/// Errors surfaced from `forge share`.
#[derive(Debug, Error)]
pub enum CliError {
    /// Binding or serving failed.
    #[error("share: {0}")]
    Serve(String),
    /// Topic or payload validation failed.
    #[error("share: {0}")]
    Validation(String),
}

/// Top-level `forge share` command. Re-exported as
/// `ShareCommandGroup` so the parent binary can surface
/// `forge share <subcommand>`.
///
/// Derives `Subcommand` so it slots directly into `TopLevelCommand::Share(_)`
/// inside `forge_main`'s CLI — same shape as `LspCommandGroup`.
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Start a TCP relay that bridges `ShareHub` topics to SSE + WebSocket.
    ///
    /// Binds `[host]:[port]` and serves:
    ///   GET  /sse/<topic>     -> text/event-stream of `ShareMessage`s
    ///   POST /publish/<topic> -> 202 Accepted (JSON body forwarded)
    ///   GET  /ws/<topic>      -> RFC 6455 WebSocket (JSON envelope)
    Serve {
        /// Interface to bind (e.g. 127.0.0.1).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// TCP port to listen on.
        #[arg(long, default_value_t = 8765)]
        port: u16,
    },
    /// Publish a single JSON payload to a topic (ephemeral hub, exits after send).
    ///
    /// Useful for scripting: `forge share publish --topic chat --payload '{"text":"hi"}'`
    /// fans the payload out to any subscribers on that topic. With no live
    /// subscribers the publish is a no-op (returns the receiver count).
    Publish {
        /// Topic name (non-empty, no NUL).
        #[arg(long)]
        topic: String,
        /// JSON payload to publish. Bare strings are wrapped as JSON strings.
        #[arg(long)]
        payload: String,
    },
    /// List the topics currently registered in a fresh hub (always empty).
    ///
    /// Exists so the subcommand tree has a diagnostic leaf and `clap` help
    /// stays complete. A live `serve` process is the real registry.
    Topics,
    /// Attach an in-process session to the relay as a pure ingestion
    /// endpoint.
    ///
    /// Binds `[bind]` (default `127.0.0.1:9900`) and accepts inbound TCP
    /// connections. Each connection streams newline-delimited JSON: every
    /// complete line is wrapped in a `ShareMessage` and published to the
    /// `--topic` channel of a fresh `ShareHub`. No outbound delivery,
    /// no protocol framing — this is a write-side bridge so a long-running
    /// session can publish events to the relay while a separate client
    /// (SSE/WS) is subscribed elsewhere. Press Ctrl-C to shut down.
    Attach {
        /// Topic / channel name to register on the ingest hub.
        #[arg(long)]
        topic: String,
        /// Listen address (`host:port`). Default `127.0.0.1:9900`.
        #[arg(long, default_value = "127.0.0.1:9900")]
        bind: String,
        /// Optional session id surfaced on every published
        /// `ShareMessage.payload.session_id` so SSE/WS consumers can
        /// attribute events to a specific session.
        #[arg(long)]
        session_id: Option<String>,
    },
}

/// Top-level CLI parser (for direct binary use, e.g. tests).
///
/// Mirrors `forge_lsp::commands::Cli`: a `Parser`-derived wrapper around the
/// `Command` enum so `Cli::parse()` works in standalone contexts.
#[derive(Parser, Debug)]
#[command(name = "share", about = "ShareCLI realtime relay")]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

fn parse_addr(host: &str, port: u16) -> Result<SocketAddr, CliError> {
    format!("{host}:{port}")
        .parse()
        .map_err(|e| CliError::Serve(format!("invalid --host/--port: {e}")))
}

/// Run a `forge share` subcommand. The `serve` and `attach` subcommands
/// block until the listener is closed (Ctrl-C / SIGTERM). The other
/// subcommands return immediately with an optional stdout string.
pub fn run_command(cmd: &Command) -> Result<Option<String>, CliError> {
    match cmd {
        Command::Serve { host, port } => {
            let addr = parse_addr(host, *port)?;
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| CliError::Serve(format!("failed to build runtime: {e}")))?;
            rt.block_on(serve(addr))?;
            Ok(None)
        }
        Command::Publish { topic, payload } => {
            let hub = ShareHub::new();
            let value: serde_json::Value = serde_json::from_str(payload)
                .unwrap_or_else(|_| serde_json::Value::String(payload.clone()));
            let n = hub
                .publish(crate::ShareMessage::new(topic.clone(), value))
                .map_err(|e| CliError::Validation(e.to_string()))?;
            Ok(Some(format!("published to {n} receiver(s)\n")))
        }
        Command::Topics => Ok(Some(String::new())),
        Command::Attach { topic, bind, session_id } => {
            let topic = topic.clone();
            let bind = bind.clone();
            let session_id = session_id.clone();
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| CliError::Serve(format!("failed to build runtime: {e}")))?;
            rt.block_on(attach(&topic, &bind, session_id.as_deref()))?;
            Ok(None)
        }
    }
}

async fn serve(addr: SocketAddr) -> Result<(), CliError> {
    let hub = Arc::new(ShareHub::new());
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CliError::Serve(format!("bind {addr}: {e}")))?;
    tracing::info!(%addr, "forge share: listening (SSE + WS)");
    run_relay(hub, listener).await
}

/// Drive the `forge share` TCP relay against an already-bound [`TcpListener`].
///
/// This is the inner accept-loop extracted from [`serve`] so that integration
/// tests can bind a listener on an ephemeral port and exercise the live
/// HTTP path end-to-end. Production callers should keep using
/// [`serve`] (which performs the bind); tests can skip the bind by handing
/// in their own listener.
pub async fn run_relay(
    hub: Arc<ShareHub>,
    listener: tokio::net::TcpListener,
) -> Result<(), CliError> {
    loop {
        let (stream, peer) = listener
            .accept()
            .await
            .map_err(|e| CliError::Serve(format!("accept: {e}")))?;
        let hub = Arc::clone(&hub);
        tokio::spawn(async move {
            // Peek the request to decide transport without consuming the
            // stream for the chosen handler. Both handlers re-parse the
            // request from the stream — the peek is only for routing:
            // keep it cheap (first 2 KiB) and fall back to SSE if
            // inconclusive.
            let route_ws = {
                let mut buf = [0u8; 2048];
                let mut peek_stream = &stream;
                let n = match tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    peekable_read(&mut peek_stream, &mut buf),
                )
                .await
                {
                    Ok(Ok(n)) => n,
                    _ => 0,
                };
                let head = std::str::from_utf8(&buf[..n])
                    .map(str::to_ascii_lowercase)
                    .unwrap_or_default();
                head.contains("upgrade: websocket") || head.contains("get /ws/")
            };

            let res = if route_ws {
                crate::transport::ws::serve_ws(hub, stream).await
            } else {
                crate::transport::sse::serve_sse(hub, stream).await
            };
            if let Err(e) = res {
                tracing::debug!(%peer, error=%e, "share connection closed");
            }
        });
    }
}

async fn peekable_read(
    stream: &mut &tokio::net::TcpStream,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    // `TcpStream::peek` is the correct primitive for non-consuming lookahead.
    stream.peek(buf).await
}

// ---------------------------------------------------------------------------
// `attach` runtime
// ---------------------------------------------------------------------------

/// Run the `attach` listener. Blocks until Ctrl-C is received.
///
/// * Creates a `ShareHub`, registers `topic`.
/// * On TCP connect, accepts newline-delimited JSON lines and publishes
///   each as a `ShareMessage` to the topic.
/// * Drains at most one line per `read_line` call, with a short idle
///   cap so a silent client cannot wedge a connection slot indefinitely.
/// * On `tokio::signal::ctrl_c`, prints `attach: shutting down`, drops
///   the listener and hub, and returns `Ok(())`.
async fn attach(topic: &str, bind: &str, session_id: Option<&str>) -> Result<(), CliError> {
    // Register the topic eagerly so it shows up on hub introspection before
    // any subscriber attaches — mirrors what `transport::sse::handle_sse_get`
    // does for inbound SSE sessions.
    let hub = Arc::new(ShareHub::new());
    hub.register_topic(topic)
        .map_err(|e| CliError::Validation(e.to_string()))?;

    let addr: SocketAddr = bind
        .parse()
        .map_err(|e| CliError::Serve(format!("invalid --bind {bind}: {e}")))?;

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CliError::Serve(format!("bind {addr}: {e}")))?;
    let actual = listener
        .local_addr()
        .map_err(|e| CliError::Serve(format!("local_addr: {e}")))?;

    // Startup line on stdout, flushed before we yield to the runtime.
    // Printers that watch the parent process expect this before the
    // process blocks in `accept`.
    {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "attach: listening on {actual} for topic {topic}");
        let _ = out.flush();
    }
    info!(addr = %actual, topic = %topic, "forge share attach: listening");

    // Race the accept loop against SIGINT.
    let accept_topic = topic.to_string();
    let accept_session = session_id.map(str::to_owned);
    let accept = async {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(p) => p,
                Err(e) => {
                    warn!(error = %e, "attach: accept failed");
                    continue;
                }
            };
            let hub = Arc::clone(&hub);
            let topic_inner = accept_topic.clone();
            let session_inner = accept_session.clone();
            tokio::spawn(async move {
                if let Err(e) = ingest_connection(hub, topic_inner, session_inner, stream).await {
                    debug!(%peer, error = %e, "attach: connection ended with error");
                }
            });
        }
    };

    tokio::select! {
        _ = signal::ctrl_c() => {
            println!("attach: shutting down");
            drop(listener);
            drop(hub);
            Ok(())
        }
        _ = accept => {
            // `accept` only returns if the listener is closed by some
            // out-of-band path; unreachable in practice.
            Ok(())
        }
    }
}

/// Drain newline-delimited JSON from a single inbound TCP connection.
///
/// Each line is parsed as JSON; a valid line is wrapped in a `ShareMessage`
/// and published to `topic`. Malformed lines are logged at `debug` and
/// skipped — the connection keeps reading until EOF or a hard I/O error.
///
/// We bound each `read_line` to 5 seconds so a silent client cannot pin a
/// spawned task forever; idle disconnects produce a `TimedOut` and return.
async fn ingest_connection(
    hub: Arc<ShareHub>,
    topic: String,
    session_id: Option<String>,
    stream: TcpStream,
) -> io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let read =
            match tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line)).await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(e),
                Err(_) => {
                    debug!(topic = %topic, "attach: client idle; closing connection");
                    let _ = write_half.shutdown().await;
                    return Ok(());
                }
            };
        if read == 0 {
            // EOF — peer closed cleanly.
            let _ = write_half.shutdown().await;
            return Ok(());
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                warn!(topic = %topic, error = %e, raw = %trimmed, "attach: skipping malformed line");
                continue;
            }
        };

        let payload = match session_id.as_deref() {
            Some(sid) => serde_json::json!({ "session_id": sid, "event": value }),
            None => serde_json::json!({ "event": value }),
        };

        // We swallow publish errors (no live subscribers is the normal
        // case for `attach`) — only log if the message itself is invalid.
        match hub.publish(ShareMessage::new(topic.clone(), payload)) {
            Ok(n) => {
                debug!(topic = %topic, receivers = n, "attach: published");
            }
            Err(e) => {
                warn!(topic = %topic, error = %e, "attach: publish rejected");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    // Required unit test — drives the ingest path end-to-end:
    //   bind 127.0.0.1:0, open a TCP connection, write a single JSON line,
    //   verify a subscriber on the topic receives a ShareMessage whose
    //   payload contains the original JSON object.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn attach_ingest_publishes_to_subscriber() {
        let topic = "session-events";

        // 1) Bind listener on an ephemeral port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr: SocketAddr = listener.local_addr().expect("local_addr");

        // 2) Stand up a hub + subscriber before any connect, mirroring the
        //    ordering a real client would observe.
        let hub = Arc::new(ShareHub::new());
        hub.register_topic(topic).expect("register");
        let mut sub = hub.subscribe(topic).expect("subscribe");

        // 3) Spawn the same accept/ingest pair `attach()` uses, but with a
        //    bounded lifetime so the test deterministically terminates:
        //    1 accepted connection then the task exits.
        let topic_owned = topic.to_string();
        let hub_for_server = Arc::clone(&hub);
        let server = tokio::spawn(async move {
            // Accept exactly one connection.
            let (stream, _peer) = listener.accept().await.expect("accept");
            let _ = ingest_connection(
                hub_for_server,
                topic_owned,
                Some("test-session".to_string()),
                stream,
            )
            .await;
            // Close the listener so a second accept never fires.
            drop(listener);
        });

        // 4) Open a client connection and write one newline-delimited
        //    JSON line.
        let mut client = TcpStream::connect(addr).await.expect("connect");
        client
            .write_all(b"{\"hello\":1}\n")
            .await
            .expect("write line");
        // Half-close so the server-side `read_line` returns EOF after the
        // single line we've sent.
        client.shutdown().await.expect("shutdown client");

        // 5) Subscriber must observe a ShareMessage whose payload contains
        //    `hello: 1`, wrapped under `event` (with our `session_id`).
        let msg = tokio::time::timeout(Duration::from_secs(2), sub.recv())
            .await
            .expect("sub recv timed out")
            .expect("sub recv");
        assert_eq!(msg.topic, topic);
        let event = msg
            .payload
            .get("event")
            .expect("payload should carry an `event` envelope");
        let hello = event
            .get("hello")
            .expect("`event` payload should carry `hello`");
        assert_eq!(hello, &serde_json::json!(1));
        assert_eq!(
            msg.payload.get("session_id"),
            Some(&serde_json::json!("test-session"))
        );

        // 6) Let the server task drain its write-half shutdown + exit.
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("server task join")
            .expect("server task");
    }
}
