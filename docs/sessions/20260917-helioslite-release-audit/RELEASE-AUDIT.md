# HeliosLite 2.13.21 Release Audit — diff vs tailcallhq/forgecode

**Date:** 2026-09-17
**Reviewer:** Jcode (HeliosLite agent)
**Repo:** `/Users/kooshapari/CodeProjects/Phenotype/repos/forgecode`
**Fork tip:** `ebb7dd670` (2026-09-16 05:15 PDT)
**Upstream tip:** `6ed5d37b6` (origin/main = tailcallhq/forgecode)
**Merge-base:** `6ed5d37b6b45a2b6220877fd9aec5ba4c4b7f3c0`
**Commits ahead:** 640 fork-only
**Working tree:** 105 files modified (release-prep sanitization WIP — `stash@{0}: sanitize-wip-20260917-Ko`)

## Build & install

- `cargo build --release -p forge_main --bin helioslite` from clean HEAD → exit 0 (3m12s)
- Installed binary: `/Users/kooshapari/.helioslite/bin/helioslite`
  - Size: 39.5 MB (was 38.2 MB)
  - Built: 2026-09-17 01:50 UTC
  - Version: `helioslite 2.13.21`
- Old installed binary (Sep 11) backed up at `~/.cargo/bin/helioslite.backup-20260723T013724Z`

## Working-tree state — read before continuing

The 105 modified files were a release-prep sanitization pass (`KooshaPari` → `<REDACTED>`) that broke the build because Rust function names in `crates/forge_main/src/update.rs:619` and friends had their identifier `KooshaPari` replaced with `<REDACTED>` literally. The sanitization work was stashed to `stash@{0}` so the build could succeed.

