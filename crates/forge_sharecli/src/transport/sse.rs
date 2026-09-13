//! SSE transport — hand-rolled HTTP/1.1 + Server-Sent Events.
//!
//! Wires [`crate::ShareHub`] topics to the wire as follows:
//!
//! ```text
//! GET  /sse/<topic>     -> 200 text/event-stream + stream of `ShareMessage`s
//! POST /publish/<topic> -> 202 Accepted (body = JSON `ShareMessage`)
//! <other>               -> 404 Not Found
//! ```
//!
//! The HTTP framing is implemented by hand against `tokio::net::TcpStream`
//! — `Content-Length` is used on the publish endpoint, `Transfer-Encoding:
//! chunked` on the SSE endpoint. No `axum`/`hyper`/`tiny_http` involved.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::error::ShareError;
use crate::hub::ShareHub;
use crate::message::ShareMessage;

use super::RELAY_CAP;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Drive a single TCP connection under the SSE transport.
///
/// `stream` is an already-accepted `tokio::net::TcpStream`. This function
/// reads one HTTP/1.1 request, dispatches it, and drives the response —
/// returning when the connection is half-closed or an unrecoverable
/// I/O error occurs.
pub async fn serve_sse(hub: Arc<ShareHub>, mut stream: TcpStream) -> io::Result<()> {
    let req = match read_request(&mut stream).await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(error = %e, "sse: request parse failed");
            let _ = stream.shutdown().await;
            return Ok(());
        }
    };

    let result = dispatch(&hub, req, &mut stream).await;
    let _ = stream.shutdown().await;
    if let Err(e) = result {
        tracing::debug!(error = %e, "sse: connection ended with error");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Request dispatch
// ---------------------------------------------------------------------------

async fn dispatch(
    hub: &Arc<ShareHub>,
    req: ParsedRequest,
    writer: &mut TcpStream,
) -> io::Result<()> {
    let path = req.path.clone();
    let path_ref = path.split('?').next().unwrap_or(path.as_str());

    if let Some(topic) = path_ref.strip_prefix("/sse/").filter(|t| !t.is_empty()) {
        return handle_sse_get(hub, topic, writer).await;
    }
    if let Some(topic) = path_ref.strip_prefix("/publish/").filter(|t| !t.is_empty()) {
        return handle_publish_post(hub, topic, req, writer).await;
    }
    write_not_found(writer).await
}

async fn handle_sse_get(
    hub: &Arc<ShareHub>,
    topic: &str,
    writer: &mut TcpStream,
) -> io::Result<()> {
    if let Err(e) = hub.register_topic(topic) {
        return write_share_error(writer, &e).await;
    }
    let subscriber = match hub.subscribe(topic) {
        Ok(s) => s,
        Err(e) => return write_share_error(writer, &e).await,
    };

    writer.write_all(b"HTTP/1.1 200 OK\r\n").await?;
    writer
        .write_all(b"Content-Type: text/event-stream\r\n")
        .await?;
    writer.write_all(b"Cache-Control: no-cache\r\n").await?;
    writer.write_all(b"Connection: close\r\n").await?;
    writer.write_all(b"Transfer-Encoding: chunked\r\n").await?;
    writer.write_all(b"\r\n").await?;
    writer.flush().await?;

    pump_sse(hub, topic, writer, subscriber).await
}

/// Drive the SSE pump. The writer `&mut TcpStream` is split conceptually
/// into a read half (used only for disconnect detection) and a write half
/// (used for SSE frames). Since `&mut TcpStream: AsyncRead + AsyncWrite`,
/// we just call both on the same borrow and rely on `tokio::select!` to
/// arbitrate without ever holding both at once.
async fn pump_sse(
    hub: &Arc<ShareHub>,
    topic: &str,
    writer: &mut TcpStream,
    mut subscriber: crate::channel::Subscriber,
) -> io::Result<()> {
    // Forwarder: hub subscriber -> bounded mpsc.
    let (tx, mut rx) = mpsc::channel::<ShareMessage>(RELAY_CAP);
    let forward_topic = topic.to_string();
    let forward = tokio::spawn(async move {
        loop {
            match subscriber.recv().await {
                Ok(msg) => {
                    if tx.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(ShareError::Lagged { skipped }) => {
                    tracing::warn!(
                        topic = %forward_topic,
                        skipped,
                        "sse: subscriber lagged; emitting lag frame"
                    );
                    let lag = ShareMessage::new(
                        forward_topic.clone(),
                        serde_json::json!({ "__sharecli_lagged": skipped }),
                    );
                    let _ = tx.try_send(lag);
                }
                Err(ShareError::Closed) => break,
                Err(e) => {
                    tracing::warn!(topic = %forward_topic, error = %e, "sse: subscriber error");
                    break;
                }
            }
        }
    });

    // Writer: drive SSE frames. Race with the disconnect reader so we
    // exit cleanly when the peer hangs up. We never write and read at
    // the same time — the select! arm runs to completion before the
    // other is polled.
    loop {
        let disconnected = read_one_byte(writer).await;
        if disconnected {
            break;
        }
        // Pending read byte (if any) — drain it. If we already got the
        // byte above, the buffer is now empty.
        tokio::select! {
            biased;
            res = read_one_byte_or_zero(writer) => {
                if res {
                    break; // EOF
                }
                // otherwise we read a stray byte; ignore.
            }
            maybe = rx.recv() => {
                match maybe {
                    Some(msg) => {
                        if write_event_frame(writer, &msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }

    forward.abort();
    let _ = forward.await;

    // Best-effort chunked terminator (only if the connection is still
    // writable — write failures are expected here on a peer-side close).
    let _ = write_chunk(writer, b"").await;
    let _ = writer.flush().await;
    let _ = hub; // suppress unused-warning while keeping the param
    Ok(())
}

/// Poll once for a byte from the stream with no wait. Returns:
/// * `true` if the peer closed (EOF).
/// * `false` otherwise (data was available or the poll would block).
async fn read_one_byte(stream: &mut TcpStream) -> bool {
    let mut buf = [0u8; 1];
    match stream.try_read(&mut buf) {
        Ok(0) => true,
        Ok(_) => false,
        // WouldBlock: peer hasn't sent anything yet.
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => false,
        Err(_) => true,
    }
}

/// Same as [`read_one_byte`] but waits. We use this only inside
/// `tokio::select!` where we race the read against an mpsc receive.
async fn read_one_byte_or_zero(stream: &mut TcpStream) -> bool {
    let mut buf = [0u8; 1];
    match stream.read(&mut buf).await {
        Ok(0) => true,
        Ok(_) => false,
        Err(_) => true,
    }
}

async fn handle_publish_post(
    hub: &Arc<ShareHub>,
    topic: &str,
    req: ParsedRequest,
    writer: &mut TcpStream,
) -> io::Result<()> {
    if !req.headers.contains_key("content-length") {
        return write_simple(writer, 411, "Length Required", b"Content-Length required").await;
    }

    let msg: ShareMessage = match serde_json::from_slice(&req.body) {
        Ok(m) => m,
        Err(e) => {
            return write_simple(
                writer,
                400,
                "Bad Request",
                format!("invalid ShareMessage JSON: {e}").as_bytes(),
            )
            .await;
        }
    };

    let mut msg = msg;
    msg.topic = topic.to_string();
    msg.seq = 0;

    match hub.publish(msg) {
        Ok(n) => {
            write_simple(
                writer,
                202,
                "Accepted",
                format!("{{\"receivers\":{n}}}\n").as_bytes(),
            )
            .await
        }
        Err(e) => write_share_error(writer, &e).await,
    }
}

// ---------------------------------------------------------------------------
// Frame + status helpers
// ---------------------------------------------------------------------------

fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        411 => "Length Required",
        422 => "Unprocessable Entity",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

async fn write_simple(
    writer: &mut TcpStream,
    code: u16,
    reason: &'static str,
    body: &[u8],
) -> io::Result<()> {
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        code = code,
        reason = reason,
        len = body.len(),
    );
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

async fn write_not_found(writer: &mut TcpStream) -> io::Result<()> {
    write_simple(
        writer,
        404,
        reason_phrase(404),
        b"{\"error\":\"not found\"}\n",
    )
    .await
}

async fn write_share_error(writer: &mut TcpStream, err: &ShareError) -> io::Result<()> {
    let body = format!("{{\"error\":\"{err}\"}}\n");
    let code = match err {
        ShareError::InvalidTopic { .. } => 422,
        ShareError::Full { .. } => 503,
        ShareError::Closed => 503,
        ShareError::Lagged { .. } => 503,
        ShareError::Internal(_) => 500,
    };
    write_simple(writer, code, reason_phrase(code), body.as_bytes()).await
}

async fn write_event_frame(writer: &mut TcpStream, msg: &ShareMessage) -> io::Result<()> {
    let payload = serde_json::to_vec(msg).map_err(io::Error::other)?;
    let mut buf = BytesMut::new();
    buf.extend_from_slice(b"id: ");
    buf.extend_from_slice(msg.id.as_bytes());
    buf.extend_from_slice(b"\n");
    buf.extend_from_slice(b"event: message\n");
    buf.extend_from_slice(b"data: ");
    buf.extend_from_slice(&payload);
    buf.extend_from_slice(b"\n\n");

    write_chunk(writer, &buf).await?;
    writer.flush().await?;
    Ok(())
}

async fn write_chunk(writer: &mut TcpStream, payload: &[u8]) -> io::Result<()> {
    let header = format!("{:X}\r\n", payload.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(payload).await?;
    writer.write_all(b"\r\n").await
}

// ---------------------------------------------------------------------------
// HTTP request parsing
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ParsedRequest {
    #[allow(dead_code)]
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Bytes,
}

async fn read_request(stream: &mut TcpStream) -> io::Result<ParsedRequest> {
    let mut reader = BufReader::new(&mut *stream);
    let read_deadline = Duration::from_secs(5);

    let mut line = Vec::new();
    let n = timeout(read_deadline, reader.read_until(b'\n', &mut line))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request line read timed out"))??;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "empty request",
        ));
    }
    let req_line = std::str::from_utf8(&line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("utf8: {e}")))?;
    let req_line = req_line.trim_end_matches(['\r', '\n']);
    let mut parts = req_line.split_ascii_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing method"))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?
        .to_string();

    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = Vec::new();
        let n = timeout(read_deadline, reader.read_until(b'\n', &mut line))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "header read timed out"))??;
        if n == 0 {
            break;
        }
        let s = std::str::from_utf8(&line)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("utf8: {e}")))?;
        let s = s.trim_end_matches(['\r', '\n']);
        if s.is_empty() {
            break;
        }
        if let Some((k, v)) = s.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let mut body = BytesMut::new();
    if let Some(len_str) = headers.get("content-length") {
        let len: usize = len_str.parse().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("content-length: {e}"))
        })?;
        let cap = len.min(1 << 20);
        body.resize(cap, 0);
        if cap > 0 {
            if let Some(buf) = body.get_mut(..cap) {
                reader.read_exact(buf).await?;
            } else {
                return Err(io::Error::other("internal: body buffer shrunk under us"));
            }
        }
        body.truncate(len);
    }

    Ok(ParsedRequest { method, path, headers, body: body.freeze() })
}
