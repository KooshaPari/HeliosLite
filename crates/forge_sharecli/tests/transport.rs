//! Integration tests for `forge_sharecli::transport` — SSE and WebSocket.
//!
//! Tests in this file spin up the transports against an ephemeral
//! `TcpListener` on `127.0.0.1:0`, drive real TCP connections through
//! the hand-rolled wire protocols, and assert on the bytes observed at
//! the client side. No HTTP/WS framework is involved — only tokio.
//!
//! The wire-protocol tests below intentionally index into byte slices
//! whose bounds are proven correct by construction (length checks above
//! each access). We silence the workspace-wide `indexing_slicing` lint
//! at the file level rather than littering the tests with `get`/match.
#![allow(clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::Duration;

use forge_sharecli::transport::{sse, ws};
use forge_sharecli::{ShareHub, ShareMessage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Test utilities
// ---------------------------------------------------------------------------

/// Bind a TcpListener on 127.0.0.1:0 and return (listener, addr).
async fn bind_local() -> (TcpListener, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    (listener, addr)
}

/// Count non-overlapping occurrences of `needle` inside `haystack`.
fn count_subsequences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

/// Decode bytes to a lossy UTF-8 string. Wraps `bstr::ByteSlice::to_str_lossy`
/// because the workspace clippy lint forbids `String::from_utf8_lossy` on
/// byte-oriented paths. The slice form is required because
/// `bstr::ByteSlice` is only implemented for `[u8]`, not `Vec<u8>`.
fn decode_lossy(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    use bstr::ByteSlice;
    bytes.to_str_lossy()
}

// ---------------------------------------------------------------------------
// Required test #5 — hub_register_topic_and_topics_round_trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hub_register_topic_and_topics_round_trip() {
    let hub = ShareHub::new();
    assert!(hub.register_topic("alpha").is_ok());
    assert!(hub.register_topic("beta").is_ok());
    // Re-registering is idempotent.
    assert!(hub.register_topic("alpha").is_ok());

    let mut topics = hub.topics();
    topics.sort();
    assert_eq!(topics, vec!["alpha", "beta"]);

    // Invalid topics still surface errors.
    assert!(hub.register_topic("").is_err());
    assert!(hub.register_topic("bad\0topic").is_err());
}

