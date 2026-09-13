//! Integration smoke test — proves that `forge_sharecli`'s `ShareHub`
//! composes with `forge_tracera`'s `TraceraSink` end-to-end.
//!
//! The two crates do not have a hard dependency on each other at runtime:
//! `ShareHub` is the in-process broadcast substrate and `TraceraSink` is
//! the outbound HTTP telemetry sink. Both speak `serde_json::Value`
//! shaped payloads, so the wiring is at the **envelope level**: a
//! `ShareMessage` is wrapped into a `TraceraEvent` (preserving the
//! original payload as `payload`), and the resulting event is submitted
//! to the sink. This file demonstrates the surface interop without
//! requiring an HTTP transport.
//!
//! Tests in this file exercise three things:
//!
//! 1. `ShareHub::publish` round-trips through a subscriber (sanity).
//! 2. The conversion function `share_message_to_tracera_event` produces
//!    a well-formed `TraceraEvent` from a `ShareMessage` (the wire shapes
//!    align).
//! 3. The resulting `TraceraEvent` is accepted by a `TraceraSink` and
//!    its underlying `MemoryStore` holds the event with the expected
//!    fields. This proves the data shape propagates through both crates.
//!
//! Index-based assertions below have length-guarded preconditions; we
//! silence the workspace-wide `indexing_slicing` lint at the file level
//! rather than refactoring already-stable tests.
#![allow(clippy::indexing_slicing)]

use forge_sharecli::{ShareHub, ShareMessage};
use forge_tracera::{
    AuthMode, EventKind, MemoryStore, SinkConfig, StoreHandle, TelemetrySink, TraceraEvent,
    TraceraSink,
};
use std::collections::BTreeMap;

/// Source identifier stamped on every conversion — matches
/// `SinkConfig::default().source`.
const SOURCE: &str = "forgecode";

/// Convert a [`ShareMessage`] into a [`TraceraEvent`].
///
/// The conversion preserves the original payload as the event's
/// structured `payload`, and stashes the originating topic + sequence +
/// share-message-id into the event's `tags` map (BTreeMap, low-cardinality
/// by convention) so downstream collectors can correlate events back to
/// their ShareHub topic without re-encoding the payload.
///
/// Wire-shape contract (this function is the proof it holds):
///
/// ```json
/// {
///   "schema": "tracera.v1",
///   "id": "<ulid>",
///   "ts": "<rfc3339>",
///   "source": "forgecode",
///   "session_id": null,
///   "kind": "custom",
///   "payload": <original sharecli_payload>,
///   "tags": { "sharecli.topic": "...", "sharecli.seq": "...", "sharecli.id": "..." }
/// }
/// ```
fn share_message_to_tracera_event(msg: &ShareMessage) -> TraceraEvent {
    let mut tags = BTreeMap::new();
    // Low-cardinality tags — strip whitespace/control chars would belong
    // upstream; the topic validator already rejects NUL.
    tags.insert("sharecli.topic".to_string(), msg.topic.clone());
    tags.insert("sharecli.seq".to_string(), msg.seq.to_string());
    tags.insert("sharecli.id".to_string(), msg.id.clone());

    TraceraEvent::new(SOURCE, EventKind::Custom, msg.payload.clone()).with_tags_map(tags)
}

// Helper: keep the call sites readable. `TraceraEvent::with_tag` takes
// a single (k, v) pair; a small wrapper lets us seed multiple tags in
// one chain without forcing every test to repeat the boilerplate.
trait TraceraEventExt {
    fn with_tags_map(self, tags: BTreeMap<String, String>) -> Self;
}

impl TraceraEventExt for TraceraEvent {
    fn with_tags_map(mut self, tags: BTreeMap<String, String>) -> Self {
        for (k, v) in tags {
            self = self.with_tag(k, v);
        }
        self
    }
}

