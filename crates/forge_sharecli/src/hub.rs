//! `ShareHub` — central registry for ShareCLI channels, queues, and fanout.
//!
//! `ShareHub` is the in-process realtime relay. It owns all `Channel`s
//! (broadcast pub/sub) and can mint `Queue`s (single-consumer bounded FIFOs).
//! It is `Clone` (internally `Arc`-backed) so it can be shared across tasks
//! without extra wrapping.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::channel::{Channel, Subscriber};
use crate::error::ShareError;
use crate::message::ShareMessage;
use crate::queue::{Queue, QueueConsumer, QueueProducer};

const DEFAULT_CHANNEL_CAP: usize = 256;
const DEFAULT_QUEUE_CAP: usize = 128;

/// Central hub — cheap to clone, safe to share across tasks.
///
/// All operations take `&self` (interior mutability) so a single `ShareHub`
/// can be held by the application root and cloned into every async task that
/// needs realtime publish/subscribe.
#[derive(Clone, Default)]
pub struct ShareHub {
    inner: Arc<HubInner>,
}

struct HubInner {
    channels: RwLock<HashMap<String, Arc<Channel>>>,
    default_channel_cap: usize,
    default_queue_cap: usize,
}

impl Default for HubInner {
    fn default() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            default_channel_cap: DEFAULT_CHANNEL_CAP,
            default_queue_cap: DEFAULT_QUEUE_CAP,
        }
    }
}

impl ShareHub {
    /// Create a hub with default capacities (256 for channels, 128 for queues).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a hub with custom default capacities.
    #[must_use]
    pub fn with_capacities(channel_cap: usize, queue_cap: usize) -> Self {
        Self {
            inner: Arc::new(HubInner {
                channels: RwLock::new(HashMap::new()),
                default_channel_cap: channel_cap.max(1),
                default_queue_cap: queue_cap.max(1),
            }),
        }
    }

    // ---- validation -------------------------------------------------------

    fn validate_topic(topic: &str) -> Result<(), ShareError> {
        if topic.is_empty() {
            return Err(ShareError::InvalidTopic { reason: "topic must be non-empty".to_string() });
        }
        if topic.contains('\0') {
            return Err(ShareError::InvalidTopic {
                reason: "topic must not contain NUL".to_string(),
            });
        }
        Ok(())
    }

    // ---- channels ---------------------------------------------------------

    /// Return the channel for `topic`, creating it if it does not yet exist.
    pub fn channel(&self, topic: &str) -> Result<Arc<Channel>, ShareError> {
        Self::validate_topic(topic)?;
        // fast path: read lock
        {
            let map = self.inner.channels.read();
            if let Some(ch) = map.get(topic) {
                return Ok(Arc::clone(ch));
            }
        }
        // slow path: write lock + double-check
        let mut map = self.inner.channels.write();
        if let Some(ch) = map.get(topic) {
            return Ok(Arc::clone(ch));
        }
        let ch = Arc::new(Channel::new(
            topic.to_string(),
            self.inner.default_channel_cap,
        ));
        map.insert(topic.to_string(), Arc::clone(&ch));
        Ok(ch)
    }

    /// Eagerly register a topic channel. Returns `Ok(())` whether or not the
    /// topic already existed — the operation is idempotent. Invalid topic
    /// names yield `ShareError::InvalidTopic`.
    ///
    /// This is the explicit counterpart to [`Self::channel`]: callers that
    /// want the topic to appear in [`Self::topics`] *before* a subscriber
    /// attaches (e.g. transport wiring that needs to enumerate topics up
    /// front) can use this method. Errors are returned only for malformed
    /// topic names; the default channel capacity is used.
    pub fn register_topic(&self, topic: impl Into<String>) -> Result<(), ShareError> {
        let topic = topic.into();
        Self::validate_topic(&topic)?;
        {
            let map = self.inner.channels.read();
            if map.contains_key(&topic) {
                return Ok(());
            }
        }
        let mut map = self.inner.channels.write();
        // entry().or_insert_with collapses the contains_key/insert pattern
        // clippy warns about. The closure may not run if the key exists,
        // and we ignore the return value — registration is a presence
        // assertion, the handle doesn't need to be returned.
        let _ = map.entry(topic.clone()).or_insert_with(|| {
            Arc::new(Channel::new(topic.clone(), self.inner.default_channel_cap))
        });
        Ok(())
    }

    /// Return the channel for `topic` with an explicit capacity when creating.
    /// If the channel already exists the `capacity` argument is ignored.
    pub fn channel_with_capacity(
        &self,
        topic: &str,
        capacity: usize,
    ) -> Result<Arc<Channel>, ShareError> {
        Self::validate_topic(topic)?;
        {
            let map = self.inner.channels.read();
            if let Some(ch) = map.get(topic) {
                return Ok(Arc::clone(ch));
            }
        }
        let mut map = self.inner.channels.write();
        if let Some(ch) = map.get(topic) {
            return Ok(Arc::clone(ch));
        }
        let ch = Arc::new(Channel::new(topic.to_string(), capacity.max(1)));
        map.insert(topic.to_string(), Arc::clone(&ch));
        Ok(ch)
    }

