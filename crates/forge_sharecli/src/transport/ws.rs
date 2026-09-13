//! WebSocket transport — minimal RFC 6455 server, hand-rolled.
//!
//! Wires [`crate::ShareHub`] topics to the wire as RFC 6455 text frames:
//!
//! * The HTTP/1.1 `Upgrade: websocket` handshake is performed with the
//!   standard `Sec-WebSocket-Accept` value (computed with the inline
//!   SHA-1 in [`super`]).
//! * Frames use opcode `0x1` (text) for application data and `0x8`
//!   (close) to terminate the connection. Ping frames (opcode `0x9`) are
//!   answered with a pong (`0xA`); pong frames are ignored.
//! * Server→client frames are NOT masked. Client→server frames ARE
//!   masked per the spec; the mask is removed after read.
//! * Application-level envelope: `{"topic": "...", "message": <ShareMessage>}`.
//!
//! No `tungstenite`/`tokio-tungstenite`/`fastwebsockets` is used — the
//! framing and handshake are implemented from scratch using only
//! `tokio::net::TcpStream` plus `bytes`.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::channel::Subscriber;
use crate::error::ShareError;
use crate::hub::ShareHub;
use crate::message::ShareMessage;

use super::RELAY_CAP;

// ---------------------------------------------------------------------------
// Wire envelope (RFC 6455 text payload)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    topic: String,
    message: ShareMessage,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Drive a single TCP connection under the WebSocket transport.
///
/// `stream` is an already-accepted `tokio::net::TcpStream`. The function
/// performs the RFC 6455 handshake, then runs the read/write pump until
/// either side issues a close frame or the connection drops.
pub async fn serve_ws(hub: Arc<ShareHub>, stream: TcpStream) -> io::Result<()> {
    let mut stream = stream;
    let path = match perform_handshake(&mut stream).await {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!(error = %e, "ws: handshake failed");
            let _ = stream.shutdown().await;
            return Ok(());
        }
    };
    let topic = match path.strip_prefix("/ws/").filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => {
            send_close(
                &mut stream,
                CloseCode::PolicyViolation,
                "expected /ws/<topic>",
            )
            .await
            .ok();
            let _ = stream.shutdown().await;
            return Ok(());
        }
    };

    if let Err(e) = hub.register_topic(&topic) {
        send_close(&mut stream, CloseCode::PolicyViolation, &e.to_string())
            .await
            .ok();
        let _ = stream.shutdown().await;
        return Ok(());
    }

    let subscriber = match hub.subscribe(&topic) {
        Ok(s) => s,
        Err(e) => {
            send_close(&mut stream, CloseCode::PolicyViolation, &e.to_string())
                .await
                .ok();
            let _ = stream.shutdown().await;
            return Ok(());
        }
    };

    run_pump(hub, topic, stream, subscriber).await
}

// ---------------------------------------------------------------------------
// Handshake (HTTP/1.1 Upgrade)
// ---------------------------------------------------------------------------

async fn perform_handshake(stream: &mut TcpStream) -> io::Result<String> {
    // Read using a borrowed view of the TcpStream (TcpStream implements
    // AsyncRead/AsyncWrite via &mut — we use AsyncReadExt directly).
    let mut reader = BufReader::new(&mut *stream);
    let deadline = Duration::from_secs(5);

    let mut line = Vec::new();
    let n = timeout(deadline, reader.read_until(b'\n', &mut line))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ws request line read timed out"))??;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "ws empty"));
    }
    let req_line = std::str::from_utf8(&line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("utf8: {e}")))?;
    let req_line = req_line.trim_end_matches(['\r', '\n']);
    let mut parts = req_line.split_ascii_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if !method.eq_ignore_ascii_case("GET") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: method must be GET",
        ));
    }
    if path.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ws: path empty"));
    }

    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = Vec::new();
        let n = timeout(deadline, reader.read_until(b'\n', &mut line))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "ws header read timed out"))??;
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

    let upgrade = headers
        .get("upgrade")
        .map(|s| s.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if !upgrade {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: missing/invalid Upgrade header",
        ));
    }
    let connection = headers
        .get("connection")
        .map(|s| s.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);
    if !connection {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: missing/invalid Connection header",
        ));
    }
    let key = headers
        .get("sec-websocket-key")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "ws: missing Sec-WebSocket-Key"))?
        .clone();
    let accept = super::ws_accept_key(&key);

    // Drop the buffered reader so we can use the stream's writer half.
    drop(reader);

    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;

    Ok(path.split('?').next().unwrap_or(path).to_string())
}