/// Subscribe to a brand-new `ShareHub` and publish a single
/// `ShareMessage`. The subscriber must observe the message verbatim.
///
/// This is the baseline sanity test that the local fanout substrate is
/// working before we exercise the cross-crate interop.
#[tokio::test]
async fn hub_publish_subscribe_roundtrip() {
    let hub = ShareHub::new();
    let mut sub = hub
        .subscribe("test")
        .expect("subscribe to a fresh topic should succeed");

    let msg = ShareMessage::json("test", &serde_json::json!({ "hello": "world", "n": 42u32 }));
    let n = hub.publish(msg.clone()).expect("publish should succeed");
    assert_eq!(n, 1, "exactly one subscriber should receive the message");

    let received = sub
        .recv()
        .await
        .expect("subscriber should receive the published message");
    assert_eq!(received.payload, msg.payload);
    assert_eq!(received.topic, "test");
    assert_eq!(received.id, msg.id);
    assert!(received.seq >= 1);
}

/// `share_message_to_tracera_event` preserves the original payload and
/// stamps the sharecli correlation tags onto the envelope.
///
/// The unit test asserts the wire-shape contract documented above.
#[test]
fn conversion_preserves_payload_and_stamps_correlation_tags() {
    let msg = ShareMessage::json(
        "agent.events",
        &serde_json::json!({ "tool": "fs.read", "path": "/etc/hosts" }),
    );
    let topic = msg.topic.clone();
    let id = msg.id.clone();
    let seq = msg.seq;

    let event = share_message_to_tracera_event(&msg);

    // Wire-shape assertions
    assert_eq!(event.schema, "tracera.v1");
    assert_eq!(event.source, SOURCE);
    assert_eq!(event.kind, EventKind::Custom);
    // Payload is moved through unchanged — this is the critical
    // interop guarantee.
    assert_eq!(event.payload, msg.payload);

    // Correlation tags are present and stable.
    assert_eq!(
        event.tags.get("sharecli.topic").map(String::as_str),
        Some(topic.as_str())
    );
    assert_eq!(
        event.tags.get("sharecli.id").map(String::as_str),
        Some(id.as_str())
    );
    assert_eq!(
        event.tags.get("sharecli.seq").map(String::as_str),
        Some(seq.to_string().as_str()),
        "sharecli.seq tag must reflect the channel-assigned seq"
    );

    // ts is stamped at conversion time — present and a valid epoch
    // second. We avoid `Utc::now()` here so the test does not need the
    // `chrono` `clock` feature on the dev-dependency surface.
    assert!(
        event.ts.timestamp() > 0,
        "event timestamp should be a valid epoch second (got {})",
        event.ts.timestamp()
    );
}

/// Forwarding a `ShareMessage` through the conversion + a `TraceraSink`
/// (without flushing over HTTP) lands the resulting `TraceraEvent` in
/// the sink's underlying `MemoryStore`. We then drain the store and
/// assert the event round-trips with the original payload.
///
/// This is the proof the surfaces compose: a `ShareMessage` published
/// on a `ShareHub` can be converted to a `TraceraEvent` and submitted to
/// the telemetry sink with no data-shape loss. The HTTP transport is
/// not exercised — that's a separate concern, owned by the sink.
#[tokio::test]
async fn share_message_can_be_forwarded_into_tracera_sink_store() {
    // Step 1: build a ShareHub, publish a ShareMessage, and drain it
    // through a subscriber — proves the hub is producing valid
    // messages.
    let hub = ShareHub::new();
    let mut sub = hub.subscribe("bridge").expect("subscribe");
    let original = ShareMessage::json(
        "bridge",
        &serde_json::json!({
            "channel": "agent-7",
            "tokens": 1284,
            "tool": "fs.read",
        }),
    );
    let n = hub.publish(original.clone()).expect("publish");
    assert_eq!(n, 1);
    let received = sub.recv().await.expect("subscriber recv");
    assert_eq!(received.payload, original.payload);

    // Step 2: convert the ShareMessage into a TraceraEvent.
    let event = share_message_to_tracera_event(&received);

    // Step 3: build a TraceraSink pointed at a mock endpoint (the
    // endpoint URL is never contacted because we never call flush).
    let sink = TraceraSink::new(SinkConfig {
        endpoint: "http://127.0.0.1:1/v1/events".to_string(),
        auth: AuthMode::None,
        // Keep capacity small — the store we hand-construct below is
        // used for direct assertions, so we don't need to rely on
        // the sink's internal store for the data round-trip.
        capacity: 8,
        ..SinkConfig::default()
    })
    .expect("sink config should validate");

    // Step 4: submit the event. The sink's `submit` is non-network:
    // it lands the event in its underlying MemoryStore (or returns
    // StoreFull on capacity issues).
    sink.submit(event.clone())
        .await
        .expect("submit should succeed");

    // Step 5: drain the sink's store and assert the event is there
    // with its payload + correlation tags intact.
    let store_handle = sink.store();
    assert_eq!(
        store_handle.len(),
        1,
        "exactly one event should be buffered in the sink's MemoryStore"
    );
    let drained = store_handle.drain_batch(8);
    assert_eq!(drained.len(), 1);
    let stored = &drained[0];

    // Wire-shape guarantees: the original payload made it through.
    assert_eq!(stored.payload, original.payload);
    // Source is stamped from the sink config — proves the envelope
    // was processed through the sink (not just round-tripped past it).
    assert_eq!(stored.source, SOURCE);
    // The correlation tags from `share_message_to_tracera_event` are
    // preserved through submit/drain.
    assert_eq!(
        stored.tags.get("sharecli.topic").map(String::as_str),
        Some("bridge"),
        "sharecli.topic correlation tag must survive the sink store"
    );
    assert_eq!(
        stored.tags.get("sharecli.id").map(String::as_str),
        Some(received.id.as_str()),
        "sharecli.id correlation tag must survive the sink store"
    );
}

