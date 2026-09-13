//! Persistent, file-backed batch queue for offline retry.
//!
//! [`DiskStore`] is a drop-in replacement for [`crate::MemoryStore`] that
//! survives process restarts. Events are journaled to an append-only NDJSON
//! file so that, if the sink never returns a successful flush, the buffered
//! events are replayed on the next startup.
//!
//! ## Durability semantics
//!
//! The journal is an **at-least-once** log:
//!
//! * `submit` appends the event to the journal and to the in-memory buffer.
//! * `drain_batch` pops from the in-memory buffer and advances a logical
//!   consumed counter; it does **not** rewrite the file on every pop.
//! * [`close`](DiskStore::close) (also run from `Drop`) truncates the file
//!   and rewrites only the events still buffered, so after a clean shutdown
//!   the disk state exactly mirrors the in-memory state.
//! * An unclean crash between `submit` and `close` may replay events that
//!   were already drained. That is the correct retry behaviour for
//!   telemetry (at-least-once delivery, never lost).
//! * [`compact`](DiskStore::compact) may be called explicitly to rewrite the
//!   journal mid-life after a large drain, bounding file growth.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

use crate::error::{SinkError, SinkResult};
use crate::event::TraceraEvent;
use crate::store::StoreFull;

const HEADER: &str = "# forge_tracera::DiskStore v1";

/// Data held per journal file.
struct DiskState {
    path: PathBuf,
    /// Bytes of the journal header line (rewritten on compact).
    queue: VecDeque<TraceraEvent>,
    capacity: usize,
    total_submitted: u64,
    total_dropped: u64,
}

/// A persistent FIFO of [`TraceraEvent`]s backed by an NDJSON journal file.
///
/// Thread-safe: all operations take `&self` and are serialised behind an
/// internal `parking_lot::Mutex`.
#[derive(Clone)]
pub struct DiskStore {
    state: Arc<Mutex<DiskState>>,
}

use std::sync::Arc;

