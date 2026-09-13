//! R2 — integration smoke test: `forge_sharecli` `ShareHub` → `forge_tracera`
//! `TraceraSink`, over real HTTP.
//!
//! Proves the two P3 surfaces compose end-to-end: a `ShareHub` subscriber on a
//! topic receives published `ShareMessage`s, a forwarding bridge converts each
//! message into a `TraceraEvent`, and the sink transmits the JSON wire payload
//! to an in-process HTTP responder which records it and replies `200 OK`.
//!
//! This is the HTTP-transport companion to `composes_with_tracera.rs` (which
//! checks the wire-shape conversion + `MemoryStore` acceptance WITHOUT an HTTP
//! transport, per its header). Here we drive the actual `reqwest` request over
//! a `TcpListener` responder so the whole transmit-and-record path is covered.
//!
//! `forge_tracera` is a dev-dependency of `forge_sharecli` (see Cargo.toml), so
//! the tracera crate still builds standalone — this test only compiles when
//! running `cargo test -p forge_sharecli`.

use forge_sharecli::ShareHub;
use forge_tracera::{EventKind, SinkConfig, TelemetrySink, TraceraEvent, TraceraSink};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

// ---------------------------------------------------------------------------
// Minimal in-process HTTP/1.1 responder. Mirrors the helper used by
// forge_tracera's own tests (crates/forge_tracera/src/sink.rs) so the smoke
// test rides the exact same wire expectations the sink crate already verifies.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct CapturedRequest {
    headers: String,
    body: Vec<u8>,
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|l| {
            let lower = l.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

/// Accept one HTTP request on an ephemeral port, capture its headers + body,
/// reply `200 OK`, and ship the capture back via a oneshot channel.
async fn spawn_capture_server() -> (u16, oneshot::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = oneshot::channel();

    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = Vec::with_capacity(4096);
        let mut tmp = [0u8; 2048];
        // Read until we have headers and at least the content-length bytes.
        loop {
            let n = sock.read(&mut tmp).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(idx) = find_header_end(&buf) {
                let header_str = std::str::from_utf8(&buf[..idx]).unwrap_or("");
                let content_length = parse_content_length(header_str);
                if buf.len() >= idx + 4 + content_length {
                    break;
                }
            }
        }

        let header_end = find_header_end(&buf).unwrap_or(buf.len());
        let header_str =
            std::str::from_utf8(&buf[..header_end]).unwrap_or("").to_string();
        let content_length = parse_content_length(&header_str);
        let body_start = header_end + 4;
        let body = if buf.len() >= body_start + content_length {
            buf[body_start..body_start + content_length].to_vec()
        } else {
            Vec::new()
        };

        let _ = tx.send(CapturedRequest { headers: header_str, body });
        sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            .await
            .ok();
        sock.shutdown().await.ok();
    });

    (port, rx)
}

// ---------------------------------------------------------------------------
// The smoke test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sharehub_forwards_sequence_ordered_events_to_tracera_sink() {
    let n = 3usize;
    let topic = "traces";
    let expected_payloads = vec!["alpha", "beta", "gamma"];

    // (1) Local in-process HTTP responder that records whatever the sink sends.
    let (port, rx) = spawn_capture_server().await;
    let sink = TraceraSink::new(SinkConfig {
        endpoint: format!("http://127.0.0.1:{port}/v1/events"),
        // Plain JSON for this smoke test so the capture is directly parseable.
        compression_enabled: false,
        ..SinkConfig::default()
    })
    .expect("valid sink config must construct");

    // (2) Subscribe a ShareHub Subscriber on a topic.
    let hub = ShareHub::new();
    let mut sub = hub.subscribe(topic).expect("valid topic subscribes");

    // Bridge task: pull `n` messages off the hub, assert the seq assignment is
    // monotonic in publish order, then forward each as a TraceraEvent and flush.
    let forwarder = tokio::spawn(async move {
        let mut seen_seq = Vec::new();
        for _ in 0..n {
            let msg = sub.recv().await.expect("hub delivers the message");
            seen_seq.push(msg.seq);
            let ev = TraceraEvent::new("forge_sharecli", EventKind::Drift, msg.payload.clone())
                .with_session(topic)
                .with_tag("sharecli_seq", msg.seq.to_string());
            sink.submit(ev).await.expect("submit buffers the event");
        }
        assert_eq!(
            seen_seq,
            vec![1, 2, 3],
            "hub must assign sequence numbers monotonically in publish order"
        );
        sink.flush().await.expect("flush transmits the batch")
    });

    // (3) Publish `n` ShareMessages on the topic.
    for p in &expected_payloads {
        hub.publish_text(topic, *p).expect("publish succeeds");
    }

    // (4) The forwarder delivered all `n` events to the server (200 OK each).
    let delivered = forwarder.await.expect("bridge task completes");
    assert_eq!(delivered, n, "exactly the forwarded events must flush");

    // (5) Inspect what the server recorded — the JSON wire envelope.
    let cap = rx.await.expect("server captured the request");
    let headers = cap.headers.to_ascii_lowercase();
    assert!(
        headers.contains("content-type: application/json"),
        "expected JSON content type in headers:\n{headers}"
    );

    let wire: serde_json::Value =
        serde_json::from_slice(&cap.body).expect("sink body must be valid JSON");
    let events = wire
        .get("events")
        .and_then(serde_json::Value::as_array)
        .expect("wire envelope must have an events array");
    assert_eq!(events.len(), n, "server must record every forwarded event");

    let payloads: Vec<&str> = events
        .iter()
        .map(|e| {
            e.get("payload")
                .and_then(serde_json::Value::as_str)
                .expect("payload is a string")
        })
        .collect();
    assert_eq!(
        payloads,
        expected_payloads,
        "server must receive events in the original publish order"
    );

    // (6) Ordering/seq sanity: each wire event echoes its hub-assigned seq.
    for (i, ev) in events.iter().enumerate() {
        assert_eq!(
            ev.get("kind"),
            Some(&serde_json::Value::String("drift".to_string())),
            "event kind must round-trip as drift"
        );
        assert_eq!(
            ev.get("session_id"),
            Some(&serde_json::Value::String(topic.to_string())),
            "session id must round-trip"
        );
        let seq_tag = ev
            .get("tags")
            .and_then(|t| t.get("sharecli_seq"))
            .and_then(serde_json::Value::as_str)
            .expect("sharecli_seq tag present");
        assert_eq!(
            seq_tag,
            (i as u64 + 1).to_string(),
            "server must see hub sequence numbers in ascending order"
        );
    }

    // (7) Sanity: the hub still owns the channel we published to.
    assert!(hub.has_channel(topic), "hub retains the channel after the test");
    assert_eq!(hub.topics(), vec![topic.to_string()]);
}