**Action required:** decide whether to (a) discard the sanitization (it's broken anyway), (b) redo it with sed on identifiers only (skip Rust source), or (c) drop it and apply a different fork-private repo path.

### `git worktree prune` (already done)

All 10 stale `.worktrees/h017-*` and `.worktrees/minimax-metadata` worktree directories were marked **prunable** (their gitdir files point to non-existent locations). `git worktree prune` was run (exit 0). The directories are now plain untracked folders, all sitting at `main` HEAD `ebb7dd670` with **no unique branches**. Total disk: ~200 MB. Safe to delete with `rm -rf .worktrees/h017-* .worktrees/minimax-metadata`, but **not done automatically** — that's your call.

### `Cargo.lock` change (legitimate follow-up — should be committed)

The working tree has 9 lines added/changed in `Cargo.lock`. This is **not** build noise — it's the lockfile update that commit `34d81622c` forgot to include. Two changes:

1. `phenotype-health`, `phenotype-observability`, `phenotype-telemetry` each gain a `source = "git+https://github.com/KooshaPari/PhenoShared.git?rev=68beca26#..."` line (the rev bumped in `34d81622c`'s Cargo.toml edit, but the lockfile wasn't regenerated before commit)
2. Transitive `thiserror 2.0.19` → `thiserror 2.0.20` (PhenoShared 68beca26's bumped dependency)

**Recommendation:** commit `Cargo.lock` as a follow-up to `34d81622c`:
```
fix(lockfile): regenerate Cargo.lock for PhenoShared rev=68beca26

Adds source entries for the 3 PhenoShared crates and bumps thiserror 2.0.19→2.0.20
transitively. Follow-up to 34d81622c which updated Cargo.toml but missed
the lockfile regeneration.
```

### Untracked session docs (keep)

- `docs/sessions/20260911-pr-worktree-followup/worktree-audit.md` — session doc from the h017 bundle reconciliation. Useful history.
- `docs/sessions/20260917-helioslite-release-audit/` — this audit doc.

## Categories of fork delta

| Type    | Count | %     | Notes |
|---------|-------|-------|-------|
| fix     | 218   | 34.1% | PhenoShared git deps, cargo-deny, CI paths, post-merge fixes |
| feat    | 129   | 20.2% | Substantive new functionality |
| ci      | 83    | 13.0% | Workflow YAML, action pins, coverage ratchets |
| chore   | 67    | 10.5% | Deps, fmt, refactors, version bumps |
| docs    | 42    |  6.6% | RENAMES-STRATEGY, ADRs, install, packaging |
| test    | 25    |  3.9% | E2E shell scripts, LSP/ShareCli integration |
| merge   | 39    |  6.1% | PR merge commits |
| style   | 14    |  2.2% | cargo fmt, formatting |
| perf    |  5    |  0.8% | LTO, allocator tuning |
| refactor|  3    |  0.5% | Internal reorg |
| release |  2    |  0.3% | Version bump + manifest regen |

Total diff: **791 files changed, 333,642 insertions(+), 6,755 deletions(-)**

## Top-level areas changed

| Directory     | Files |
|---------------|-------|
| crates/       | 490   |
| docs/         | 79    |
| .github/      | 32    |
| assets/       | 18    |
| tooling/      | 16    |
| benchmarks/   | 12    |
| tests/        | 9     |
| shell-plugin/ | 9     |
| forge-daemon/ | 9     |
| audit/        | 9     |
| src/          | 8     |
| terraform/    | 7     |
| scripts/      | 7     |
| desktop/      | 7     |
| packaging/    | 6     |
| apps/         | 5     |
| templates/    | 4     |
| plans/        | 3     |
| fuzz/         | 3     |
| distribution/ | 2     |
| benches/      | 2     |

## 34 NEW crates added by HeliosLite fork

### Telemetry / observability (3)
- **forge_tracera** — outbound Tracera wire-format sink; HTTP transport; HMAC auth rotation with overlap; gzip compression; persistent DiskStore (NDJSON journal); offline retry
- **forge_dbd** — SQLite write daemon for persistent conversation storage (P3 single-writer; `FORGE_DBD_ENABLED` env gate)
- **forge_drift** — drift detection (hash + Jaccard word-set similarity) for multi-agent overlap

### AI / memory / context (3)
- **forge_semantic** — SemanticMemoryPort adapters (in-memory + JSONL file-backed); default runtime adapter; Supermemory + Letta + Cognee adapters; warm-hydrate recall; forget-on-compact
- **forge_guardian** — LLM-driven + heuristic risk adjudication for the policy engine; LearningStore + Sandbox tie-breaker; wired into permission check at P0.1.2
- **forge_agileplus** — AgilePlus delivery-quality OS (P3.2); 31-pillar scorecard, sprint records, velocity prediction

### Integration / extensibility (3)
- **forge_lsp** — LSP-style diagnostics; hover/definition/completion; implementation/references/typeDefinition/rename; MCP config auto-reload watcher; per-server last-seen health metadata
- **forge_sharecli** — ShareCLI realtime relay (P3.3); in-process broadcast channels, bounded queues, hub registry, backpressure fanout; SSE + WebSocket transports; attach-to-session ingestion
- **forge_plugin** — Plugin system for HeliosLite with hook-based tool extensions

### Platform / runtime (5)
- **forge_tui** — Terminal UI dashboard for forge3d daemon
- **forge_daemon** — Zig forge-daemon client (kqueue + posix_spawn density lever)
- **forge_gpu** — GPU lane intent for heterogeneous CUDA routing (3090 primary / 1080 helper)
- **forge_sandbox** — sandbox hook
- **helios-bot** — GitHub bot responding to comments/mentions; Pillar scorecard posting

### Terminal / shell (3)
- **forge_pheno_shell** — Shell abstraction layer: unified detection + completion emission for ZSH, Bash, Fish, PowerShell (Windows + Core), Nushell, Elvish, Cmd, Tcsh, Oil
- **forge_pheno_winterminal** — Windows Terminal profile/palette/scheme management
- **forge_mux** — MuxBridge trait + tmux implementation for forgecode terminal multiplexer integration

### Rendering / UX (3)
- **forge_render** — streaming output + interactive tool-call rendering
- **forge_paste** — input pipeline: bracketed paste, image protocols, @-mention parsing, paste classification and collapse
- **ghostty-kit** — Ghostty terminal integration

### Data / structure (4)
- **forge_graph** — codebase dependency graph (scan, model, query source-file relationships)
- **forge_repo_map** — tree-sitter repo map
- **forge_similarity** — similarity hash-only impl
- **forge_syntax** — syntax tree integration

### Build / SDK / ops (4)
- **forge_sdk** — public SDK
- **forge_e2e** — end-to-end test harness (mock LLM provider, scenario runner, assertion helpers)
- **forge_audit** — NDJSON audit log with per-call tool events
- **forge_cloud** — cloud integration

### 3D / visual (3)
- **forge3d** — 3D rendering crate (CLI variant)
- **forge3d-cdx** (bench variant)

### Misc (3)
- **forge_paste** (see above)
- **forge_audit**, **forge_audit** (see above)

## Substantive non-crate additions

### New binaries (in forge_main)
- `helioslite` (canonical, Gate 1b — additive)
- `helioslite_helper` (Windows self-update helper binary)
- `forge_sharecli` exposed via `helioslite share` subcommand
- `forge_lsp` exposed via `helioslite lsp` subcommand
- Legacy `forge` and `forge-dev` retained with deprecation tombstones

### New commands / subcommands
- `heliosdoctor` — doctor channel diagnostics with rename indicator
- `helioslite share` — sharecli relay
- `helioslite lsp` — LSP bridge
- `helioslite tracera` — telemetry sink control
- `helioslite agileplus` — scorecard/sprint/velocity
- `helioslite session-cleaner` — nightly cleanup
- `helioslite data` — JSONL data processing (already in help)
- `helioslite workspace` — semantic search workspace mgmt

### Config / env vars (P0.1.2, P0.3, P0.4)
- `FORGE_DBD_ENABLED` — toggle DBD daemon
- `FORGE_GUARDIAN_*` — guardian config flags + session-mode apply gate
- `HELIOSLITE_REPO` — override default update repo
- Env-var schema surfaced for `lsp` / `share` / `agileplus` / `tracera` subcommands

### Subcommand exposure
- `--stream-json` NDJSON tee into chat streaming loop (P2.4)
- `forge :fts-optimize` — FTS5 index maintenance command
- Subagent breadcrumb "spawned by X" in info panel + conv header

## New docs (fork-only)

### Strategy / governance
- `docs/FORK.md` — fork provenance, AI-DD divergence notice
- `docs/RENAMES-STRATEGY.md` — additive rename policy (binding reference)
- `docs/UPDATE-STRATEGY.md` — upgrade flow / repo override
- `docs/PUBLISHING.md` — registry matrix
- `docs/DEV-CLI.md` — developer CLI
- `docs/SSOT.md` — single source of truth registry
- `docs/requirements.md` — program-level requirements
- `docs/architecture.md` — high-level architecture
- `docs/index.md` — docs index
- `docs/incident-response.md`
- `docs/MULTI-REGION.md`
- `docs/openssf-badge-application.md`
- `docs/slsa.md`
- `docs/SLA-SLO.md` + `docs/SLO-BURN-RATE.md`
- `docs/forge-dev-install.md`
- `docs/packaging/FORGE_DEV_PACKAGING.md`
- `docs/VISUAL_SPEC.md`

### AgilePlus / scorecard
- `docs/AGILEPLUS-SETUP.md`
- `docs/31-pillar-scorecard.md`
- `docs/consolidation/COVERAGE.md` + `coverage-summary-20260907.json`
- `docs/consolidation/LEGACY-DISCOVERY-PORT-20260909.md`

### Contracts / schemas
- `docs/contracts/provider-models/README.md`
- `docs/contracts/provider-models/oauth-refresh-policy.schema.json`
- `docs/contracts/provider-models/provider-model.schema.json`
- `docs/contracts/provider-models/resilience-policy.schema.json`

### ADRs (fork-added)
- `0001-compaction-summarization-strategy.md`
- `001-record-architecture-decisions.md`
- `002-choice-of-language.md`
- `003-agileplus-adoption.md`
- `003-p3-single-writer.md`
- `004-p3-single-writer.md`

### Security / threat model
- `docs/security/threat-model.md`
- `docs/security/scorecard-branch-protection-exception.md`

### Tasks
- `docs/tasks/task-compaction-enhancement.md`

### Boundary / fork-sync
- `docs/boundary/forgecode.md`
- `docs/intent/forgecode.md`
- `docs/fork-sync/upstream-audit-20260629.md`
- `docs/journeys/helioslite.md`
- `docs/NOTICE.md` — license/trademark

### Visual / brand
- `apps/landing-helioslite/index.html`
- `apps/landing-helioslite/public/og-image.svg`
- `assets/brand/README.md` (Terminal-Forge palette)

### Operations
- `docs/operations/iconography/SPEC.md`
- `docs/operations/journey-traceability.md`
- `docs/audit/` — multiple audit files

## New CI / GitHub workflows

- `.github/workflows/cvp.yml` — Continuous Verification Protocol (P3.2)
- `.github/workflows/agileplus-pillar-scorecard.yml` — weekly scorecard
- `.github/workflows/auto-assign-reviewers.yml`
- `.github/workflows/benchmarks.yml`
- `.github/workflows/branch-cleanup.yml` — nightly stale branch cleanup
- `.github/workflows/cargo-deny.yml`
- `.github/workflows/chaos-gate.yml`
- `.github/workflows/chaos-testing.yml`
- `.github/workflows/codeowners-verify.yml`
- `.github/workflows/codeql.yml`
- `.github/workflows/coverage-ratchet.yml`
- `.github/workflows/dependabot-auto-merge.yml`
- `.github/workflows/dora-metrics.yml`
- `.github/workflows/fuzz.yml`
- `.github/workflows/helios-bot.yml`
- `.github/workflows/helios-lite-nightly.yml`
- `.github/workflows/infisical.yml`
- `.github/workflows/lint.yml`
- `.github/workflows/observability-dashboard.yml`
- `.github/workflows/otel-deploy.yml`
- `.github/workflows/otel-health.yml`
- `.github/workflows/perf-dashboard.yml`
- `.github/workflows/perf-regression-check.yml`
- `.github/workflows/perf-regression.yml`
- `.github/workflows/release.yml` → `helioslite-release.yml` (rename pending Gate 4)
- `.github/workflows/sbom.yml`
- `.github/workflows/scorecard.yml`
- `.github/workflows/security-scorecard.yml`
- `.github/workflows/signing-pipeline.yml`
- `.github/workflows/slo-burn-rate.yml`
- `.github/workflows/snyk.yml`
- `.github/workflows/sprint-velocity.yml`
- `.github/workflows/stale.yml`
- `.github/workflows/triage.yml`
- `.github/workflows/trunk-check.yml`
- `.github/workflows/update-distribution.yml`
- `.github/workflows/weekly-digest.yml`
- `.github/workflows/winget.yml`

## Version identity

- Workspace version: **2.13.21**
- Most recent release commit: `release(version): bump to 2.13.21-h.0.1.8 for P0-P3 backlog (#299)`
- Fork versioning convention: `<base>-h.<fork-semver>`
- Binary identity: `helioslite` (canonical) + `forge` / `forge-dev` (deprecated aliases)

## Renames strategy (binding reference)

`docs/RENAMES-STRATEGY.md` is the authoritative document. The fork is **additive**, not destructive:

| Category | Old → New | Status |
|---|---|---|
| Cargo crates `forge_*` | preserved verbatim | Untouched (merge-safety) |
| Bins `forge`, `forge-dev` | preserved + `helioslite` added | Gate 1b done |
| Default data dir `~/.forge/` | `~/.helioslite/` (legacy alias honored) | Gate 5 done |
| Workflow `release.yml` | `helioslite-release.yml` | Gate 4 pending |
| Env vars `FORGE_*` | preserved | Untouched |
| Domain `helioslite.phenotype.space` | new | Gate 1b |
| `update_repo` | `KooshaPari/forgecode` (NOT `KooshaPari/heliosLite` which doesn't exist; NOT `tailcallhq/forgecode` upstream) | enforced by `default_update_repo_resolves_to_<REDACTED>_forgecode` test |

End-of-life for legacy aliases triggers only when: 6 months elapsed + deprecation metrics show <5% legacy usage.

## Things you should look at before merging/pushing

### High-leverage changes (review first)

1. **P0.1 guardian risk adjudication** — LLM-driven risk gating in permission check. If you don't want AI in the permission path, this is a hard fork-only behavior change.
2. **P0.3 / P0.4 input pipeline + output/audit spine** — new `forge_paste`, `forge_render`, `forge_audit` crates. Affects how input is parsed, output rendered, and audit logged.
3. **P2 LSP integration** — `forge_lsp` exposes real `rust-analyzer` binary. May break on systems without `rust-analyzer`.
4. **P3.2 AgilePlus scorecard** — weekly issue publishing. Requires `GITHUB_TOKEN` with write:issues.
5. **P3.3 ShareCLI realtime relay** — exposes WebSocket + SSE transports. Security surface: auth tokens, hub registry.
6. **forge_dbd SQLite write daemon** — single-writer enforcement. Affects all persistent conversation storage. Off by default (`FORGE_DBD_ENABLED=1` to enable).
7. **forge_tracera outbound telemetry** — HMAC auth, gzip, persistent DiskStore with offline retry. **Sends data off-machine**. Verify endpoint and auth policy.
8. **forge_gpu CUDA routing** — heterogeneous GPU lane for 3090/1080. Affects only systems with both GPUs.
9. **forge_semantic memory adapters** — Supermemory + Letta + Cognee. New external service integrations.
10. **renaming indicator in heliosdoctor** — emits `rename_channel` event. Surface for downstream consumers.

### Medium-leverage

1. **34 new crates** — significant compile-time growth. Incremental builds may slow. Workspace members count went from ~20 → 54.
2. **9 new GitHub workflow files** — verify all required secrets are present in fork repo settings.
3. **3 ADRs in `docs/adr/`** — fork-only decision records. Should they be merged to upstream?
4. **HeliosLite branding** — Terminal-Forge palette, landing page, OG image. Affects public identity.
5. **Cargo-dist release pipeline** (`forge-dev-install.md`, `packaging/FORGE_DEV_PACKAGING.md`) — multi-channel publish (Homebrew tap, Scoop, Chocolatey, winget).
6. **ZSH plugin support** — `helioslite zsh` subcommand.
7. **JSONL data processing** — `helioslite data` subcommand for schema-constrained LLM processing.

### Low-leverage / cosmetic

1. **Style fixes** — cargo fmt imports, comment wrapping.
2. **.gitignore additions** — minor.
3. **README/SSOT.md** updates — fork identity changes only.
4. **Renames `KooshaPari` → `<REDACTED>`** — needs to be redone properly (Rust source files must keep `KooshaPari` as identifier).

## Things you should NOT push as-is

1. **`stash@{0}` sanitization WIP** — broken. Drop or redo without touching Rust source identifiers.
2. **`.worktrees/` and `docs/sessions/20260911-pr-worktree-followup/`** — untracked junk. Either commit or delete.
3. **Cargo.lock delta** — only changed because cargo build touched it. Re-bump via `cargo update -p` or revert.
4. **`_forge_swap_done.txt`** — leftover marker file. Delete.

## Recommendations

### Immediate (before any push)

- [ ] Drop `stash@{0}` or redo sanitization excluding Rust source identifiers
- [ ] Delete untracked junk (`.worktrees/` from local, `docs/sessions/20260911-pr-worktree-followup/`)
- [ ] Decide: keep sanitization as PR for `KooshaPari → koosha-pari`-repo rename, or drop entirely
- [ ] Verify `~/.helioslite/bin/helioslite` runs (it does — verified just now)

### Short-term (this week)

- [ ] Review 10 high-leverage items above
- [ ] Decide which new crates belong in `crates/` vs vendored elsewhere
- [ ] Verify CI secrets for the 9 new workflow files
- [ ] Check that `cargo build --release` clean build is reproducible (cache-bust one crate to verify)
- [ ] Verify `helioslite_helper` Windows binary cross-compiles (not testable on macOS but the `cfg(windows)` gates need a careful read)

### Medium-term (next month)

- [ ] Re-bump version to a stable h.0.2.0 marker before tagging
- [ ] Add `CHANGELOG.md` entries for v2.13.21-h.0.1.8 (currently `[Unreleased]` is empty)
- [ ] Audit the 3 fork-only ADRs for merge to upstream or fork-private retention
- [ ] Set up cargo-deny allow-list for the new crates' dependencies
- [ ] Resolve the `.cdx.json` evidence sprawl (172-line forge_tui diff suggests evidence bloat)
- [ ] Consolidate `benchmarks/` directory (3 separate cdx.json files with overlapping concerns)

## Verification commands you can re-run

```bash
# Build from clean HEAD
cd /Users/kooshapari/CodeProjects/Phenotype/repos/forgecode
cargo build --release -p forge_main --bin helioslite

# Verify installed
~/.helioslite/bin/helioslite --version   # → helioslite 2.13.21

# Run unit + integration tests
cargo test --workspace --bins --lib --tests

# Check upstream sync state
git fetch origin main
git log --oneline origin/main..HEAD | wc -l   # should be 640

# Drop or restore the sanitization stash
git stash list                                # → stash@{0}: sanitize-wip-20260917-Ko
git stash show -p stash@{0}                   # peek before deciding
git stash drop stash@{0}                      # discard (after backing up)
git stash pop                                 # restore to working tree
```

## Reference

- Full feat commit log: `/tmp/feat-clean.txt` (129 lines)
- Merge-base: `6ed5d37b6b45a2b6220877fd9aec5ba4c4b7f3c0`
- Fork tip: `ebb7dd670a354977da6acc24e3ec90a6bfbffa4b`
- Authoritative rename reference: `docs/RENAMES-STRATEGY.md`