// ---------------------------------------------------------------------------
// Pump (concurrent reader + writer driven by the hub subscriber)
// ---------------------------------------------------------------------------

async fn run_pump(
    hub: Arc<ShareHub>,
    topic: String,
    stream: TcpStream,
    mut subscriber: Subscriber,
) -> io::Result<()> {
    let (read_half, mut write_half) = tokio::io::split(stream);

    let (tx, mut rx) = mpsc::channel::<Bytes>(RELAY_CAP);
    let writer_task = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if write_half.write_all(&frame).await.is_err() {
                break;
            }
            if write_half.flush().await.is_err() {
                break;
            }
        }
    });

    let forward_topic = topic.clone();
    let forward_tx = tx.clone();
    let forward = tokio::spawn(async move {
        loop {
            match subscriber.recv().await {
                Ok(msg) => {
                    let env = Envelope { topic: msg.topic.clone(), message: msg };
                    let body = match serde_json::to_vec(&env) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!(error = %e, "ws: encode envelope failed");
                            continue;
                        }
                    };
                    let frame = match encode_text_frame(&body) {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!(error = %e, "ws: encode frame failed");
                            continue;
                        }
                    };
                    if forward_tx.try_send(frame).is_err() {
                        tracing::warn!(topic = %forward_topic, "ws: outbound buffer full, dropping frame");
                    }
                }
                Err(ShareError::Lagged { skipped }) => {
                    tracing::warn!(topic = %forward_topic, skipped, "ws: subscriber lagged");
                    let env = Envelope {
                        topic: forward_topic.clone(),
                        message: ShareMessage::new(
                            forward_topic.clone(),
                            serde_json::json!({ "__sharecli_lagged": skipped }),
                        ),
                    };
                    if let Ok(body) = serde_json::to_vec(&env)
                        && let Ok(frame) = encode_text_frame(&body)
                    {
                        let _ = forward_tx.try_send(frame);
                    }
                }
                Err(ShareError::Closed) => break,
                Err(e) => {
                    tracing::warn!(topic = %forward_topic, error = %e, "ws: subscriber error");
                    break;
                }
            }
        }
    });

    let mut reader = BufReader::new(read_half);
    let result = drive_inbound(&mut reader, &hub, &topic, &tx).await;

    // Best-effort close frame back to the client.
    let close = encode_close_frame(CloseCode::Normal, "ok").unwrap_or_default();
    let _ = tx.send(close).await;

    drop(tx);
    forward.abort();
    let _ = forward.await;
    let _ = writer_task.await;
    result
}

