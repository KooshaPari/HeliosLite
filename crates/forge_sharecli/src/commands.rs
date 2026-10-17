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
        /// Running relay address owned by `share serve`.
        #[arg(long, default_value = "127.0.0.1:8765")]
        relay: String,
    },
    /// List topics from the running relay process.
    Topics {
        /// Running relay address owned by `share serve`.
        #[arg(long, default_value = "127.0.0.1:8765")]
        relay: String,
    },
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
        /// Running relay address owned by `share serve`.
        #[arg(long, default_value = "127.0.0.1:8765")]
        relay: String,
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
///
/// The blocking subcommands detect whether a tokio runtime is already
/// live on the calling thread (e.g. when dispatched from
/// `helioslite`'s main `runtime.block_on(async_main())`). In that
/// case they off-load the listen-accept loop to a freshly spawned
/// OS thread that owns its own multi-thread runtime — `block_on` on
/// a thread already driven by an outer runtime would otherwise panic
/// with "Cannot start a runtime from within a runtime". When called
/// from a sync context (a standalone script, the LSP CLI, an
/// integration test scaffolding), `run_command` builds the runtime
/// on the calling thread as before.
pub fn run_command(cmd: &Command) -> Result<Option<String>, CliError> {
    match cmd {
        Command::Serve { host, port } => {
            let addr = parse_addr(host, *port)?;
            run_serve_blocking(addr)?;
            Ok(None)
        }
        Command::Publish { topic, payload, relay } => {
            let value: serde_json::Value = serde_json::from_str(payload)
                .unwrap_or_else(|_| serde_json::Value::String(payload.clone()));
            let topic = topic.clone();
            let relay = relay.clone();
            let receivers = run_on_dedicated_thread(move |rt| {
                rt.block_on(publish_remote(&relay, &topic, value))
            })??;
            Ok(Some(format!("published to {receivers} receiver(s)\n")))
        }
        Command::Topics { relay } => {
            let relay = relay.clone();
            let topics = run_on_dedicated_thread(move |rt| rt.block_on(topics_remote(&relay)))??;
            let json = serde_json::to_string(&topics)
                .map_err(|e| CliError::Serve(e.to_string()))?;
            Ok(Some(format!("{json}\n")))
        }
        Command::Attach { topic, bind, session_id, relay } => {
            let topic = topic.clone();
            let bind = bind.clone();
            let relay = relay.clone();
            run_attach_blocking(&topic, &bind, session_id.as_deref(), &relay)?;
            Ok(None)
        }
    }
}

/// Drive `serve(addr)` either inline (no outer runtime) or on a
/// dedicated OS thread that owns its own multi-thread runtime
/// (called from inside another runtime's worker — the helioslite
/// `main` path).
fn run_serve_blocking(addr: SocketAddr) -> Result<(), CliError> {
    match tokio::runtime::Handle::try_current() {
        // No outer runtime — build one on the calling thread and
        // drive `serve` to completion.
        Err(_) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| CliError::Serve(format!("failed to build runtime: {e}")))?;
            rt.block_on(serve(addr))
        }
        // `Handle::current()` only succeeds if the calling thread is
        // *being driven by* a runtime, in which case `block_on` would
        // panic. Off-load to a fresh OS thread instead. The
        // dedicated thread owns its own runtime, so we need to move
        // the (cheap, `Copy`) `SocketAddr` into the closure.
        Ok(_) => {
            let addr_owned = addr;
            run_on_dedicated_thread(move |rt| rt.block_on(serve(addr_owned)))
                .and_then(|inner| inner)
        }
    }
}

/// Mirror of [`run_serve_blocking`] for the `attach` subcommand.
fn run_attach_blocking(topic: &str, bind: &str, session_id: Option<&str>, relay: &str) -> Result<(), CliError> {
    match tokio::runtime::Handle::try_current() {
        Err(_) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| CliError::Serve(format!("failed to build runtime: {e}")))?;
            rt.block_on(attach(topic, bind, session_id, relay))
        }
        // `topic`/`bind`/`session_id` are short-lived borrows
        // owned by the caller; clone them into `String`/`Option<String>`
        // so the dedicated-thread closure can outlive this stack frame.
        Ok(_) => {
            let topic = topic.to_owned();
            let bind = bind.to_owned();
            let session_id = session_id.map(str::to_owned);
            let relay = relay.to_owned();
            run_on_dedicated_thread(move |rt| {
                rt.block_on(attach(&topic, &bind, session_id.as_deref(), &relay))
            })
            .and_then(|inner| inner)
        }
    }
}