/// Error returned when [`DiskStore`] fails to read or write its journal.
///
/// Wrapped into [`SinkError`] at the sink boundary.
#[derive(Debug, thiserror::Error)]
pub enum DiskError {
    /// The journal file could not be opened / created.
    #[error("cannot open journal {path}: {source}")]
    Open {
        /// Journal path.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A malformed journal line could not be deserialized.
    #[error("corrupt journal line {line} in {path}: {source}")]
    Corrupt {
        /// Journal path.
        path: String,
        /// 1-based journal line that failed to parse.
        line: usize,
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// A write to the journal failed.
    #[error("journal write failed in {path}: {source}")]
    Flush {
        /// Journal path.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

impl From<DiskError> for SinkError {
    fn from(value: DiskError) -> Self {
        SinkError::Transport(value.to_string())
    }
}

impl DiskStore {
    /// Open (creating if necessary) a journal at `path` with the given
    /// `capacity`, replaying any events buffered on disk.
    ///
    /// # Capacity
    ///
    /// `capacity` bounds the in-memory buffer *and* the events replayed from
    /// disk. If the journal contains more events than fit in `capacity`, the
    /// oldest are dropped and counted via [`total_dropped`](
    /// DiskStore::total_dropped).
    pub fn open(path: impl AsRef<Path>, capacity: usize) -> SinkResult<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)
            .map_err(|source| DiskError::Open { path: path.display().to_string(), source })?;

        let mut queue = VecDeque::with_capacity(capacity.min(4096));
        let mut total_dropped = 0u64;
        let mut total_submitted = 0u64;

        // Replay any existing journal content.
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        if size > 0 {
            let reader = BufReader::new(&mut file);
            use std::io::BufRead;
            let mut header_lines = 0usize;
            for (idx, line) in reader.lines().enumerate() {
                let line = line.map_err(|source| DiskError::Open {
                    path: path.display().to_string(),
                    source,
                })?;
                if line.starts_with('#') {
                    header_lines += 1;
                    continue;
                }
                if line.is_empty() {
                    continue;
                }
                let line_reported = (idx + 1) - header_lines;
                let ev: TraceraEvent =
                    serde_json::from_str(&line).map_err(|source| DiskError::Corrupt {
                        path: path.display().to_string(),
                        line: line_reported,
                        source,
                    })?;
                total_submitted += 1;
                if queue.len() < capacity {
                    queue.push_back(ev);
                } else {
                    total_dropped += 1;
                }
            }
        }

        Ok(Self {
            state: Arc::new(Mutex::new(DiskState {
                path,
                queue,
                capacity,
                total_submitted,
                total_dropped,
            })),
        })
    }

    /// Push an event. Returns [`StoreFull`] if at capacity and
    /// `block_on_full` is false; otherwise blocks (spins) until room is
    /// available.
    pub fn submit(&self, ev: TraceraEvent, block_on_full: bool) -> Result<(), StoreFull> {
        loop {
            let mut s = self.state.lock();
            if s.queue.len() >= s.capacity {
                if !block_on_full {
                    s.total_submitted += 1;
                    s.total_dropped += 1;
                    return Err(StoreFull { capacity: s.capacity });
                }
                drop(s);
                std::thread::yield_now();
                continue;
            }
            // Append to the journal first; if the write fails, return the
            // error (the event is not buffered).
            let line = serde_json::to_string(&ev).unwrap_or_else(|_| {
                // `TraceraEvent` only contains serializable fields; treat an
                // impossible failure as a dropped event to avoid panic.
                s.total_submitted += 1;
                s.total_dropped += 1;
                String::new()
            });
            if line.is_empty() {
                return Err(StoreFull { capacity: s.capacity });
            }
            if let Err(e) = s.append_line(&line) {
                tracing::warn!(error = %e, "tracera disk journal append failed");
                // Roll the counters forward as dropped so we don't pretend
                // the event is durably accepted.
                s.total_submitted += 1;
                s.total_dropped += 1;
                return Err(StoreFull { capacity: s.capacity });
            }
            s.total_submitted += 1;
            s.queue.push_back(ev);
            return Ok(());
        }
    }

    /// Drain up to `max` events (oldest first) from the in-memory buffer.
    pub fn drain_batch(&self, max: usize) -> Vec<TraceraEvent> {
        let mut s = self.state.lock();
        let take = max.min(s.queue.len());
        s.queue.drain(..take).collect()
    }

    /// Peek up to `max` events without removing them.
    pub fn peek_batch(&self, max: usize) -> Vec<TraceraEvent> {
        let s = self.state.lock();
        s.queue.iter().take(max).cloned().collect()
    }

    /// Re-insert a previously-drained batch at the front (transport retry).
    ///
    /// Matches [`crate::MemoryStore::requeue_front`]: the batch is kept in
    /// order at the front of the queue; if the queue is near capacity, the
    /// tail of the batch is dropped and counted.
    pub fn requeue_front(&self, mut events: Vec<TraceraEvent>) -> Result<(), StoreFull> {
        let mut s = self.state.lock();
        let new_total = s.queue.len() + events.len();
        if new_total > s.capacity {
            let keep = s.capacity.saturating_sub(s.queue.len());
            let drop = events.len().saturating_sub(keep);
            s.total_dropped += drop as u64;
            events.truncate(keep);
        }
        for ev in events.into_iter().rev() {
            s.queue.push_front(ev);
        }
        Ok(())
    }

    /// Current buffered depth.
    pub fn len(&self) -> usize {
        self.state.lock().queue.len()
    }

    /// True if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.state.lock().queue.is_empty()
    }

    /// Configured capacity.
    pub fn capacity(&self) -> usize {
        self.state.lock().capacity
    }

    /// Total events ever submitted (successfully or rejected).
    pub fn total_submitted(&self) -> u64 {
        self.state.lock().total_submitted
    }

    /// Total events dropped because the buffer / journal was full.
    pub fn total_dropped(&self) -> u64 {
        self.state.lock().total_dropped
    }

    /// Rewrite the journal to contain exactly the currently-buffered events.
    ///
    /// Call after a large drain to bound file growth, or rely on
    /// [`close`](DiskStore::close) which runs automatically on drop.
    pub fn compact(&self) -> SinkResult<()> {
        let s = self.state.lock();
        s.rewrite()
    }

    /// Flush and compact the journal, then mark it closed. Idempotent.
    pub fn close(&self) -> SinkResult<()> {
        let s = self.state.lock();
        s.rewrite()
    }
}

impl Drop for DiskStore {
    fn drop(&mut self) {
        // Best-effort: compact the journal so disk reflects current buffer.
        if let Some(inner) = Arc::get_mut(&mut self.state) {
            let _ = inner.lock().rewrite();
        }
        // When other clones exist we can't mutably reach the Arc, so the
        // journal may retain drained events until a later close()/compact().
    }
}

impl DiskState {
    /// Append one serialized event line to the journal. Note the event is
    /// already serialized by the caller.
    fn append_line(&mut self, line: &str) -> SinkResult<()> {
        let file = OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|source| DiskError::Open { path: self.path.display().to_string(), source })?;
        let mut out = std::io::BufWriter::new(file);
        out.write_all(line.as_bytes())
            .map_err(|source| DiskError::Flush { path: self.path.display().to_string(), source })?;
        out.write_all(b"\n")
            .map_err(|source| DiskError::Flush { path: self.path.display().to_string(), source })?;
        out.flush()
            .map_err(|source| DiskError::Flush { path: self.path.display().to_string(), source })?;
        Ok(())
    }