// ---------------------------------------------------------------------------
// Required test #1 — sse_get_streams_messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_get_streams_messages() {
    let hub = Arc::new(ShareHub::new());
    let (listener, addr) = bind_local().await;

    let hub_for_server = hub.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        sse::serve_sse(hub_for_server, stream)
            .await
            .expect("serve_sse");
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");
    let request = b"GET /sse/foo HTTP/1.1\r\nHost: localhost\r\n\r\n";
    client.write_all(request).await.expect("write request");

    // Read until we have the full headers.
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let header_deadline = Duration::from_secs(5);
    loop {
        let n = timeout(header_deadline, client.read(&mut tmp))
            .await
            .expect("header read timed out")
            .expect("header read");
        if n == 0 {
            panic!("server closed before sending headers");
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let header_text = decode_lossy(&buf[..header_end]);
    assert!(
        header_text.starts_with("HTTP/1.1 200 OK"),
        "got: {header_text}"
    );
    assert!(header_text.contains("text/event-stream"));
    assert!(header_text.contains("Transfer-Encoding: chunked"));

    let mut body = buf[header_end + 4..].to_vec();

    // Publish 3 messages via the hub.
    for i in 0..3 {
        hub.publish_text("foo", format!("msg-{i}"))
            .expect("publish");
    }

    let deadline = Duration::from_secs(5);
    loop {
        let count = count_subsequences(&body, b"event: message");
        if count >= 3 {
            break;
        }
        let mut tmp = [0u8; 4096];
        let n = timeout(deadline, client.read(&mut tmp))
            .await
            .expect("body read timed out")
            .expect("body read");
        if n == 0 {
            panic!(
                "server closed before all messages arrived; got body: {}",
                decode_lossy(&body)
            );
        }
        body.extend_from_slice(&tmp[..n]);
    }

    // Split on the SSE terminator `\n\n`. Each frame: `id:...\nevent: message\ndata: ...\n\n`.
    let body_str = std::str::from_utf8(&body).expect("body is utf8");
    let chunks: Vec<&str> = body_str
        .split("\n\n")
        .filter(|s| s.contains("data: "))
        .collect();
    assert!(
        chunks.len() >= 3,
        "expected >= 3 SSE frames, got {}; body = {body_str}",
        chunks.len()
    );

    let mut found_ids = 0;
    let mut found_event = false;
    for seg in &chunks {
        if seg.contains("id: ") {
            found_ids += 1;
        }
        if seg.contains("event: message") {
            found_event = true;
        }
    }
    assert!(found_ids >= 3, "expected at least 3 id: fields");
    assert!(found_event, "expected event: message field");

    drop(client);
    timeout(Duration::from_secs(5), server)
        .await
        .expect("server task exit")
        .expect("server join");
}

// ---------------------------------------------------------------------------
// Required test #2 — sse_post_publishes_to_hub
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sse_post_publishes_to_hub() {
    let hub = Arc::new(ShareHub::new());
    let (listener, addr) = bind_local().await;

    let mut sub = hub.subscribe("foo").expect("subscribe");

    let hub_for_server = hub.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        sse::serve_sse(hub_for_server, stream)
            .await
            .expect("serve_sse");
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");

    let payload = serde_json::to_vec(&ShareMessage::text("foo", "hello-post")).expect("encode");
    let mut req = format!(
        "POST /publish/foo HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n",
        payload.len()
    )
    .into_bytes();
    req.extend_from_slice(&payload);

    client.write_all(&req).await.expect("write POST");

    let mut response = Vec::new();
    let deadline = Duration::from_secs(5);
    loop {
        let mut tmp = [0u8; 1024];
        let n = timeout(deadline, client.read(&mut tmp))
            .await
            .expect("read timed out")
            .expect("read");
        if n == 0 {
            break;
        }
        response.extend_from_slice(&tmp[..n]);
        if response.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let resp_str = decode_lossy(&response);
    assert!(
        resp_str.starts_with("HTTP/1.1 202 Accepted"),
        "unexpected response: {resp_str}"
    );

    let got = timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("subscriber recv timed out")
        .expect("subscriber recv");
    assert_eq!(got.payload, serde_json::json!("hello-post"));
    assert_eq!(got.topic, "foo");

    drop(client);
    let _ = timeout(Duration::from_secs(5), server).await;
}

// ---------------------------------------------------------------------------
// Required test #3 — ws_handshake_and_echo
// ---------------------------------------------------------------------------

const WS_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

/// Encode a masked client→server text frame (opcode 0x1).
fn encode_client_text_frame(key: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x81);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(key);
    let mut masked = payload.to_vec();
    for (i, b) in masked.iter_mut().enumerate() {
        *b ^= key[i & 3];
    }
    out.extend_from_slice(&masked);
    out
}

/// Encode a client→server close frame (opcode 0x8).
fn build_close(key: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x88);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(key);
    let mut masked = payload.to_vec();
    for (i, b) in masked.iter_mut().enumerate() {
        *b ^= key[i & 3];
    }
    out.extend_from_slice(&masked);
    out
}