/// Spin up a dedicated OS thread, give it its own multi-thread
/// runtime, drive `f` to completion via `block_on` there, and return
/// the result to the caller. The dedicated thread is *not* part of
/// any other runtime, so `block_on` is safe even when the caller is
/// currently inside another runtime's worker thread.
///
/// We deliberately use a `std::thread::spawn` instead of
/// `Handle::spawn_blocking` because the inner future wants to
/// *drive* the runtime (with `block_on`); spawning it onto
/// `spawn_blocking` would deadlock that inner runtime's IO/timer
/// drivers. Owning a dedicated runtime on a dedicated thread is the
/// only configuration where `block_on` does not clash with a
/// pre-existing runtime.
fn run_on_dedicated_thread<F, T>(f: F) -> Result<T, CliError>
where
    F: FnOnce(&tokio::runtime::Runtime) -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel::<Result<T, String>>();
    std::thread::Builder::new()
        .name("share-cli-runtime".into())
        .spawn(move || {
            let result = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => Ok(f(&rt)),
                Err(e) => Err(format!("failed to build runtime: {e}")),
            };
            let _ = tx.send(result);
        })
        .map_err(|e| CliError::Serve(format!("failed to spawn dedicated runtime thread: {e}")))?;
    rx.recv()
        .map_err(|e| CliError::Serve(format!("dedicated runtime thread disconnected: {e}")))?
        .map_err(CliError::Serve)
}

async fn serve(addr: SocketAddr) -> Result<(), CliError> {
    eprintln!("[share-serve-debug] entering serve({addr})");
    let hub = Arc::new(ShareHub::new());
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            eprintln!("[share-serve-debug] bound {addr}");
            tracing::info!(%addr, "forge share: listening (SSE + WS)");
            run_relay(hub, listener).await
        }
        Err(e) => {
            eprintln!("[share-serve-debug] bind failed for {addr}: {e}");
            Err(CliError::Serve(format!("bind {addr}: {e}")))
        }
    }
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
                // `.get(..n)` instead of `&buf[..n]`: panicking slice indexing is
                // denied in CI (`-D clippy::indexing_slicing`). An out-of-range
                // `n` or invalid UTF-8 yields an empty head, exactly like the
                // previous `from_utf8(..).unwrap_or_default()` on an error.
                let head = buf
                    .get(..n)
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
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
async fn publish_remote(relay: &str, topic: &str, payload: serde_json::Value) -> Result<usize, CliError> {
    let mut stream = TcpStream::connect(relay).await
        .map_err(|e| CliError::Serve(format!("connect relay {relay}: {e}")))?;
    let body = serde_json::to_vec(&ShareMessage::new(topic.to_string(), payload))
        .map_err(|e| CliError::Validation(format!("serialize message: {e}")))?;
    let request = format!("POST /publish/{topic} HTTP/1.1\r\nHost: {relay}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(request.as_bytes()).await.map_err(|e| CliError::Serve(format!("write relay request: {e}")))?;
    stream.write_all(&body).await.map_err(|e| CliError::Serve(format!("write relay body: {e}")))?;
    stream.shutdown().await.map_err(|e| CliError::Serve(format!("finish relay request: {e}")))?;
    let mut response = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut response).await
        .map_err(|e| CliError::Serve(format!("read relay response: {e}")))?;
    parse_publish_response(&response)
}

fn parse_http_body(response: &[u8]) -> Result<&[u8], CliError> {
    let Some(split) = response.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(CliError::Serve("relay returned malformed HTTP response".into()));
    };
    let head = std::str::from_utf8(&response[..split])
        .map_err(|e| CliError::Serve(format!("relay response header utf8: {e}")))?;
    if !head.starts_with("HTTP/1.1 2") {
        return Err(CliError::Serve(format!("relay rejected request: {}", head.lines().next().unwrap_or("unknown status"))));
    }
    Ok(&response[split + 4..])
}

fn parse_publish_response(response: &[u8]) -> Result<usize, CliError> {
    let value: serde_json::Value = serde_json::from_slice(parse_http_body(response)?)
        .map_err(|e| CliError::Serve(format!("relay publish response json: {e}")))?;
    value.get("receivers").and_then(serde_json::Value::as_u64).map(|n| n as usize)
        .ok_or_else(|| CliError::Serve("relay publish response missing receivers".into()))
}

async fn topics_remote(relay: &str) -> Result<Vec<String>, CliError> {
    let mut stream = TcpStream::connect(relay).await
        .map_err(|e| CliError::Serve(format!("connect relay {relay}: {e}")))?;
    let request = format!("GET /topics HTTP/1.1\r\nHost: {relay}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.map_err(|e| CliError::Serve(format!("write topics request: {e}")))?;
    stream.shutdown().await.map_err(|e| CliError::Serve(format!("finish topics request: {e}")))?;
    let mut response = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut stream, &mut response).await
        .map_err(|e| CliError::Serve(format!("read topics response: {e}")))?;
    serde_json::from_slice(parse_http_body(&response)?)
        .map_err(|e| CliError::Serve(format!("relay topics response json: {e}")))
}

