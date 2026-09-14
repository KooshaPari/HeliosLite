//! CLI surface for `forge share` — the consumer entry point for `ShareHub`.
//!
//! The `share` group owns a single process-wide [`crate::ShareHub`] and
//! exposes it on the wire. Each subcommand constructs its own hub (matching
//! the `agileplus` / `lsp` precedent where the binary dispatches before the
//! full UI startup so no existing runtime is disturbed).

use std::net::SocketAddr;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use thiserror::Error;

use crate::ShareHub;

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

/// Run a `forge share` subcommand. The `serve` subcommand blocks until the
/// listener is closed (Ctrl-C / SIGTERM). The other subcommands return
/// immediately with an optional stdout string.
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
    }
}

async fn serve(addr: SocketAddr) -> Result<(), CliError> {
    let hub = Arc::new(ShareHub::new());
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CliError::Serve(format!("bind {addr}: {e}")))?;
    tracing::info!(%addr, "forge share: listening (SSE + WS)");

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