#[tokio::test]
async fn ws_handshake_and_echo() {
    let hub = Arc::new(ShareHub::new());
    let (listener, addr) = bind_local().await;

    let hub_for_server = hub.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        ws::serve_ws(hub_for_server, stream)
            .await
            .expect("serve_ws");
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");

    // WebSocket handshake (RFC 6455 §1.3 example).
    let handshake = format!(
        "GET /ws/echo HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {WS_KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    client
        .write_all(handshake.as_bytes())
        .await
        .expect("write handshake");

    let mut buf = Vec::new();
    let deadline = Duration::from_secs(5);
    loop {
        let mut tmp = [0u8; 1024];
        let n = timeout(deadline, client.read(&mut tmp))
            .await
            .expect("handshake read timed out")
            .expect("handshake read");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let resp = decode_lossy(&buf);
    assert!(
        resp.starts_with("HTTP/1.1 101 Switching Protocols"),
        "handshake failed: {resp}"
    );
    assert!(resp.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));

    // Subscribe BEFORE the inbound publish so we observe the round-trip.
    let mut sub = hub.subscribe("echo").expect("subscribe");

    // First inbound frame: triggers hub publish + server-side subscriber
    // gets the same message and forwards it back as a text frame.
    let envelope = serde_json::json!({
        "topic": "echo",
        "message": {
            "id": uuid::Uuid::new_v4().to_string(),
            "topic": "echo",
            "payload": {"hello": "ws"},
            "created_at": chrono::Utc::now(),
            "seq": 0,
        }
    });
    let envelope_str = serde_json::to_string(&envelope).expect("encode");
    let key = [0x01u8, 0x02, 0x03, 0x04];
    let frame = encode_client_text_frame(&key, envelope_str.as_bytes());
    client.write_all(&frame).await.expect("write frame");

    // The subscriber should observe the publish immediately.
    let got = timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("sub recv timed out")
        .expect("sub recv");
    assert_eq!(got.topic, "echo");

    // The server also subscribed to "echo", so the same message will be
    // relayed back to us as a text frame. Read until we see one.
    let mut rx_buf = Vec::new();
    let read_deadline = Duration::from_secs(3);
    let mut saw_payload = false;
    while !saw_payload {
        let mut tmp = [0u8; 4096];
        let n = match timeout(read_deadline, client.read(&mut tmp)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => panic!("read failed: {e}"),
            Err(_) => panic!(
                "outbound read timed out; rx_buf so far ({} bytes) = {}",
                rx_buf.len(),
                decode_lossy(&rx_buf)
            ),
        };
        if n == 0 {
            panic!(
                "server closed before sending echo; rx_buf so far ({} bytes) = {}",
                rx_buf.len(),
                decode_lossy(&rx_buf)
            );
        }
        rx_buf.extend_from_slice(&tmp[..n]);

        // Walk any complete frames we now have.
        let mut pos = 0;
        while pos + 2 <= rx_buf.len() {
            let b0 = rx_buf[pos];
            let b1 = rx_buf[pos + 1];
            let masked = (b1 & 0x80) != 0;
            let mut len = (b1 & 0x7F) as usize;
            let mut header_len = 2;
            if len == 126 {
                if pos + 4 > rx_buf.len() {
                    break;
                }
                len = u16::from_be_bytes([rx_buf[pos + 2], rx_buf[pos + 3]]) as usize;
                header_len = 4;
            } else if len == 127 {
                if pos + 10 > rx_buf.len() {
                    break;
                }
                let mut buf8 = [0u8; 8];
                buf8.copy_from_slice(&rx_buf[pos + 2..pos + 10]);
                len = u64::from_be_bytes(buf8) as usize;
                header_len = 10;
            }
            if pos + header_len + len > rx_buf.len() {
                break; // incomplete frame
            }
            let payload_start = pos + header_len;
            let payload = &rx_buf[payload_start..payload_start + len];
            if (b0 & 0x0F) == 0x1 {
                // Server frames are NOT masked.
                assert!(!masked, "server→client frame must not be masked");
                let s = std::str::from_utf8(payload).unwrap_or("");
                if s.contains("\"hello\"") || s.contains("hello") {
                    saw_payload = true;
                }
            }
            pos = payload_start + len;
        }
    }

    assert!(
        saw_payload,
        "expected at least one server text frame echoing our envelope; got {} bytes",
        rx_buf.len()
    );

    drop(client);
    let _ = timeout(Duration::from_secs(5), server).await;
}

// ---------------------------------------------------------------------------
// Required test #4 — ws_close_propagates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ws_close_propagates() {
    let hub = Arc::new(ShareHub::new());
    let (listener, addr) = bind_local().await;

    let hub_for_server = hub.clone();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        ws::serve_ws(hub_for_server, stream).await
    });

    let mut client = TcpStream::connect(addr).await.expect("connect");
    let handshake = format!(
        "GET /ws/foo HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {WS_KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    client
        .write_all(handshake.as_bytes())
        .await
        .expect("write handshake");

    let mut buf = Vec::new();
    let deadline = Duration::from_secs(5);
    loop {
        let mut tmp = [0u8; 1024];
        let n = timeout(deadline, client.read(&mut tmp))
            .await
            .expect("handshake read")
            .expect("handshake read");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let resp = decode_lossy(&buf);
    assert!(resp.starts_with("HTTP/1.1 101"), "got: {resp}");

    let key = [0x10u8, 0x20, 0x30, 0x40];
    let mut close_payload = Vec::new();
    close_payload.extend_from_slice(&1000u16.to_be_bytes());
    close_payload.extend_from_slice(b"bye");
    let close = build_close(&key, &close_payload);
    client.write_all(&close).await.expect("write close");

    // Read the close frame from the server (the server writes nothing
    // else on a clean close-from-peer).
    let mut rx = [0u8; 16];
    let n = timeout(Duration::from_secs(3), client.read(&mut rx))
        .await
        .expect("server close timed out")
        .expect("server close read");
    assert!(n >= 2, "server should have sent a close frame");
    let b0 = rx[0];
    let b1 = rx[1];
    let masked = (b1 & 0x80) != 0;
    assert!(!masked, "server→client must not mask");
    let opcode = b0 & 0x0F;
    assert_eq!(opcode, 0x8, "expected close opcode, got 0x{opcode:x}");

    let server_result = timeout(Duration::from_secs(5), server)
        .await
        .expect("server task join timed out")
        .expect("server task panicked");
    assert!(
        server_result.is_ok(),
        "serve_ws should return Ok(()) on peer close: {server_result:?}"
    );
}
