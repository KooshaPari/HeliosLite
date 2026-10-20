# forge_dbd architecture/existence finding

Date: 2026-09-30. Status: EXPERIMENTAL OPT-IN / EXISTENCE GATE OPEN.

## Corrected mountedness
Earlier comments inside `forge_dbd::client` saying nothing outside the crate uses it are stale. Frozen source mounts `DaemonConversationRepository` from `forge_repo::ForgeRepo` when `Environment::dbd_enabled()` is true. `FORGE_DBD_ENABLED` defaults to false, so direct SQLite remains the normal path.

## Useful primitive worth preserving
The daemon client/repository explicitly distinguishes `Unavailable` (fresh connection failed; no request bytes sent, direct fallback may safely replay) from `Indeterminate` (connection established/request exchange began; daemon may have committed, so direct fallback is forbidden). This is strong internal prior art for the broader ExternalEffectReceipt uncertainty model.

Do not generalize database-write certainty into arbitrary external effects: DB daemon protocol controls its own request boundary; third-party tools may not expose comparable receipts/idempotency.

## Existing direct-path control
The normal DatabasePool already sets `busy_timeout=30000`, `journal_mode=WAL`, `synchronous=NORMAL`, disables per-connection autocheckpoint and runs a dedicated WAL checkpointer. It also uses a split write DB + attached legacy read projection. Therefore the daemon must beat an already concurrency-aware baseline, not naive SQLite defaults.

## Existence test
Before any mature/default promotion, compare daemon OFF versus ON under matched multi-process writer workloads. Bind database contents and process configuration. Measure lock/busy failures, p50/p95/p99 write latency, throughput, process/RSS overhead, WAL/checkpoint behavior, restart/socket recovery, lost/duplicate mutation outcomes and operational/install burden. Inject failure before send, after request bytes, after DB commit/before ack and during daemon restart.

SQLite WAL itself serializes writers while allowing readers and writers concurrently. Application-level single-writer serialization may help under real write-lock contention, but this must be demonstrated for Helios workloads rather than assumed.

## Current decision
KEEP EXPERIMENT + CERTAINTY PRIMITIVE; DO NOT COUNT AS DEFAULT PRODUCT CAPABILITY; DO NOT EXPAND until matched experiment demonstrates value. Issue #326 tracks the gate.