/// Two `ShareMessage`s published back-to-back can both be converted
/// and submitted in order. The store should drain them in submission
/// order with their original payloads preserved.
#[tokio::test]
async fn multiple_share_messages_preserve_order_in_sink_store() {
    let hub = ShareHub::new();
    let mut sub = hub.subscribe("stream").expect("subscribe");

    let msgs: Vec<ShareMessage> = (0..5)
        .map(|i| {
            ShareMessage::json(
                "stream",
                &serde_json::json!({ "i": i, "marker": format!("m-{i}") }),
            )
        })
        .collect();
    for m in &msgs {
        hub.publish(m.clone()).expect("publish");
    }

    let mut received = Vec::new();
    for _ in 0..msgs.len() {
        received.push(sub.recv().await.expect("recv"));
    }
    // The seq field is monotonic per channel — sanity check.
    for w in received.windows(2) {
        assert!(
            w[1].seq > w[0].seq,
            "sharecli.seq should be monotonic across publishes"
        );
    }

    let sink = TraceraSink::new(SinkConfig {
        endpoint: "http://127.0.0.1:1/v1/events".to_string(),
        auth: AuthMode::None,
        capacity: 16,
        ..SinkConfig::default()
    })
    .expect("sink config should validate");

    for m in &received {
        let event = share_message_to_tracera_event(m);
        sink.submit(event).await.expect("submit");
    }

    let store: StoreHandle = sink.store();
    assert_eq!(store.len(), received.len());
    let drained = store.drain_batch(received.len());
    assert_eq!(drained.len(), received.len());

    // Order: the i-th drained event should correspond to the i-th
    // received ShareMessage.
    for (i, (msg, ev)) in received.iter().zip(drained.iter()).enumerate() {
        assert_eq!(ev.payload, msg.payload, "payload mismatch at index {i}");
        assert_eq!(
            ev.tags.get("sharecli.seq").map(String::as_str),
            Some(msg.seq.to_string().as_str()),
            "seq tag mismatch at index {i}"
        );
    }
}

/// A hand-constructed `MemoryStore` (independent of `TraceraSink`) is
/// used here to assert that the conversion function produces a value
/// that any `MemoryStore`-backed pipeline will accept. This mirrors
/// what `TraceraSink::submit` does internally and lets the test fail
/// loudly if the conversion ever produces a value the store rejects.
#[test]
fn converted_event_fits_in_a_standalone_memory_store() {
    let msg = ShareMessage::json(
        "store.direct",
        &serde_json::json!({ "k": "v", "list": [1, 2, 3] }),
    );
    let event = share_message_to_tracera_event(&msg);

    let store = MemoryStore::new(4);
    store
        .submit(event.clone(), false)
        .expect("store should accept the converted event");

    assert_eq!(store.len(), 1);
    let drained = store.drain_batch(4);
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].payload, event.payload);
    assert_eq!(
        drained[0].tags.get("sharecli.topic").map(String::as_str),
        Some("store.direct")
    );
}
