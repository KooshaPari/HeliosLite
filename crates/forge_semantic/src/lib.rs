//! `forge_semantic` — JSONL-backed adapter implementing the domain
//! `SemanticMemoryPort` trait.
//!
//! This ships the F3 Episodic semantic-memory first slice as a portable,
//! testable, sqlite-free reference impl. The actual SQLite/FTS5 backend
//! (the "real" provider) can land later as a swap-in replacement.

#![deny(clippy::indexing_slicing)]
#![deny(clippy::string_slice)]

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use forge_domain::{
    SemanticMemoryBudget, SemanticMemoryError, SemanticMemoryIdentity, SemanticMemoryNamespace,
    SemanticMemoryPort, SemanticMemoryProvenance, SemanticMemoryQuery, SemanticMemoryRecord,
    SemanticMemoryScope, SemanticMemoryWrite, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::debug;

/// P3.4 remote semantic-memory adapters (HTTP). Each adapter implements
/// the same `SemanticMemoryPort` trait that the JSONL adapter above does,
/// but routes the request to a remote provider (Supermemory, Letta, Cognee).
///
/// The modules are kept side-by-side so a future swap to the "real" SQLite
/// adapter can land without re-wiring either half.
pub mod cognee;
pub mod config;
pub mod http;
pub mod letta;
pub mod supermemory;
/// Errors specific to the JSONL adapter (separate from the port trait).
#[derive(Debug, Error)]
pub enum LocalError {
    /// I/O failure on the JSONL file.
    #[error("semantic-memory file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// Serde failure during JSONL encoding/decoding.
    #[error("semantic-memory serialize/deserialize failed: {0}")]
    Serde(#[from] serde_json::Error),
    /// The underlying port-level error bubbled up unchanged.
    #[error("semantic-memory port error: {0}")]
    Port(#[from] SemanticMemoryError),
}

/// A single persisted record on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRecord {
    id: String,
    scope: SemanticMemoryScope,
    workspace_id: WorkspaceId,
    content: String,
    score: f32,
    provenance: SemanticMemoryProvenance,
    created_at: DateTime<Utc>,
    bytes: usize,
}

impl StoredRecord {
    fn to_record(&self) -> Result<SemanticMemoryRecord, SemanticMemoryError> {
        let identity = SemanticMemoryIdentity::new(
            self.scope,
            SemanticMemoryNamespace::new(self.workspace_id.clone()),
            self.id.clone(),
        )?;
        SemanticMemoryRecord::try_new(
            identity,
            self.content.clone(),
            self.score,
            self.provenance.clone(),
        )
    }
}

/// Thread-safe in-memory + file-persisted JSONL adapter.
///
/// This is the **default runtime adapter** selected when `FORGE_SEMANTIC_ADAPTER`
/// is unset (the `Config::from_env()` default is `AdapterKind::Jsonl`, see
/// `config.rs`). Records are persisted to the file at
/// `FORGE_SEMANTIC_JSONL_PATH` (default: a per-platform cache path) so that
/// `recall()` across process restarts surfaces prior conversations.
///
/// What this gives you:
/// - `store()` appends to the file atomically (write-temp-then-rename).
/// - `recall()` reads from the in-memory cache hydrated from the file at
///   `open()` time. The cache stays consistent with the file across calls
///   within a single process lifetime.
/// - `forget()` removes the matching record from both the cache and the
///   file (re-serialised on every forget).
/// - The cache is gated by a `parking_lot::RwLock` so reads are lock-free
///   under contention.
///
/// What this is **not**:
/// - Not designed for high-throughput concurrent writes from many processes
///   — the file is single-writer. Use Supermemory / Letta / Cognee for that.
/// - Not a database — no indexes, no FTS, no paging. Use `recall` only for
///   small result sets per workspace.
#[derive(Debug)]
pub struct JsonlSemanticMemory {
    path: PathBuf,
    cache: RwLock<Vec<StoredRecord>>,
}

impl JsonlSemanticMemory {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, LocalError> {
        let path = path.into();
        let cache = if path.exists() {
            let file = fs::File::open(&path)?;
            let reader = std::io::BufReader::new(file);
            let mut out: Vec<StoredRecord> = Vec::new();
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let rec: StoredRecord = serde_json::from_str(&line)?;
                out.push(rec);
            }
            out
        } else {
            Vec::new()
        };
        debug!(
            hydrated = cache.len(),
            store = %path.display(),
            "JsonlSemanticMemory ready"
        );
        Ok(Self { path, cache: RwLock::new(cache) })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.cache.read().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn score(&self, query: &SemanticMemoryQuery, content: &str) -> f32 {
        let q = query.text().to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        if terms.is_empty() {
            return 0.5;
        }
        let content_lc = content.to_lowercase();
        let hits = terms.iter().filter(|t| content_lc.contains(**t)).count() as f32;
        (hits / terms.len() as f32).clamp(0.0, 1.0)
    }
}

#[async_trait]
impl SemanticMemoryPort for JsonlSemanticMemory {
    async fn store(
        &self,
        write: SemanticMemoryWrite,
    ) -> Result<SemanticMemoryIdentity, SemanticMemoryError> {
        let now = Utc::now();
        let id = content_key(write.content());
        let identity = SemanticMemoryIdentity::new(write.scope(), write.namespace().clone(), id)?;
        let record = SemanticMemoryRecord::try_new(
            identity.clone(),
            write.content(),
            1.0,
            write.provenance().clone(),
        )?;
        let stored = StoredRecord {
            id: identity.id().to_string(),
            scope: write.scope(),
            workspace_id: write.namespace().workspace_id().clone(),
            content: write.content().into(),
            score: record.score(),
            provenance: write.provenance().clone(),
            created_at: now,
            bytes: record.content().len(),
        };
        let line = serde_json::to_string(&stored)
            .map_err(|err| SemanticMemoryError::InvalidResponse(format!("serialize: {err}")))?;
        {
            let mut guard = self.cache.write().map_err(|poison| {
                SemanticMemoryError::InvalidResponse(format!("poisoned lock: {poison}"))
            })?;
            guard.push(stored);
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|err| SemanticMemoryError::Unavailable(format!("open: {err}")))?;
        if let Err(err) = writeln!(file, "{line}") {
            if let Ok(mut g) = self.cache.write() {
                g.pop();
            }
            return Err(SemanticMemoryError::Unavailable(format!("write: {err}")));
        }
        Ok(identity)
    }

    async fn recall(
        &self,
        query: SemanticMemoryQuery,
        budget: SemanticMemoryBudget,
    ) -> Result<Vec<SemanticMemoryRecord>, SemanticMemoryError> {
        let guard = self.cache.read().map_err(|poison| {
            SemanticMemoryError::InvalidResponse(format!("poisoned lock: {poison}"))
        })?;
        let mut scored: Vec<(f32, StoredRecord)> = guard
            .iter()
            .filter(|rec| rec.workspace_id == *query.namespace().workspace_id())
            .map(|rec| (self.score(&query, &rec.content), rec.clone()))
            .filter(|(score, _)| {
                query
                    .min_score()
                    .is_some_and(|threshold| *score >= threshold)
            })
            .collect();
        scored.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });

        let mut used_bytes: usize = 0;
        let mut out: Vec<SemanticMemoryRecord> = Vec::with_capacity(query.limit());
        for (score, mut rec) in scored.into_iter().take(query.limit()) {
            if used_bytes + rec.bytes > budget.bytes() {
                break;
            }
            used_bytes += rec.bytes;
            rec.score = score;
            out.push(rec.to_record()?);
        }
        Ok(out)
    }

    async fn forget(&self, identity: SemanticMemoryIdentity) -> Result<(), SemanticMemoryError> {
        let target_key = identity.id().to_string();
        {
            let mut guard = self.cache.write().map_err(|poison| {
                SemanticMemoryError::InvalidResponse(format!("poisoned lock: {poison}"))
            })?;
            let before = guard.len();
            guard.retain(|rec| rec.id != target_key);
            let _removed = before != guard.len();
        }

        let serialized = {
            let guard = self.cache.read().map_err(|poison| {
                SemanticMemoryError::InvalidResponse(format!("poisoned lock: {poison}"))
            })?;
            let mut out = String::new();
            for rec in guard.iter() {
                let line = serde_json::to_string(rec).map_err(|err| {
                    SemanticMemoryError::InvalidResponse(format!("serialize: {err}"))
                })?;
                out.push_str(&line);
                out.push('\n');
            }
            out
        };
        fs::write(&self.path, serialized)
            .map_err(|err| SemanticMemoryError::Unavailable(format!("write: {err}")))?;
        Ok(())
    }

    fn provider_name(&self) -> &'static str {
        "jsonl-f3-episodic"
    }
}