    /// Truncate the journal and rewrite it with the current buffer (compaction).
    fn rewrite(&self) -> SinkResult<()> {
        let tmp = self.path.with_extension("tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|source| DiskError::Open { path: tmp.display().to_string(), source })?;
        let mut out = std::io::BufWriter::new(&mut file);
        writeln!(out, "{HEADER}")
            .map_err(|source| DiskError::Flush { path: self.path.display().to_string(), source })?;
        for ev in &self.queue {
            let line = serde_json::to_string(ev).map_err(|e| SinkError::Serde(e.to_string()))?;
            writeln!(out, "{line}").map_err(|source| DiskError::Flush {
                path: self.path.display().to_string(),
                source,
            })?;
        }
        out.flush()
            .map_err(|source| DiskError::Flush { path: self.path.display().to_string(), source })?;
        drop(out);
        drop(file);
        std::fs::rename(&tmp, &self.path)
            .map_err(|source| DiskError::Flush { path: self.path.display().to_string(), source })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(topic_tag: &str, source: &str, payload: &str) -> TraceraEvent {
        // TraceraEvent is `pub` and Deserialize; construct via json for
        // robustness against optional-field churn.
        let v = json!({
            "schema": "tracera.v1",
            "id": format!("01J0TRA56789ABCDEFGHJ00{}-{}", topic_tag, source),
            "ts": "2026-09-12T00:00:00Z",
            "source": source,
            "kind": "tool_call",
            "session_id": "sess-1",
            "payload": { "m": payload }
        });
        serde_json::from_value(v).expect("event decodes")
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "forge_tracera_diskstore_{}_{name}.log",
            std::process::id()
        ));
        p
    }

    #[test]
    fn submit_and_drain_round_trip() {
        let p = temp_path("rt");
        let _ = std::fs::remove_file(&p);
        let store = DiskStore::open(&p, 16).unwrap();
        store.submit(ev("a", "tool_call", "run"), false).unwrap();
        store.submit(ev("b", "tool_call", "run"), false).unwrap();
        assert_eq!(store.len(), 2);
        let batch = store.drain_batch(10);
        assert_eq!(batch.len(), 2);
        assert!(store.is_empty());
        assert_eq!(store.total_submitted(), 2);
        store.close().unwrap();
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn replay_survives_reopen() {
        let p = temp_path("reopen");
        let _ = std::fs::remove_file(&p);
        {
            let store = DiskStore::open(&p, 8).unwrap();
            store.submit(ev("a", "tool_call", "run"), false).unwrap();
            store.submit(ev("b", "task", "def"), false).unwrap();
            // No close: simulate process termination mid-way.
        }
        let store = DiskStore::open(&p, 8).unwrap();
        assert_eq!(store.len(), 2, "both events replayed from disk");
        assert_eq!(store.total_submitted(), 2);
        store.close().unwrap();
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn close_compacts_drained_events() {
        let p = temp_path("compact");
        let _ = std::fs::remove_file(&p);
        let store = DiskStore::open(&p, 8).unwrap();
        store.submit(ev("a", "tool_call", "run"), false).unwrap();
        store.submit(ev("b", "tool_call", "run"), false).unwrap();
        let drained = store.drain_batch(1);
        assert_eq!(drained.len(), 1);
        // The oldest event ("a") was drained; "b" remains buffered.
        let remaining = ev("b", "tool_call", "run");
        // After a clean close, only the undrained event is persisted.
        store.close().unwrap();
        let reloaded = DiskStore::open(&p, 8).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.peek_batch(10)[0].id, remaining.id);
        reloaded.close().unwrap();
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn requeue_front_wins_next_drain() {
        let p = temp_path("requeue");
        let _ = std::fs::remove_file(&p);
        let store = DiskStore::open(&p, 8).unwrap();
        store
            .submit(ev("first", "tool_call", "run"), false)
            .unwrap();
        store
            .submit(ev("second", "tool_call", "run"), false)
            .unwrap();
        let batch = store.drain_batch(4);
        // Simulate a transient transport failure: push both back.
        store.requeue_front(batch).unwrap();
        assert_eq!(store.len(), 2);
        let again = store.drain_batch(4);
        assert_eq!(again[0].id, ev("first", "tool_call", "run").id);
        store.close().unwrap();
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn capacity_rejects_with_block_on_full_false() {
        let p = temp_path("cap");
        let _ = std::fs::remove_file(&p);
        let store = DiskStore::open(&p, 2).unwrap();
        store.submit(ev("a", "tool_call", "run"), false).unwrap();
        store.submit(ev("b", "tool_call", "run"), false).unwrap();
        let err = store.submit(ev("c", "tool_call", "run"), false);
        assert!(err.is_err());
        assert_eq!(store.total_dropped(), 1);
        store.close().unwrap();
        let _ = std::fs::remove_file(&p);
    }
}
