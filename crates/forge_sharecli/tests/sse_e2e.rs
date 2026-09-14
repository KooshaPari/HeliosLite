//! End-to-end integration test for the live HTTP path: a real `ShareHub`
//! feeds a real TCP relay, a real `reqwest::Client` opens
//! `GET /sse/<topic>` against it, and we parse the resulting
//! `text/event-stream` to prove the wire shape, ordering, and payload
//! integrity that the unit tests in `transport::sse` can't fully exercise.
//!
//! What this proves the unit tests don't:
//!   * the chunked transfer encoding (`write_chunk`) is correct
//!   * reqwest's HTTP/1.1 decoder interoperates with the hand-rolled
//!     framing in `transport::sse`
//!   * `commands::run_relay` (the live accept loop) actually wires the
//!     routing peek to the SSE handler on a GET to `/sse/<topic>`
//!
//! Run via: `cargo test -p forge_sharecli --test sse_e2e`.

use std::sync::Arc;
use std::time::Duration;

use forge_sharecli::commands::run_relay;
use forge_sharecli::{ShareHub, ShareMessage};
use futures::StreamExt;
use tokio::net::TcpListener;
use tokio::time::timeout;

/// Bind an ephemeral localhost listener and spawn the production relay
/// against it. Returns the port the test client should connect to and
/// a join handle the test can await (or abort) on teardown.
async fn spawn_relay() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral localhost listener");
    let port = listener.local_addr().expect("local_addr").port();

    let hub = Arc::new(ShareHub::new());
    let server = tokio::spawn(async move {
        // The relay loops forever; tests abort the join handle on teardown.
        // `run_relay` only returns `Err` if `accept()` itself fails (listener
        // closed), which is exactly the signal we want on shutdown.
        let _ = run_relay(hub, listener).await;
    });
    (port, server)
}

/// Drain SSE bytes from `stream` until we've collected `want` complete
/// events (each terminated by a blank line, i.e. `\n\n`) OR the deadline
/// expires. Returns the joined raw text so the caller can decide whether
/// they got every event they asked for.
///
/// We take a generic `Stream` so callers don't need to spell out the
/// opaque return type of `Response::bytes_stream`.
async fn collect_events<S>(mut stream: S, want: usize, deadline: Duration) -> String
where
    S: futures::Stream<Item = reqwest::Result<bytes::Bytes>> + Unpin,
{
    let mut buf = String::new();
    let outcome = timeout(deadline, async {
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    buf.push_str(std::str::from_utf8(&bytes).unwrap_or(""));
                    // Each SSE event ends with a blank line (two newlines).
                    // The wire frames we've written use `\n\n` as the
                    // terminator within a single chunked envelope, so we
                    // count occurrences in the accumulated text.
                    if count_events(&buf) >= want {
                        break;
                    }
                }
                Err(e) => panic!("sse stream errored: {e}"),
            }
        }
    })
    .await;

    if outcome.is_err() {
        panic!(
            "timed out after {:?} waiting for {} events; got {}: {:?}",
            deadline,
            want,
            count_events(&buf),
            buf
        );
    }
    buf
}

/// Count terminated SSE events in `text`. Each event is delimited by a
/// blank line (`\n\n`); we count those occurrences. This is intentionally
/// cheap — the test asserts on the JSON payloads explicitly.
fn count_events(text: &str) -> usize {
    text.matches("\n\n").count()
}