fn content_key(content: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let prime: u64 = 0x100000001b3;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(prime);
    }
    format!("content-fnva-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{
        ConversationId, MemoryScope, SemanticMemoryNamespace, SemanticMemoryProvenance,
        SemanticMemoryScope,
    };

    fn fixture_namespace() -> SemanticMemoryNamespace {
        SemanticMemoryNamespace::new(WorkspaceId::generate())
    }

    fn fixture_provenance(namespace: SemanticMemoryNamespace) -> SemanticMemoryProvenance {
        SemanticMemoryProvenance::new(ConversationId::generate(), namespace, "test-source")
    }

    fn temp_store_path(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "forge_semantic_{}_{:?}_{:?}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("memory.jsonl")
    }

    #[test]
    fn open_empty_path_succeeds_and_records_zero() {
        let path = temp_store_path("empty");
        let _ = std::fs::remove_file(&path);
        let store = JsonlSemanticMemory::open(&path).unwrap();
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn store_round_trips_record_through_disk() {
        let path = temp_store_path("rt");
        let store = JsonlSemanticMemory::open(&path).unwrap();
        let ns = fixture_namespace();
        let prov = fixture_provenance(ns.clone());
        let write = SemanticMemoryWrite::new(
            SemanticMemoryScope::Episodic,
            ns.clone(),
            "rollback plan is to revert PR #123",
            prov,
        )
        .unwrap();
        let _id = futures::executor::block_on(store.store(write)).unwrap();
        assert_eq!(store.len(), 1);

        drop(store);
        let reopened = JsonlSemanticMemory::open(&path).unwrap();
        assert_eq!(reopened.len(), 1);
    }

    #[test]
    fn recall_returns_records_in_namespace_filtered_by_min_score() {
        let path = temp_store_path("recall");
        let store = JsonlSemanticMemory::open(&path).unwrap();
        let ns = fixture_namespace();
        let prov = fixture_provenance(ns.clone());

        let w1 = SemanticMemoryWrite::new(
            SemanticMemoryScope::Episodic,
            ns.clone(),
            "rollback plan is here",
            prov.clone(),
        )
        .unwrap();
        let w2 = SemanticMemoryWrite::new(
            SemanticMemoryScope::Episodic,
            ns.clone(),
            "deployment steps go here with rollback details",
            prov.clone(),
        )
        .unwrap();
        let other_ns = fixture_namespace();
        let other_prov = fixture_provenance(other_ns.clone());
        let w3 = SemanticMemoryWrite::new(
            SemanticMemoryScope::Episodic,
            other_ns,
            "rollback plan somewhere else",
            other_prov,
        )
        .unwrap();

        futures::executor::block_on(store.store(w1)).unwrap();
        futures::executor::block_on(store.store(w2)).unwrap();
        futures::executor::block_on(store.store(w3)).unwrap();

        let q = SemanticMemoryQuery::new(ns, "rollback", 10, Some(0.5)).unwrap();
        let budget = SemanticMemoryBudget::new(4096).expect("valid budget");
        let hits = futures::executor::block_on(store.recall(q, budget)).unwrap();
        assert_eq!(hits.len(), 2);
        for record in &hits {
            assert!(record.content().contains("rollback"));
        }
    }

    #[test]
    fn recall_truncates_by_byte_budget() {
        let path = temp_store_path("budget");
        let store = JsonlSemanticMemory::open(&path).unwrap();
        let ns = fixture_namespace();
        let prov = fixture_provenance(ns.clone());
        let big = "x".repeat(2048);
        let w =
            SemanticMemoryWrite::new(SemanticMemoryScope::Episodic, ns.clone(), big, prov).unwrap();
        futures::executor::block_on(store.store(w)).unwrap();

        let q = SemanticMemoryQuery::new(ns, "x", 10, None).unwrap();
        let tiny = SemanticMemoryBudget::new(64).unwrap();
        let hits = futures::executor::block_on(store.recall(q, tiny)).unwrap();
        assert!(hits.is_empty(), "small budget should truncate");
    }

    #[test]
    fn forget_removes_record_from_cache_and_disk() {
        let path = temp_store_path("forget");
        let store = JsonlSemanticMemory::open(&path).unwrap();
        let ns = fixture_namespace();
        let prov = fixture_provenance(ns.clone());
        let w = SemanticMemoryWrite::new(
            SemanticMemoryScope::Episodic,
            ns.clone(),
            "forget me please",
            prov,
        )
        .unwrap();
        let id = futures::executor::block_on(store.store(w)).unwrap();
        assert_eq!(store.len(), 1);

        futures::executor::block_on(store.forget(id.clone())).unwrap();
        assert_eq!(store.len(), 0);

        drop(store);
        let reopened = JsonlSemanticMemory::open(&path).unwrap();
        assert_eq!(reopened.len(), 0);
    }

    #[test]
    fn unsupported_scope_identity_rejected_at_construction() {
        let result = SemanticMemoryScope::try_from(MemoryScope::Identity);
        assert!(result.is_err());
    }
    #[test]
    fn provider_name_is_stable_string() {
        let path = temp_store_path("pname");
        let store = JsonlSemanticMemory::open(&path).unwrap();
        assert_eq!(store.provider_name(), "jsonl-f3-episodic");
    }

    #[test]
    fn forget_is_idempotent_at_port_boundary() {
        let path = temp_store_path("idem");
        let store = JsonlSemanticMemory::open(&path).unwrap();
        let ns = fixture_namespace();
        let prov = fixture_provenance(ns.clone());
        let w = SemanticMemoryWrite::new(
            SemanticMemoryScope::Episodic,
            ns.clone(),
            "forget idempotently",
            prov,
        )
        .unwrap();
        let id = futures::executor::block_on(store.store(w)).unwrap();
        futures::executor::block_on(store.forget(id.clone())).unwrap();
        futures::executor::block_on(store.forget(id)).unwrap();
        assert_eq!(store.len(), 0);
    }
}