async fn drive_inbound(
    reader: &mut BufReader<tokio::io::ReadHalf<TcpStream>>,
    hub: &Arc<ShareHub>,
    default_topic: &str,
    tx: &mpsc::Sender<Bytes>,
) -> io::Result<()> {
    loop {
        let frame = match read_frame(reader).await {
            Ok(f) => f,
            Err(e) => {
                if matches!(e.kind(), io::ErrorKind::UnexpectedEof) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        match frame.opcode {
            OpCode::Text => {
                if let Err(e) = handle_inbound_text(&frame.payload, hub, default_topic).await {
                    tracing::debug!(error = %e, "ws: inbound handler error");
                }
            }
            OpCode::Close => {
                // Echo the close to the writer queue (best-effort) and exit.
                let close = encode_close_frame(CloseCode::Normal, "ok")?;
                let _ = tx.try_send(close);
                return Ok(());
            }
            OpCode::Ping => {
                let pong = encode_control_frame(OpCode::Pong, &frame.payload)?;
                let _ = tx.try_send(pong);
            }
            OpCode::Pong | OpCode::Binary | OpCode::Continuation => {}
        }
    }
}

async fn handle_inbound_text(
    payload: &[u8],
    hub: &Arc<ShareHub>,
    default_topic: &str,
) -> io::Result<()> {
    let envelope: Envelope = match serde_json::from_slice(payload) {
        Ok(e) => e,
        Err(_) => {
            let bare: ShareMessage = serde_json::from_slice(payload).map_err(io::Error::other)?;
            Envelope { topic: bare.topic.clone(), message: bare }
        }
    };
    let topic = if envelope.topic.is_empty() {
        default_topic.to_string()
    } else {
        envelope.topic
    };
    let mut msg = envelope.message;
    msg.topic = topic;
    msg.seq = 0;
    hub.publish(msg).map_err(io::Error::other)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Frame encoding/decoding (RFC 6455 §5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum OpCode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl OpCode {
    fn from_u8(b: u8) -> io::Result<Self> {
        match b {
            0x0 => Ok(OpCode::Continuation),
            0x1 => Ok(OpCode::Text),
            0x2 => Ok(OpCode::Binary),
            0x8 => Ok(OpCode::Close),
            0x9 => Ok(OpCode::Ping),
            0xA => Ok(OpCode::Pong),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ws: unknown opcode {b}"),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
enum CloseCode {
    Normal = 1000,
    PolicyViolation = 1008,
    InternalError = 1011,
}

impl CloseCode {
    #[allow(dead_code)]
    fn from_u16(v: u16) -> io::Result<Self> {
        match v {
            1000 => Ok(CloseCode::Normal),
            1008 => Ok(CloseCode::PolicyViolation),
            1011 => Ok(CloseCode::InternalError),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("ws: unsupported close code {other}"),
            )),
        }
    }
}

#[derive(Debug)]
struct Frame {
    opcode: OpCode,
    payload: Bytes,
}

async fn read_frame(reader: &mut BufReader<tokio::io::ReadHalf<TcpStream>>) -> io::Result<Frame> {
    let b0 = read_byte(reader).await?;
    let fin = (b0 & 0x80) != 0;
    let rsv = b0 & 0x70;
    if rsv != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: reserved bits set",
        ));
    }
    let opcode = OpCode::from_u8(b0 & 0x0F)?;
    if !fin && !matches!(opcode, OpCode::Continuation) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: fragmented frames not supported",
        ));
    }

    let b1 = read_byte(reader).await?;
    let masked = (b1 & 0x80) != 0;
    let mut len = (b1 & 0x7F) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        reader.read_exact(&mut ext).await?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        reader.read_exact(&mut ext).await?;
        len = u64::from_be_bytes(ext) as usize;
        if len > usize::MAX / 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ws: payload too large",
            ));
        }
    }
    if len > 1 << 20 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: payload exceeds 1 MiB",
        ));
    }

    let mask_key = if masked {
        let mut m = [0u8; 4];
        reader.read_exact(&mut m).await?;
        m
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: client frame must be masked",
        ));
    };

    let mut payload = vec![0u8; len];
    if len > 0 {
        reader.read_exact(&mut payload).await?;
        for (i, b) in payload.iter_mut().enumerate() {
            // `mask_key` is `[u8; 4]` and `i & 3` is in `[0, 4)`, so a fixed
            // branch avoids the workspace's `indexing_slicing` lint without
            // pulling a get/ok_or into the hot path.
            let m = match i & 3 {
                0 => mask_key[0],
                1 => mask_key[1],
                2 => mask_key[2],
                _ => mask_key[3],
            };
            *b ^= m;
        }
    }

    Ok(Frame { opcode, payload: Bytes::from(payload) })
}

async fn read_byte(reader: &mut BufReader<tokio::io::ReadHalf<TcpStream>>) -> io::Result<u8> {
    let mut buf = [0u8; 1];
    let n = reader.read(&mut buf).await?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "ws: eof"));
    }
    Ok(buf[0])
}

fn encode_text_frame(payload: &[u8]) -> io::Result<Bytes> {
    encode_data_frame(OpCode::Text, payload)
}

fn encode_close_frame(code: CloseCode, reason: &str) -> io::Result<Bytes> {
    let mut body = Vec::with_capacity(2 + reason.len());
    body.extend_from_slice(&(code as u16).to_be_bytes());
    body.extend_from_slice(reason.as_bytes());
    encode_data_frame(OpCode::Close, &body)
}

fn encode_control_frame(opcode: OpCode, payload: &[u8]) -> io::Result<Bytes> {
    encode_data_frame(opcode, payload)
}

fn encode_data_frame(opcode: OpCode, payload: &[u8]) -> io::Result<Bytes> {
    let mut buf = BytesMut::new();
    buf.extend_from_slice(&[0x80 | (opcode as u8)]);
    let len = payload.len();
    if len < 126 {
        buf.extend_from_slice(&[len as u8]);
    } else if len <= u16::MAX as usize {
        buf.extend_from_slice(&[126u8]);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else if len <= u64::MAX as usize {
        buf.extend_from_slice(&[127u8]);
        buf.extend_from_slice(&(len as u64).to_be_bytes());
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: payload too large",
        ));
    }
    buf.extend_from_slice(payload);
    Ok(buf.freeze())
}

async fn send_close(stream: &mut TcpStream, code: CloseCode, reason: &str) -> io::Result<()> {
    let frame = encode_close_frame(code, reason)?;
    stream.write_all(&frame).await?;
    stream.flush().await?;
    stream.shutdown().await
}