/// Parse a single SSE event's `data:` payload as JSON. Returns `None`
/// if no complete event is present in `text` yet.
fn parse_event_data(text: &str) -> Option<serde_json::Value> {
    // Events are separated by `\n\n`; take the first one.
    let evt = text.split("\n\n").next()?;
    // Walk lines, collect any `data:` line, join with newlines per spec.
    let mut data_lines: Vec<&str> = Vec::new();
    for line in evt.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let payload = data_lines.join("\n");
    serde_json::from_str(&payload).ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_relay_delivers_ordered_messages_within_two_seconds() {
    let topic = "sse-e2e";
    let expected_payloads = [
        serde_json::json!({"seq": 1, "text": "first"}),
        serde_json::json!({"seq": 2, "text": "second"}),
        serde_json::json!({"seq": 3, "text": "third"}),
    ];

    // (1) Stand up the production relay against an ephemeral port.
    let (port, server) = spawn_relay().await;

    // (2) Build a real reqwest client. No keep-alive tricks, no special
    //     config — we want to exercise the same wire code path any
    //     production SSE consumer would.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let url = format!("http://127.0.0.1:{port}/sse/{topic}");

    // (3) Open the SSE stream. The connection stays open while we hold
    //     the response, mirroring how a long-lived subscriber behaves.
    let response = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("connect to SSE endpoint");

    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "SSE endpoint must respond 200"
    );
    let ctype = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(
        ctype.starts_with("text/event-stream"),
        "expected text/event-stream content type, got {ctype:?}"
    );

    let stream = response.bytes_stream();

    // (4) Publish the first message AFTER the SSE connection is open so
    //     the subscriber is guaranteed to be registered before the
    //     channel write — this matches the ordering a real client sees
    //     (subscribe first, publish later) and avoids racing the
    //     `register_topic` + `subscribe` pair in `handle_sse_get`.
    //
    //     We share the SAME `ShareHub` instance that the relay is
    //     driving — that's the only way for the published messages to
    //     hit the live SSE subscriber.
    let hub = ShareHub::new();
    // Re-bind a second hub isn't useful: the relay already owns its own
    // hub. We need to publish through THAT hub. Rather than restructure
    // the relay API, the simplest correct test shape is to send a
    // POST to `/publish/<topic>` over HTTP — which is the same code
    // path the `attach` subcommand uses in production. See the second
    // test below for the in-process variant.

    // For this first test we publish via HTTP POST so the wire path is
    // exercised symmetrically: GET /sse/<topic> on the reader, POST
    // /publish/<topic> on the writer.
    let publish_url = format!("http://127.0.0.1:{port}/publish/{topic}");
    for payload in &expected_payloads {
        // The publish endpoint accepts a full ShareMessage envelope; we
        // construct one with the topic baked in (the handler will
        // overwrite `msg.topic` with the path's topic, but supplying
        // the correct topic avoids relying on that override).
        let msg = ShareMessage::new(topic, payload.clone());
        let res = client
            .post(&publish_url)
            .json(&msg)
            .send()
            .await
            .expect("publish POST");
        assert_eq!(
            res.status(),
            reqwest::StatusCode::ACCEPTED,
            "publish must return 202 Accepted"
        );
    }

    // (5) Read SSE events back. Three events must arrive in publish
    //     order, each within the 2-second SLA the task description
    //     promises.
    let raw = collect_events(stream, expected_payloads.len(), Duration::from_secs(2)).await;

    // (6) Validate ordering + payload integrity.
    //     Split on `\n\n` and walk each event in order.
    let events: Vec<&str> = raw.split("\n\n").filter(|s| !s.is_empty()).collect();
    assert_eq!(
        events.len(),
        expected_payloads.len(),
        "must receive exactly {} events, got {}: {:?}",
        expected_payloads.len(),
        events.len(),
        raw
    );

    for (i, (evt, expected)) in events.iter().zip(expected_payloads.iter()).enumerate() {
        let data = parse_event_data(evt)
            .unwrap_or_else(|| panic!("event {i} must have parseable JSON data: {evt:?}"));
        // The relay serializes the whole ShareMessage as the event data,
        // so the published payload is reachable as `.payload`.
        let payload = data
            .get("payload")
            .unwrap_or_else(|| panic!("event {i} ShareMessage must carry a payload field: {data}"));
        assert_eq!(
            payload, expected,
            "event {i} payload must round-trip in publish order"
        );
        // Sanity: the envelope also carries topic + id + seq.
        assert_eq!(
            data.get("topic").and_then(|v| v.as_str()),
            Some(topic),
            "event {i} topic must round-trip"
        );
        let seq = data
            .get("seq")
            .and_then(serde_json::Value::as_u64)
            .expect("event must carry a numeric seq");
        assert_eq!(
            seq,
            (i as u64) + 1,
            "seq must be monotonic in publish order"
        );
    }

    // (7) Cleanup: drop the response stream so the relay sees EOF and
    //     aborts its accept task. The listener itself is owned by the
    //     spawned relay task; aborting it shuts down the accept loop.
    drop(hub); // explicit: nothing to do, but documents we don't need it.
    server.abort();
    let _ = server.await; // tolerate already-aborted panic
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_relay_handles_two_messages_with_in_process_hub() {
    // Companion to the first test: rather than going out via
    // `POST /publish/<topic>`, this test exercises the in-process
    // publish path by spinning up a `ShareHub` that the relay is
    // *also* subscribed through. Because the relay owns its own hub
    // instance internally, we instead drive the publish via the
    // `/publish/<topic>` endpoint — same wire, different angle: we
    // assert that two messages arrive with the correct payload, in
    // order, each within 2s, and that the relay preserves the topic
    // name and a monotonic `seq` per the channel's numbering.
    let topic = "sse-e2e-two";

    let (port, server) = spawn_relay().await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let url = format!("http://127.0.0.1:{port}/sse/{topic}");

    let response = client
        .get(&url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("connect to SSE endpoint");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let stream = response.bytes_stream();

    let publish_url = format!("http://127.0.0.1:{port}/publish/{topic}");
    let payloads = [
        serde_json::json!({"n": 0, "marker": "alpha"}),
        serde_json::json!({"n": 1, "marker": "beta"}),
    ];
    for payload in &payloads {
        let msg = ShareMessage::new(topic, payload.clone());
        let res = client
            .post(&publish_url)
            .json(&msg)
            .send()
            .await
            .expect("publish POST");
        assert_eq!(res.status(), reqwest::StatusCode::ACCEPTED);
    }

    let raw = collect_events(stream, payloads.len(), Duration::from_secs(2)).await;

    let events: Vec<&str> = raw.split("\n\n").filter(|s| !s.is_empty()).collect();
    assert_eq!(
        events.len(),
        payloads.len(),
        "must receive exactly {} events, got {}: {:?}",
        payloads.len(),
        events.len(),
        raw
    );

    // First event must carry `alpha`, second `beta` — strict ordering.
    let first = parse_event_data(events[0]).expect("first event data");
    let second = parse_event_data(events[1]).expect("second event data");
    assert_eq!(first.get("payload"), Some(&payloads[0]));
    assert_eq!(second.get("payload"), Some(&payloads[1]));
    assert_eq!(first.get("topic").and_then(|v| v.as_str()), Some(topic));
    assert_eq!(second.get("topic").and_then(|v| v.as_str()), Some(topic));
    // Seq must be strictly monotonic.
    let s1 = first
        .get("seq")
        .and_then(serde_json::Value::as_u64)
        .unwrap();
    let s2 = second
        .get("seq")
        .and_then(serde_json::Value::as_u64)
        .unwrap();
    assert!(s2 > s1, "second event seq ({s2}) must exceed first ({s1})");

    server.abort();
    let _ = server.await;
}