async fn attach(topic: &str, bind: &str, session_id: Option<&str>, relay: &str) -> Result<(), CliError> {
    let addr: SocketAddr = bind.parse().map_err(|e| CliError::Serve(format!("invalid --bind {bind}: {e}")))?;
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| CliError::Serve(format!("bind {addr}: {e}")))?;
    let actual = listener.local_addr().map_err(|e| CliError::Serve(format!("local_addr: {e}")))?;
    { let mut out = io::stdout().lock(); let _ = writeln!(out, "attach: listening on {actual} for topic {topic}; relay {relay}"); let _ = out.flush(); }
    info!(addr = %actual, topic = %topic, relay = %relay, "forge share attach: listening");
    let accept_topic = topic.to_string();
    let accept_session = session_id.map(str::to_owned);
    let relay = relay.to_string();
    let accept = async {
        loop {
            let (stream, peer) = match listener.accept().await { Ok(p)=>p, Err(e)=>{ warn!(error=%e,"attach: accept failed"); continue; } };
            let topic_inner=accept_topic.clone(); let session_inner=accept_session.clone(); let relay_inner=relay.clone();
            tokio::spawn(async move {
                if let Err(e)=ingest_connection(relay_inner,topic_inner,session_inner,stream).await {
                    warn!(%peer,error=%e,"attach: relay ingestion failed");
                }
            });
        }
    };
    tokio::select! { _ = signal::ctrl_c()=>{ println!("attach: shutting down"); drop(listener); Ok(()) }, _ = accept => Ok(()) }
}

async fn ingest_connection(relay: String, topic: String, session_id: Option<String>, stream: TcpStream) -> Result<(), CliError> {
    let (read_half, mut write_half)=stream.into_split();
    let mut reader=BufReader::new(read_half); let mut line=String::new();
    loop {
        line.clear();
        let read=match tokio::time::timeout(Duration::from_secs(5),reader.read_line(&mut line)).await {
            Ok(Ok(n))=>n, Ok(Err(e))=>return Err(CliError::Serve(e.to_string())),
            Err(_)=>{ debug!(topic=%topic,"attach: client idle; closing connection"); let _=write_half.shutdown().await; return Ok(()); }
        };
        if read==0 { let _=write_half.shutdown().await; return Ok(()); }
        let trimmed=line.trim(); if trimmed.is_empty(){continue;}
        let value:serde_json::Value=match serde_json::from_str(trimmed){Ok(v)=>v,Err(e)=>{warn!(topic=%topic,error=%e,raw=%trimmed,"attach: skipping malformed line");continue;}};
        let payload=match session_id.as_deref(){Some(sid)=>serde_json::json!({"session_id":sid,"event":value}),None=>serde_json::json!({"event":value})};
        let receivers=publish_remote(&relay,&topic,payload).await?;
        debug!(topic=%topic,receivers,"attach: relayed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remote_publish_reaches_subscriber_in_relay_process() {
        let hub=Arc::new(ShareHub::new()); let topic="cross-process";
        let mut subscriber=hub.subscribe(topic).expect("subscribe");
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr=listener.local_addr().expect("addr"); let relay_hub=Arc::clone(&hub);
        let server=tokio::spawn(async move { let (stream,_)=listener.accept().await.expect("accept"); crate::transport::sse::serve_sse(relay_hub,stream).await.expect("serve"); });
        let receivers=publish_remote(&addr.to_string(),topic,serde_json::json!({"hello":1})).await.expect("publish");
        assert_eq!(receivers,1);
        let msg=tokio::time::timeout(Duration::from_secs(2),subscriber.recv()).await.expect("timeout").expect("recv");
        assert_eq!(msg.payload,serde_json::json!({"hello":1}));
        server.await.expect("join");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn topics_reads_running_relay_registry() {
        let hub=Arc::new(ShareHub::new()); hub.register_topic("alpha").unwrap(); hub.register_topic("beta").unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let addr=listener.local_addr().unwrap(); let relay_hub=Arc::clone(&hub);
        let server=tokio::spawn(async move { let (stream,_)=listener.accept().await.unwrap(); crate::transport::sse::serve_sse(relay_hub,stream).await.unwrap(); });
        let mut topics=topics_remote(&addr.to_string()).await.unwrap(); topics.sort();
        assert_eq!(topics,vec!["alpha".to_string(),"beta".to_string()]);
        server.await.unwrap();
    }
}