    /// Subscribe to `topic`. Creates the channel if needed.
    pub fn subscribe(&self, topic: &str) -> Result<Subscriber, ShareError> {
        Ok(self.channel(topic)?.subscribe())
    }

    /// Publish `msg` to its `msg.topic` channel. Returns number of receivers.
    pub fn publish(&self, msg: ShareMessage) -> Result<usize, ShareError> {
        let topic = msg.topic.clone();
        Self::validate_topic(&topic)?;
        self.channel(&topic)?.try_publish(msg)
    }

    /// Convenience: publish a JSON value to `topic`.
    pub fn publish_json<T: serde::Serialize>(
        &self,
        topic: &str,
        value: &T,
    ) -> Result<usize, ShareError> {
        Self::validate_topic(topic)?;
        self.channel(topic)?.publish_json(value)
    }

    /// Convenience: publish a text value to `topic`.
    pub fn publish_text(&self, topic: &str, text: impl Into<String>) -> Result<usize, ShareError> {
        Self::validate_topic(topic)?;
        self.channel(topic)?.publish_text(text)
    }

    /// Fan out `msg` to **every** existing channel (useful for global notices).
    /// Returns total receivers across all channels.
    pub fn broadcast(&self, payload: serde_json::Value) -> usize {
        let channels: Vec<Arc<Channel>> = {
            let map = self.inner.channels.read();
            map.values().cloned().collect()
        };
        let mut total = 0;
        for ch in channels {
            let msg = ShareMessage::new(ch.topic().to_string(), payload.clone());
            if let Ok(n) = ch.try_publish(msg) {
                total += n;
            }
        }
        total
    }

    /// List all known topic names.
    #[must_use]
    pub fn topics(&self) -> Vec<String> {
        let map = self.inner.channels.read();
        map.keys().cloned().collect()
    }

    /// Number of distinct channels currently registered.
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.inner.channels.read().len()
    }

    /// Whether a channel for `topic` exists.
    #[must_use]
    pub fn has_channel(&self, topic: &str) -> bool {
        self.inner.channels.read().contains_key(topic)
    }

    /// Remove the channel for `topic` (existing subscribers keep draining).
    /// Returns true if something was removed.
    pub fn remove_channel(&self, topic: &str) -> bool {
        self.inner.channels.write().remove(topic).is_some()
    }

    // ---- queues (factory) -------------------------------------------------

    /// Mint a new bounded queue for `topic` with default queue capacity.
    ///
    /// Each call creates a **new** queue — the hub does not deduplicate queues
    /// (queues have single-consumer semantics, so sharing would violate that).
    /// Callers that need a long-lived queue should hold onto the returned halves.
    #[must_use]
    pub fn create_queue(&self, topic: impl Into<String>) -> (QueueProducer, QueueConsumer) {
        self.create_queue_with_capacity(topic, self.inner.default_queue_cap)
    }

    /// Mint a new bounded queue with explicit capacity.
    #[must_use]
    pub fn create_queue_with_capacity(
        &self,
        topic: impl Into<String>,
        capacity: usize,
    ) -> (QueueProducer, QueueConsumer) {
        let mut q = Queue::new(topic, capacity);
        q.split()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hub_publish_subscribe_roundtrip() {
        let hub = ShareHub::new();
        let mut sub = hub.subscribe("news").unwrap();
        hub.publish_text("news", "breaking").unwrap();
        let msg = sub.recv().await.unwrap();
        assert_eq!(msg.payload.as_str().unwrap(), "breaking");
    }

    #[tokio::test]
    async fn hub_channel_singleton() {
        let hub = ShareHub::new();
        let a = hub.channel("t").unwrap();
        let b = hub.channel("t").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn hub_rejects_empty_topic() {
        let hub = ShareHub::new();
        assert!(hub.channel("").is_err());
        assert!(hub.publish_text("", "hi").is_err());
    }

    #[tokio::test]
    async fn hub_broadcast_fanout() {
        let hub = ShareHub::new();
        let mut a = hub.subscribe("a").unwrap();
        let mut b = hub.subscribe("b").unwrap();
        let n = hub.broadcast(serde_json::json!({"ev":"ping"}));
        assert_eq!(n, 2);
        assert!(a.recv().await.is_ok());
        assert!(b.recv().await.is_ok());
    }

    #[tokio::test]
    async fn hub_queue_factory() {
        let hub = ShareHub::new();
        let (prod, mut cons) = hub.create_queue("jobs");
        prod.send(ShareMessage::text("jobs", "task")).await.unwrap();
        let m = cons.recv().await.unwrap();
        assert_eq!(m.payload.as_str().unwrap(), "task");
    }

    #[tokio::test]
    async fn hub_concurrent_publishers() {
        let hub = ShareHub::new();
        let mut sub = hub.subscribe("chat").unwrap();
        let h2 = hub.clone();
        let jh = tokio::spawn(async move {
            for i in 0..20 {
                h2.publish_text("chat", format!("msg-{i}")).unwrap();
            }
        });
        jh.await.unwrap();
        let mut got = 0;
        while let Some(Ok(_)) = sub.try_recv() {
            got += 1;
        }
        // Also drain via async recv for any remaining (try_recv empties current buffer)
        assert!(got >= 10, "at least half should be observable, got {got}");
    }
}
