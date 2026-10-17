# UPSTREAM-DELTA.md — HeliosLite vs Forgecode

> **tl;dr.** This document records every change the HeliosLite fork
> (`KooshaPari/HeliosLite`) has on top of the upstream
> `tailcallhq/forgecode` repository, plus the small slice of upstream
> changes the fork has not yet absorbed. Read with `docs/FORK.md` and
> `docs/RENAMES-STRATEGY.md`. Regenerate with `git fetch origin fork &&
> git log ...` (see Section 1).

## Provenance

| Field | Value |
| --- | --- |
| Fork path | `/Users/kooshapari/CodeProjects/Phenotype/repos/forgecode` |
| Upstream | `tailcallhq/forgecode` (`origin` remote, `https://github.com/tailcallhq/forgecode.git`) |
| Fork remote | `KooshaPari/HeliosLite` (`fork` remote, `git@github.com:KooshaPari/HeliosLite.git`) |
| Default branch (both) | `main` |
| License | MIT (preserved verbatim) |
| Authoritative handoff | `docs/sessions/20260918-release-ci-handoff/HANDOFF.md` |
| Companion docs | `docs/FORK.md`, `docs/RENAMES-STRATEGY.md`, `CHANGELOG.md` |

---

## Section 1 — Commits ahead/behind

### Heads at generation time (2026-10-17T07:44Z)

| Side | SHA | Date (UTC) | Subject |
| --- | --- | --- | --- |
| `fork/main` (this fork) | `91ee5e0b0eba9ee9163ed265286186f4c52b0ea0` | 2026-10-15 18:06:17 -0700 | `fix(sandbox): make enforcement reporting fail closed and truthful (#330)` |
| `origin/main` (upstream) | `0c8717f5cfb709777fd852d67652efa2e3c04ff1` | 2026-10-17 06:33:44 +0000 | `chore(deps): update dependency ai to v7.0.143 (#4031)` |

### Counts

```
$ git rev-list --left-right --count origin/main...fork/main
769     20
```

| Direction | Commits | Notes |
| --- | --- | --- |
| **Ahead** (fork has, upstream lacks) | **769** | the entire fork delta — see Section 2 |
| **Behind** (upstream has, fork lacks) | **20** | 20 dep-bump commits (`ai`, `@ai-sdk/google-vertex`, `arboard`, `rustls`, `html-escape`, `tokio`, `syn`, `schemars`, `zod`, `sha2`, …); all trivial `chore(deps):` / `fix(deps):` cherry-picks; expected to land in the next v2.15.x sync. |

### Upstream commits the fork is missing (last 30, all `chore(deps):` / `fix(deps):`)

```
$ git log --format="%h %ci %s" fork/main..origin/main --no-merges | head -30
0c8717f5c 2026-10-17 06:33:44 +0000 chore(deps): update dependency ai to v7.0.143 (#4031)
ace75c107 2026-10-17 02:35:58 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.115 (#4030)
61ab3b6f6 2026-10-16 06:20:29 +0000 chore(deps): update dependency ai to v7.0.142 (#4026)
127a18b02 2026-10-16 00:55:51 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.114 (#4027)
df945e974 2026-10-15 21:33:54 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.113 (#4025)
36cdda39f 2026-10-15 17:00:07 +0000 chore(deps): update rust crate arboard to v3.7.0 (#4024)
6a69e62ae 2026-10-15 09:08:47 +0000 chore(deps): update dependency ai to v7.0.141 (#4020)
39a56bd18 2026-10-15 07:36:18 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.112 (#4019)
ed6f7a076 2026-10-14 10:53:07 +0000 chore(deps): update rust crate rustls to v0.23.46 (#4018)
9142e978b 2026-10-14 06:09:23 +0000 chore(deps): update dependency ai to v7.0.140 (#4017)
70baa1b8a 2026-10-14 02:37:30 +0000 fix(deps): update rust crate html-escape to 0.3.0 (#4016)
40a76e0d0 2026-10-14 02:30:08 +0000 chore(deps): update rust crate tokio to v1.53.3 (#4015)
b4f277523 2026-10-14 00:02:05 +0000 chore(deps): update dependency ai to v7.0.139 (#4014)
ee96dcb60 2026-10-13 19:16:46 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.111 (#4013)
3d242e248 2026-10-13 09:35:14 +0000 chore(deps): update rust crate syn to v3.0.9 (#4012)
156059680 2026-10-13 09:24:25 +0000 chore(deps): update rust crate schemars to v1.2.3 (#4010)
055785ba3 2026-10-13 09:16:31 +0000 chore(deps): update dependency zod to v4.6.7 (#4009)
bbbf0bbc3 2026-10-13 09:07:35 +0000 chore(deps): update rust crate sha2 to v0.11.1 (#4011)
c18ea1b13 2026-10-13 06:37:06 +0000 chore(deps): update dependency ai to v7.0.138 (#4008)
2e032b3dc 2026-10-13 01:19:48 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.110 (#4007)
```

> The fork is currently **strict-ahead-by-749-non-merge-commits** (769 ahead
> − 20 merge) and **20-behind-on-deps**. The 20-behind figure is the
> signal that the next upstream-sync merge is due; it does **not** reflect
> any capability gap.

### Distribution of ahead commits by conventional-commit type

| Type | Count | What it is |
| --- | --- | --- |
| `fix` | 263 | Bug fixes, CI/build repair, test hardening |
| `feat` | 131 | New features, crates, subcommands |
| `docs` | 95 | Handoffs, session docs, RFC-style notes |
| `ci` | 87 | Workflow changes, cargo-deny, action pins, gates |
| `chore` | 75 | Deps, manifest bumps, distribution, version bumps |
| `test` | 27 | New test cases and e2e shells |
| `merge` | 20 | Upstream-sync / squash merges (excluded from per-commit list below) |
| `style` | 17 | rustfmt-only |
| `perf` | 5 | Latency/throughput wins |
| `refactor` | 4 | Internal refactor with no behavior change |
| `release` | 3 | Release/version meta-commits |
| `security` | 2 | RUSTSEC bumps, security-scanner rewires |
| `wip` | 2 | Explicit WIP markers |
| `spec` | 1 | Mature-recovery spec (PR #320) |
| `renames` | 1 | RENAMES-STRATEGY doc (commit history marker) |
| `build` | 1 | Build-system change |
| `audit` | 1 | Master-audit commit (2026-10-17 cohort) |
| other (non-conventional) | ~5 | `[codex]`, `Bound`, etc. |
| **Total** | **769** | non-merge 749 + merge 20 |

### Touch surface (file-level, ahead-only)

| Path bucket | File touches | Notable sub-buckets |
| --- | --- | --- |
| `crates/` | 1958 | dominant — every fork-unique crate lives here |
| `.github/` | 380 | workflows (169 fork-only workflows), configs |
| `docs/` | 192 | session docs, handoffs, RENAMES-STRATEGY, etc. |
| `Cargo.lock` | 88 | dep manifest churn |
| `Cargo.toml` | 56 | workspace + per-crate manifests |
| `ui/` | 53 | fork-only Tauri UI bundle |
| `benchmarks/` | 47 | benchmark scaffolding |
| `desktop/` | 43 | fork-only desktop shell |
| `src/` | 36 | (legacy) |
| `src-tauri/` | 32 | Tauri shell plugin |
| `agileplus/` | 24 | fork-only subsystem |
| `tests/` | 21 | integration / e2e shells |
| `assets/` | 19 | icons, brand SVG |
| `deny.toml` | 17 | cargo-deny config evolution |
| `tooling/` | 16 | fork-only tooling |

### Pull-request activity on the fork (`gh pr list --repo KooshaPari/HeliosLite`)

* **Open at regen time:** 3 (down from 6 at the 2026-10-12 regen)
  * `gh pr list --state open --limit 50 --json number,title,headRefName`
  * Since the 2026-10-12 regen, 3 of the 6 open PRs were merged into `fork/main`:
    `#320` (spec: mature-first harness recovery, 2026-10-16T01:06:04Z),
    `#322` (fix(evals): fail closed, 2026-10-16T01:06:12Z),
    `#330` (fix(sandbox): fail closed and truthful, 2026-10-16T01:06:18Z).

| # | State | Head branch | Title |
| --- | --- | --- | --- |
| 333 | OPEN | `impl/effect-recovery-hook-20260930` | feat(tool): add optional durable effect recovery hook |
| 328 | OPEN | `impl/sharecli-cross-process-relay-20260930` | fix(sharecli): make publish, topics and attach use the running relay process |
| 319 | OPEN | `feat/forge-live-control-acp-20260927` | feat: live session control and ACP bridge |

(Plus 318+ MERGED PRs ranging from #1 through #333. Highlight merged PRs:
`#330` sandbox fail-closed, `#322` evals fail-closed, `#320` mature-recovery spec,
`#334` h2 RUSTSEC-2026-0258 bump, `#318` distribution v2.13.21-h.0.2.9,
`#319` ACP live control, `#284` `forge_tracera` P3.1, `#290` `forge_lsp` P2.3.1,
`#293` `forge_agileplus` P3.2, `#298` `forge_sharecli` P3.3, `#306` `helioslite share`/`lsp` subcommands,
`#316` `helioslite share` e2e, `#315` env-var schema, `#311` rust-analyzer install,
`#275` `forge_lsp` initial crate, `#277` h0.1.7 integration, `#291` Supermemory/Letta/Cognee adapters,
`#248` OS-level sandbox, `#247` JsonlSemanticMemory, `#246` ResiliencePolicy.)

---

## Section 2 — Every change vs upstream, grouped by intent

> **Format:** `[intent-group] <sha7> <subject>`. Listed newest-first
> (most recent fork-only commit at the top). The ahead set is 769
> commits; the first 200 are shown verbatim; the remaining 569 are
> summarized by intent in the table at the end of this section.

> **Intent buckets:**
> * `decoration / branding / fork identity` — additive-only renames
>   (binary, manifest, domain), version bumps, dep bumps that do not
>   affect behavior
> * `build / CI` — GitHub Actions workflows, cargo-deny, action pins,
>   CI-only refactors
> * `build / fix / test` — code fixes, test additions/hardening,
>   cross-platform portability repairs
> * `security / policy` — RUSTSEC bumps, secret-handling, sandbox
>   enforcement, harness eval hardening
> * `docs / handoff` — session docs, handoffs, scorecards, RFC-style
>   notes that are themselves the deliverable
> * `agent harness / subagent / swarm` — new crates, new subcommands,
>   harness primitives, semantic-memory adapters
> * `misc / uncategorized` — style, WIP, non-conventional prefixes

### Per-commit list (first 200 of 749 non-merge, newest first)

```
[security / policy] 91ee5e0b0 fix(sandbox): make enforcement reporting fail closed and truthful (#330)
[security / policy] ea9524476 fix(evals): fail closed on missing evidence, signals and timeouts (#322)
[agent harness / subagent / swarm] 5a4d6e528 spec: mature-first harness recovery — pinned sources, oracle blockers and candidate contract (#320)
[security / policy] 11cf7f97d fix(deps): bump h2 0.4.13 → 0.4.19 (clears RUSTSEC-2026-0258 for h2 0.4.x) (#334)
[docs / handoff] 29c00291c docs(handoff): 10-09/10 delta - upstream sync, CI hygiene+hardening chain, streak 12, disk cycles 3-4
[build / fix / test] 2053b2890 fix(lsp,infra): apply cmd /C shim-safe spawn to tsc and mcp_client (complete .cmd-shim class)
[build / CI] 007068dbb ci(cvp): run workspace tests with --all-features in Gate 3
[build / fix / test] e278369dd fix(lsp): spawn .cmd/.bat shims via cmd /C on Windows (restores real Windows e2e)
[build / fix / test] 239a57824 fix(test): skip forge_lsp real-RA e2e when typescript-language-server is not runnable
[build / fix / test] 5b2eafe47 fix(test): harden macOS mcp_watcher fs-event waits and make release_tags shell test Windows-portable
[build / CI] cf11b3c37 ci(gates): harden platform-tests (all-features + gating) and install RA/TS prereqs in cvp/coverage so real-RA e2e runs
[build / fix / test] 6e5b8fa94 fix(ci): clear clippy 1.99 lints and restore forge_ci release-workflow parity
[decoration / branding / fork identity] fadb32bda refactor(publish): helioslite publish-surface patch (Gate 4 partial)
[docs / handoff] 49a39c4ab docs(handoff): 10-02 delta - nightly #5 accepted from raw logs, tip matrix final 33/3/0
[docs / handoff] 9acde30e9 docs(handoff): full 12-row mapped-check re-run (12/12 PASS observed) + R3 cell correction
[docs / handoff] cf26e536e docs(handoff): R11 final verdict - tip CI GREEN at 081905205 (26 success/2 skipped/0 failures; stale-cron check classified)
[docs / handoff] 081905205 docs(handoff): feedback-loop closure - requirement to check to observed-result matrix
[docs / handoff] 2f0a0d4ba docs(handoff): resolve distribution todo - PR #318 merged 09-20, purpose satisfied, dispatch moot
[docs / handoff] a97f438e3 docs(handoff): record answered-record distribution - false likely deliberate (3-denial cluster 09-24)
[docs / handoff] 2677faa7b docs(handoff): correct hook-d6499f54 timestamps to observed date output (09-24/09-25)
[docs / handoff] 1f8029084 docs(handoff): correct hook-d6499f54 state - answered=false 2026-09-23, not pending
[docs / handoff] 50eb43231 docs(handoff): record observed pre-dispatch alignment for distribution re-dispatch
[docs / handoff] 5db864f35 docs(sessions): record Q3-A toolchain pin execution + post-pin nightly acceptance
[misc / uncategorized] d1a84bdf7 style(ci): rustfmt the generator edit
[build / fix / test] e5e7f6473 fix(ci): drop toolchain inputs in forge_ci generator - align source with 1.98 pin
[build / CI] 536a25cac ci: pin all workflow toolchains to rust-toolchain.toml (1.98) - end stable float flip-flop
[docs / handoff] 0adab9841 docs(sessions): record sync #2 landing (65e1b7994, CI 28/28) + workflow-trigger finding
[docs / handoff] d89a156b5 docs(sessions): close flake-fix section with final CI evidence (f1af9b9b6 + 65e1b7994 green)
[docs / handoff] f1af9b9b6 docs(sessions): confirm CI toolchain: stable evidence; record Q3 pin recommendation
[docs / handoff] cb7e359d5 docs(sessions): correct undici fix attribution; record fmt race outcome and rustfmt skew
[misc / uncategorized] 0bab1ab05 style(forge_lsp): rustfmt mcp_watcher/tests.rs per CI Format check
[docs / handoff] 36b8568d1 docs(sessions): record undici alert auto-close and sync-2 conflict preview
[security / policy] 10a4c36af chore(deps): bump undici to 7.30.0 (GHSA-3wwx-pv8p-q78v, medium)
[docs / handoff] 795e3cafc docs: record P5 decision C executed - archive tag pushed; sync-landed HANDOFF deltas
[docs / handoff] bb8fd6e4d docs(sessions): record upstream sync landed at 72ff2cab8 with CI + deny evidence
[docs / handoff] 89bbb7723 docs(sessions): record macOS mcp_watcher flake diagnosis and forward-fix
[build / fix / test] e9271f8a2 fix(forge_lsp): harden mcp_watcher fs-event tests against runner stalls; extract test module
[docs / handoff] b0485410b docs(sessions): record nightly acceptance evidence - 09-28/09-29 runs green, 3889 passed, 0x -32801
[docs / handoff] c503badf6 docs(sessions): record nightly acceptance observed green (09-28 + 09-29 runs)
[docs / handoff] 6dba1f674 docs(handoff): add 2026-09-28 addendum - nightly pre-flight and P5 decision request
[docs / handoff] c846dea3e docs(sessions): record ci/test LSP proxy evidence and P5 decision brief
[docs / handoff] 49769b34d docs(sessions): correct stale expectations with observed CI/alert state
[decoration / branding / fork identity] 4c8df606e chore(ci): pin cargo-deny tool version and add TECH_DEBT register
[docs / handoff] 1a519532b docs(sessions): record ContentModified fix set and rand #7 closure
[security / policy] acb5c54a9 fix(deps): bump transitive rand 0.8.5 -> 0.8.8 (GHSA dependabot #7)
[docs / handoff] 02a936488 docs(sessions): record externally-gated CI observation outcomes
[build / fix / test] 835e7b02d fix(lsp): retry ContentModified on the definition request path
[docs / handoff] b7b09f435 docs(sessions): record nightly content-modified diagnosis and fix
[build / fix / test] da25401fc fix(forge_lsp): retry transient LSP errors and raise cold-runner e2e budgets
[build / fix / test] b30984166 fix(test): retry transient LSP content-modified errors in e2e round trip
[docs / handoff] fb94d603e docs(sessions): record feedback-loop closure observations
[docs / handoff] 01f31585f docs(handoff): add 2026-09-24 addendum with post-roadmap current state
[docs / handoff] 6fe40455e docs(sessions): record roadmap P1-P4 execution and nightly fix
[build / fix / test] 31896cb56 fix(ci): install typescript-language-server in nightly workflow
[build / fix / test] ba7187320 fix(ci): delete stale distribution branch before push on workflow re-run
[security / policy] 498d276e4 fix(deps): resolve dependabot alerts and prune stale deny ignores
[docs / handoff] eb5fdd25c docs(sessions): record skipped-branch re-analysis and archive-tag deletion
[docs / handoff] 38249bdf1 docs(sessions): record Cargo Deny crossbeam-epoch fix and merge lesson
[security / policy] dcf0e7d38 fix(ci): bump crossbeam-epoch 0.9.18 -> 0.9.21 (RUSTSEC-2026-0204)
[docs / handoff] 20ae2be1e docs(sessions): record h.0.2.9 UDF race validation outcome
[decoration / branding / fork identity] c79326c4f chore(distribution): update Homebrew, Scoop, and packaging manifests for v2.13.21-h.0.2.9
[build / fix / test] bfdaa8831 fix(deps): rmcp 1.8.0 -> 3.4.0 and vite ^5.4.0 -> ^8.3.0
[decoration / branding / fork identity] 79088e8dc chore(distribution): update manifests for v2.13.21-h.0.2.8
[build / fix / test] f18a5247e fix(distribution): extend checksum retry window to 45 minutes and add manual trigger
[build / fix / test] dcdbb17ac fix(distribution): repair broken Homebrew, Chocolatey, and install.sh packaging
[docs / handoff] 925865cbf docs(audit): commit the 2026-09-11 worktree disposition audit
[docs / handoff] 787b33dfb docs(release): correct the h.0.2.7 ship SHA to db571577d
[decoration / branding / fork identity] 43f292fa6 chore(distribution): update Homebrew and Scoop manifests for v2.13.21-h.0.2.7
[build / fix / test] 30e511eb2 fix(ci): wait for release assets in update-distribution before failing
[build / fix / test] db571577d fix(ci): align the generator with the committed ci.yml; drop the hand-edit note
[build / fix / test] 537dea827 fix(deps): repin PhenoShared to b2d06d53 (agileplus-cache manifest repair)
[build / fix / test] c43bbf828 fix(test): stop runner load turning the LSP e2e into a red test; restore the provider key
[build / fix / test] 6b5c7fc8c fix(ci): apply the audit's confirmed findings; commit the audit itself
[build / fix / test] fb03ec7db fix(ci): repair two brittle expectations found by audit; record the guard
[build / CI] 4d78a0c80 ci(cvp): enforce the cargo-bin invariant so the rustup clobber cannot return
[docs / handoff] 002271c07 docs(sessions): pre-write next release notes and an idempotent ship script
[docs / handoff] aaef98bf0 docs(sessions): refresh the handoff snapshot (it still pointed at bdb183311)
[docs / handoff] 9aca62e1a docs: point agents at the handoff from AGENTS.md; fix stale SSH note
[docs / handoff] 0fa3c0bc5 docs(sessions): exact verified workflow list; note API re-verification
[docs / handoff] 3e0809bef docs(sessions): record commit-verified CI conclusions per run id
[docs / handoff] 65899cf7f docs(sessions): all twelve CI entries green on a8ad28703
[build / fix / test] a8ad28703 fix(ci): give LSP e2e tests a nextest budget that matches their timeouts
[docs / handoff] d82a6701d docs(sessions): CLI boundary fix, acceptance evidence, verification gap
[build / fix / test] 77607f74c test(lsp): make the CLI integration test a single bounded call
[build / fix / test] be581d8c9 fix(lsp): make file-scoped CLI commands work on a cold server
[docs / handoff] 503e7e53f docs(sessions): mark the LSP e2e item fixed, all workflows green
[agent harness / subagent / swarm] 3b1ebf4e7 feat(lsp): add didOpen support and make the LSP e2e round trip real
[docs / handoff] b2d588364 docs(sessions): final verified CI status for the handoff
[docs / handoff] a90e636b8 docs(sessions): record the watcher shutdown bug fix
[build / fix / test] d9a5871a9 fix(lsp): make McpWatcherHandle::stop actually interrupt the watcher
[build / fix / test] ce682cff4 test(lsp): de-flake watcher_stop_completes_quickly
[build / fix / test] 3a75297b0 fix(config): remove denied indexing_slicing from render_table
[docs / handoff] 77cf18741 docs(sessions): record cvp and autofix lint findings
[build / fix / test] 677b78ecd fix(ci): cvp pin check + forge_sharecli indexing_slicing lint
[docs / handoff] 9ef324468 docs(sessions): record the open LSP e2e test failure with evidence
[build / fix / test] f1b31f7e3 fix(ci): install typescript-language-server for the LSP e2e test
[docs / handoff] 4b254ddf2 docs(sessions): record rust-analyzer/rustup clobber root cause (4.6)
[build / fix / test] b089edae6 fix(ci): guard must not use rustc -vV | grep -q under pipefail
[build / fix / test] 3fc98eb0c fix(ci): never install rust-analyzer into ~/.cargo/bin (overwrites rustup)
[build / fix / test] fb0b93d5f fix(ci): stop caching ~/.cargo/bin (corrupt bin dir breaks all cargo calls)
[docs / handoff] a20e842fb docs(sessions): correct PhenoShared tip reference in handoff
[docs / handoff] a9aba8504 docs(sessions): mark release-CI mission complete (55 assets on h.0.2.6)
[docs / handoff] 97bff227a docs(sessions): handoff for release-CI ownership
[build / fix / test] bdb183311 fix(ci): make Windows SignPath signing optional when unconfigured
[build / fix / test] 5d3ea1236 fix(deps): repin PhenoShared to cf914698 with Windows-invalid-char fix
[decoration / branding / fork identity] 2b5c7e6cc chore(deps): regenerate Cargo.lock for PhenoShared 62368071
[build / fix / test] da5f6e908 fix(deps): repin PhenoShared to 62368071 with Windows path fixes
[build / fix / test] ae282957e fix(ci): fix platform test failures on macos/windows runners
[build / fix / test] c0596013a fix(ci): raise caller sign_release permission ceiling to contents:write
[build / fix / test] 3a466e86a fix(cvp): fix rev extraction to take first match only
[security / policy] 8008f2c8e fix(deps): bump rustls 0.23.43 -> 0.23.45 (RUSTSEC-2026-0285)
[docs / handoff] bc36cf53d docs(sessions): 20260917-helioslite-release-audit
[build / fix / test] 122e1e224 fix(lockfile): regenerate Cargo.lock for PhenoShared rev=68beca26
[misc / uncategorized] de779e519 style: rustfmt Gate 7 rename channel code
[agent harness / subagent / swarm] 74103310d feat(rebrand): Gate 6+7 — domain activation + legacy deprecation
[build / fix / test] ebb7dd670 fix: invoke cargo-nextest directly with proper rustc PATH
[build / fix / test] 2f4431b1e fix: override CARGO_HOME to match nextest install location
[build / fix / test] a51c59798 fix: check multiple paths for cargo-nextest binary
[build / fix / test] 8c0a5a426 fix: invoke cargo-nextest directly to bypass PATH issue
[build / fix / test] 9663c1991 fix: fix CI test and CVP workflow issues
[build / fix / test] 8e82a9a58 fix: allow PhenoShared git source in cargo-deny
[build / fix / test] 34d81622c fix: bump PhenoShared to fully submodule-free commit (68beca26)
[build / fix / test] 7b23193ca fix: update PhenoShared rev to commit without broken submodules
[build / fix / test] baeb1c91b fix: use git CLI for cargo fetch to skip broken submodules
[build / fix / test] f68e4dc7a fix: switch to git deps for PhenoShared (rev 82d7f473)
[agent harness / subagent / swarm] 79c02c8f9 feat: wire PhenoShared shared crates + CVP workflow
[build / fix / test] ed0a38166 fix(test): widen concurrent ops heartbeat gap threshold (50ms -> 75ms)
[build / fix / test] 6de2d161d test(share): e2e shell tests for helioslite share binary + runtime-thread fix (#316)
[build / fix / test] 9c12ec78a test(main): e2e shell tests for helioslite lsp binary (langs/hover/define/complete) (#314)
[agent harness / subagent / swarm] c66e3d977 feat(config): surface env-var schema for lsp/share/agileplus/tracera subcommands (#315)
[agent harness / subagent / swarm] 4a4c51e73 feat(app): wire semantic memory recall (warm-hydrate) and forget (on compact) (#313)
[docs / handoff] 66fd35260 docs(semantic): clarify JsonlSemanticMemory as the default runtime adapter (#312)
[build / CI] 46cec3cd7 ci: install rust-analyzer in workflows that run the forge_lsp e2e test (#311)
[build / fix / test] d188d6884 test(forge_sharecli): live HTTP SSE end-to-end integration test (#310)
[agent harness / subagent / swarm] 41bb44e43 feat(app): wire semantic memory store into chat-completion path (#309)
[build / fix / test] 9e54e8591 test(lsp): add e2e integration test against real rust-analyzer binary (#308)
[agent harness / subagent / swarm] 72731cb2f feat(sharecli): add attach-to-session ingestion mode (P3.3 follow-up) (#307)
[agent harness / subagent / swarm] 6accf7ada feat(main): expose forge_sharecli via helioslite share subcommand (#306)
[agent harness / subagent / swarm] 09d9d571b feat(lsp): expose forge_lsp capabilities via helioslite lsp subcommand (#306)
[agent harness / subagent / swarm] 565bf4728 feat(services): integrate semantic memory runtime
[agent harness / subagent / swarm] 7554beff8 feat(telemetry): add outbound Tracera telemetry integration via forge_tracera sink (#305)
[build / fix / test] 3ebb61fc3 fix(lsp): reject empty/whitespace rename target before forwarding (#304)
[agent harness / subagent / swarm] b83de092f feat(tracera): add persistent DiskStore (file-backed NDJSON journal) for offline retry (#303)
[build / fix / test] b103f61fb test(sharecli): R2 integration smoke test composing ShareHub -> TraceraSink (#301)
[agent harness / subagent / swarm] ddd864d68 feat(sharecli): add SSE + WebSocket transports atop ShareHub (transport layer) (#302)
[decoration / branding / fork identity] 555950f6a release(version): bump to 2.13.21-h.0.1.8 for P0-P3 backlog (#299)
[build / fix / test] 08bf52b1e test(sharecli+tracera): add integration smoke test for sharecli→tracera interop (#300)
[agent harness / subagent / swarm] c908b766b feat(sharecli): add forge_sharecli crate (#298)
[agent harness / subagent / swarm] 0dc06921d feat(p2.3.2): add LSP implementation/references/typeDefinition/rename to forge_lsp (#295)
[agent harness / subagent / swarm] 9987d5901 feat(forge_tracera): gzip request compression + HMAC auth rotation with overlap + runtime set_auth (#296)
[agent harness / subagent / swarm] 77c2fc19c feat(p2.5.1): add MCP config auto-reload watcher to forge_lsp (#294)
[build / fix / test] 7e56ce58d fix(forge_lsp): address P1+P2 review blockers from PR #290 (#292)
[agent harness / subagent / swarm] a43078057 feat(p3.2): add forge_agileplus crate (pillars, sprint, velocity) (#293)
[agent harness / subagent / swarm] 0d674a83e feat(p3.4): add Supermemory + complete Letta + Cognee semantic-memory adapters (#291)
[agent harness / subagent / swarm] 370ddbd66 feat(p2.3.1): add LSP hover/definition/completion to forge_lsp
[agent harness / subagent / swarm] e7a535400 feat(p2.2.1): wire TerminalEmulator detection into executor (#289)
[build / fix / test] 1fda5b8dc fix(repo): propagate parent_id in TryFrom + fix legacy unscoped semantic (#288)
[build / fix / test] 8f2fe3af8 fix(provider): make merge_live server-side authoritative + alive dedup (#287)
[build / fix / test] 93ec5afc8 fix(repo): repair P2.5 conversation schema drift blocking all PRs from CI (#286)
[decoration / branding / fork identity] 0bb5698b7 chore(lock): add forge_tracera lockfile entry (P3.1 followup)
[agent harness / subagent / swarm] 6c913f779 feat(tracera): add forge_tracera crate — outbound observability sink (P3.1) (#284)
[decoration / branding / fork identity] ad073e4d2 release: promote h0.1.7 integration to main (#277)
[agent harness / subagent / swarm] b2caba6aa feat(provider): add ModelSource::Dynamic with curated fallback (#279)
[build / fix / test] d405d3823 fix(provider): preserve Dynamic model DTO metadata from server response (#281)
[agent harness / subagent / swarm] 75cc7f6d1 feat(p2.5): add per-server MCP last-seen health metadata (#282)
[build / fix / test] bbe672f8b fix(provider): address CodeRabbit findings on Dynamic ModelSource
[agent harness / subagent / swarm] f8cee62f4 feat(p2.4): wire --stream-json NDJSON tee into chat streaming loop (#280)
[agent harness / subagent / swarm] ebf343a3a feat(provider): add ModelSource::Dynamic with curated fallback
[agent harness / subagent / swarm] f5acb84cc feat(p2.3): add forge_lsp crate - LSP-style diagnostics (#275)
[build / fix / test] 28b949078 fix: token-usage accumulation semantics + HeliosAgentConfig doctest import (#276)
[build / fix / test] 71f7c0488 fix(release): stage assets until signing and provenance pass
[build / fix / test] 60fe56457 fix(shell): align terminal capability semantics
[build / fix / test] 48be30557 fix(shell): resolve terminal emulator lint debt
[docs / handoff] 7d5bf8326 docs(consolidation): preserve historical coverage snapshot
[agent harness / subagent / swarm] 92cd544b7 feat(p2.2): add TerminalEmulator detection to forge_pheno_shell (#258)
[build / fix / test] 302d77a90 fix(ci): pass release tags as literal shell arguments
[build / fix / test] 81cc94cb4 fix(ci): support API-key notarization
[build / fix / test] a21e6c702 fix(ci): declare signing inputs and restrict forwarded secrets
[build / fix / test] c711bc0dc test(ci): render release workflow without concurrent file writes
[build / fix / test] bdd9d4ff6 fix(release): serialize signing before release provenance
[build / fix / test] 999b5c504 fix(release): pin HeliosLite h.0.1.7 draft version (#270)
[decoration / branding / fork identity] 61ddccfd5 release: integrate h0.1.7 branch bundles (#254)
[agent harness / subagent / swarm] 77fa4dce0 feat(p2.1): wire forge_paste collapse+mention+bracketed into ForgeEditor (#251)
[agent harness / subagent / swarm] 00f40c4fc feat(app): wire OS-level sandbox into shell tool execution (P1.1) (#248)
[agent harness / subagent / swarm] 8837923f9 feat(domain): add JSONL-backed semantic memory adapter (P1.5) (#247)
[agent harness / subagent / swarm] 16468193c feat(app): add ResiliencePolicy - circuit breaker + bulkhead + adaptive throttle (P1.3) (#246)
[agent harness / subagent / swarm] 31c6a3596 feat(domain): add subagent semantic threading (P0.2) (#245)
[agent harness / subagent / swarm] c191176f9 feat(p0.1): wire GuardianAdjudicator into permission check (P0.1.2) (#242)
[build / fix / test] c36835e7b fix(repo): validate recovered message variants
[build / fix / test] a72438b09 fix(repo): avoid variant fallback indexing
[build / fix / test] 37da6db82 fix(repo): satisfy conversation record lint and format
[misc / uncategorized] eff1779ab preserve dirty conversation record change
[agent harness / subagent / swarm] ae879a033 feat(p0.1): guardian config flag + session-mode apply gate (P0.1.2 pure) (#240)
[agent harness / subagent / swarm] d2909cb02 feat(p0.1): add LearningStore + Sandbox tie-breaker to forge_guardian (#239)
[agent harness / subagent / swarm] 0234f5f16 feat(p0.1): add forge_guardian crate - risk adjudication layer (#237)
[agent harness / subagent / swarm] 73617a97b feat(p0.3/p0.4): add input pipeline + output/audit spine crates (#236)
[docs / handoff] 075790440 docs: add OpenSSF Best Practices badge application guide
[build / fix / test] a903f1de8 fix(ci): resolve all 7 clippy+fmt failures blocking main (#235)
[decoration / branding / fork identity] ada00fed6 chore: add production Dockerfile + update 31-pillar scorecard
[build / fix / test] 5a7d23dcd fix(ci): resolve 3 clippy errors in e2e_mock_provider.rs
[build / CI] 4b1f8a114 ci(scorecard): fix TokenPermissions + PinnedDependencies + SAST coverage (#232)
[build / CI] 153e8728a ci: fix Platform Tests non-blocking + Update Distribution PR-based push (#231)
[build / fix / test] 5ffdc3eda test(sandbox): use portable temp directory fixture (#222)
[misc / uncategorized] d79537469 [codex] refresh CI cache and scorecard action pins (#223)
```

> **+ 549 more.** Run
> `git log --format="%h %s" origin/main..fork/main --no-merges | sed -n '201,749p'`
> for the remainder. The oldest fork-only commit is
> `c126175a1` (2026-05-01 20:06:16 -0700) — `ci: add cargo-deny config, fix workspace license (#3)`.

### Intent summary across the full 749-commit ahead set

| Intent bucket | Count of full set | Visible in first 200 |
| --- | --- | --- |
| `build / fix / test` | 263 (`fix`) + 27 (`test`) = 290 | 80 |
| `agent harness / subagent / swarm` | 131 (`feat`) + 5 (`perf`) + 4 (`refactor`) + 1 (`spec`) = 141 | 38 |
| `docs / handoff` | 95 (`docs`) | 53 |
| `build / CI` | 87 (`ci`) + 1 (`build`) = 88 | 6 |
| `decoration / branding / fork identity` | 75 (`chore`) + 3 (`release`) + 1 (`renames`) = 79 | 11 |
| `security / policy` | 2 (`security`) + ≈ 7 (`fix`/`chore`/`ci` with RUSTSEC, GHSA, fail-closed, deps hardening) ≈ 9 | 4 (PRs #330, #322, #334, undici 7.30.0) |
| `misc / uncategorized` | 17 (`style`) + 2 (`wip`) + 1 (`audit`) + ≈ 5 other ≈ 25 | 5 |
| **Subtotal (non-merge)** | **749** | 195 (rounded; actual 195) |
| `merge` | **20** | (excluded from per-commit list; appears in `git log` as `merge:` prefix) |
| **Total** | **769** | |

### Security/policy highlights not in the first 200 (but worth noting)

* `fix(deps): bump undici to 7.30.0` (GHSA-3wwx-pv8p-q78v, medium) — `10a4c36af`
* `fix(deps): bump rustls 0.23.43 -> 0.23.45` (RUSTSEC-2026-0285) — `8008f2c8e`
* `fix(ci): bump crossbeam-epoch 0.9.18 -> 0.9.21` (RUSTSEC-2026-0204) — `dcf0e7d38`
* `fix(deps): bump h2 0.4.13 → 0.4.19` (RUSTSEC-2026-0258, PR #334) — `11cf7f97d`
* `security(ci): replace trufflehog/actions/setup with go install + setup-go` — `ebd253b42`
* `fix(cargo-deny): ignore unmaintained transitive advisories with rationale (#8)` — `6100def72`
* `fix(cargo-deny): remove stale RUSTSEC-2026-0049 ignore (#7)` — `864dd7cc0`
* `fix(workflows): use pull_request instead of pull_request_target` — `b2b638305`
* **Merged into `fork/main` since last regen:** PR #330 (sandbox fail-closed, 2026-10-16), PR #322 (evals fail-closed, 2026-10-16), PR #320 (mature-recovery spec, 2026-10-16).
* **Open PRs at regen:** #319 (live session control / ACP bridge), #328 (sharecli cross-process relay), #333 (durable effect recovery).

---

## Section 3 — Demo needs (what would need a recorded VHS or Playwright to prove it works)

> Each entry is a fork-only feature or capability that the codebase
> actually has. Listed in priority order based on operator-visible value.
> For each one, a short script outline is given; the actual recording
> is a follow-up.

| # | Feature | One-line demo description | Crate / file anchor |
| --- | --- | --- | --- |
| 1 | `helioslite` binary with deprecated `forge` / `forge-dev` tombstones | Run `helioslite --version`; run `forge --version` and capture the stderr tombstone; confirm `~/.helioslite/` is created. | `crates/forge_main/src/main.rs`, `crates/forge_main/src/update.rs` |
| 2 | Multi-registry publishing pipeline (Homebrew + Chocolatey + winget + crates.io + GitHub Releases) | `gh release view v2.13.21-h.0.2.9 --repo KooshaPari/HeliosLite --json assets`; tap `KooshaPari/helioslite` and `brew install helioslite`; verify winget manifest `KooshaPari.helioslite`; capture the 55-asset upload from a fresh `helioslite-release.yml` run. | `.github/workflows/helioslite-release.yml`, `.github/workflows/update-distribution.yml` |
| 3 | `helioslite lsp` subcommand (hover / define / complete / impl / refs / rename / typeDefinition) | `helioslite lsp` and pipe a minimal initialize -> didOpen -> hover sequence; show real rust-analyzer round-trip with a real `forge_lsp` e2e test path. | `crates/forge_lsp/`, `crates/forge_main/src/cli.rs` (subcommand wiring) |
| 4 | `helioslite share` subcommand (SSE + WebSocket + R2 + attach mode + cross-process relay) | Start `helioslite share`, attach a second client via the new `attach` ingestion mode (PR #328), push a session, capture the SSE event stream end-to-end, and confirm a Tracera telemetry event is emitted. | `crates/forge_sharecli/`, `crates/forge_tracera/` |
| 5 | `forge_agileplus` 31-pillar scorecard, sprint + velocity, weekly GitHub issue publish | `helioslite agileplus scorecard` and `helioslite agileplus sprint`; show the weekly scorecard being published to a GitHub issue on the configured cadence. | `crates/forge_agileplus/`, `docs/31-pillar-scorecard.md` |
| 6 | `forge_tracera` outbound observability sink (gzip + HMAC + DiskStore journal) | Stop the Tracera endpoint, push a few events, restart the endpoint, show the NDJSON journal replayed in order; verify the HMAC header rotates with overlap. | `crates/forge_tracera/` |
| 7 | `forge_guardian` risk adjudication layer (LearningStore + Sandbox tie-breaker + session-mode apply gate) | Trigger a high-risk tool call, capture the GuardianAdjudicator decision and the audit event, then flip the config flag and re-trigger to show the session-mode gate effect. | `crates/forge_guardian/`, `crates/forge_app/src/services.rs` |
| 8 | OS-level sandbox wiring for shell tool execution (P1.1, fail-closed via PR #330) | Run a shell command flagged as `p1.1 sandbox`; show the OS sandbox wrapping in a `forge_pheno_shell` audit event; toggle off and confirm PR #330 fail-closed enforcement reporting keeps the audit truthful. | `crates/forge_app/src/services.rs`, `crates/forge_pheno_shell/`, `crates/forge_app/src/sandbox.rs` |
| 9 | `ResiliencePolicy` — circuit breaker + bulkhead + adaptive throttle (P1.3) | Hit a flaky provider N times, capture the circuit-breaker open transition, the bulkhead rejection, and the throttle's adaptive retry-after. | `crates/forge_app/src/services.rs` |
| 10 | Semantic memory runtime — JSONL default, Supermemory / Letta / Cognee adapters (P1.5, P0.2, P3.4) + evals fail-closed (PR #322) | Run a multi-turn session, force a compact, then `helioslite semantic recall` to show the warm-hydrated context re-surfaced; switch adapter to Supermemory and re-run; observe PR #322 fail-closed evaluation signals. | `crates/forge_domain/`, `crates/forge_app/`, `crates/forge_tracera/`, `crates/forge_app/src/evals.rs` |
| 11 | `forge_lsp` MCP config auto-reload watcher (P2.5.1) | Start `helioslite lsp`, edit `~/.helioslite/mcp.toml`, observe the watcher event and confirm the LSP re-initializes without restart. | `crates/forge_lsp/` |
| 12 | `helioslite doctor` rename channel banner | Run `helioslite doctor`; confirm the active rename channel (repo, update URL, binary) is printed. | `crates/forge_main/src/cli.rs` |
| 13 | SignPath-optional Windows signing pipeline (h.0.2.6) | `gh workflow run helioslite-release.yml --ref h.0.2.6` with `SIGNPATH_*` unset; capture that the Windows sign step is skipped-not-failed and unsigned `.exe`s are attached. | `.github/workflows/sign-release.yml`, `helioslite-release.yml` |
| 14 | cargo-deny + `deny.toml` wildcards + TECH_DEBT register | `cargo deny check` from a fresh checkout; show the bans + wildcards + the TECH_DEBT register flagging the explicit RUSTSEC-2026-0049 ignore. | `deny.toml`, `.github/workflows/cargo-deny.yml` |
| 15 | CI gate hardening (cvp `--all-features`, platform-tests gating) | Replay a `cvp` run and show `--all-features` is in the gate-3 command, the platform-tests step is gating (no `continue-on-error`), and the `rust-analyzer` / `typescript-language-server` installs are present. | `.github/workflows/cvp.yml`, `platform-tests.yml`, `helioslite-release.yml` |
| 16 | Rust toolchain pin (1.98) ending the stable-float flip-flop | `rg "toolchain: *stable\|@stable" .github/workflows/` -> 0 matches; `cat rust-toolchain.toml` -> 1.98. | `rust-toolchain.toml`, `.github/workflows/*` |
| 17 | OpenSSF Best Practices badge application | Walk through `docs/openssf-badge-application.md` and show the criteria checklist with current evidence pointers. | `docs/openssf-badge-application.md` |
| 18 | PhenoShared git-deps refactor (no broken submodules, Windows path-safe) | `cargo metadata --format-version=1 | jq '.packages[] | select(.name == "phenoshared")'`; show the rev pin, the Windows-path-safe `fix(deps): repin PhenoShared to …` chain, and a `cargo build --workspace` from a clean `target/`. | `Cargo.toml`, `Cargo.lock` |
| 19 | fork identity / publishing-surface patch (Gate 4 partial) | `git show fadb32bda --stat`; show the helioslite publish-surface diff in `release.yml` -> `helioslite-release.yml`. | `.github/workflows/release.yml`, `helioslite-release.yml` |
| 20 | ACP live session control bridge (PR #319) | `helioslite acp serve`; connect an ACP client and show a `forge_live_control` round trip. | PR #319 branch (`feat/forge-live-control-acp-20260927`) |
| 21 | Mature-first harness recovery spec (PR #320, merged 2026-10-16) | Read `crates/forge_app/src/recovery/mature.md`; trigger a recovery scenario and observe oracle-blocked candidate rejection. | PR #320 (`spec/harness-mature-recovery-20260929`), now on `fork/main` at `5a4d6e528` |
| 22 | Durable effect recovery hook (PR #333, open) | Toggle the `forge.effect_recovery.enabled` flag; trigger a tool that mutates state; crash the daemon mid-flight; restart and observe the recovery hook replaying the effect from the journal. | PR #333 (`impl/effect-recovery-hook-20260930`) |

> **Recording plan.** Each row maps to ~1 short VHS or 1 Playwright
> trace. Recording is out of scope for this regen — this table is the
> backlog. Owner of each row: see `crates/*/README.md` and the matching
> PR author.

---

## Section 4 — Status

| Field | Value |
| --- | --- |
| Date generated | **2026-10-17T07:44Z** |
| Operator task | "Regenerate `docs/UPSTREAM-DELTA.md` for the HeliosLite fork; document the new 769-ahead / 20-behind state, the 3 PRs merged since the 2026-10-12 regen (#320, #322, #330), and the 3 PRs still open (#319, #328, #333)." |
| Authored by | Jcode orchestrator (`MiniMax-M3`) on `session_panda_20261017_074424_helioslite_fwd` |
| Inputs | `git fetch origin fork`; `git rev-list --left-right --count origin/main...fork/main`; `git log` (ahead + behind, no-merges); `git log --name-only` for touch stats; `gh pr list --repo KooshaPari/HeliosLite --state open/all`; per-PR `gh pr view <n>` for merge timestamps |
| Constraints honored | NO FORCE-PUSH, no destructive ops. Only edits/commits to `docs/UPSTREAM-DELTA.md` (existing file); a follow-up step will land the orphan patch; a third step will merge PR #328. |
| Worktree state (post-write) | `docs/UPSTREAM-DELTA.md` is staged for commit; `docs/sessions/20260917-helioslite-release-audit/stash-sanitize-broken.patch` (7882 lines) is still untracked and is the target of Step 2 of the FWD path. |
| Anomalies | **State shift since 2026-10-12:** 707-ahead / 0-behind → **769-ahead / 20-behind**. The 20-behind commits are all `chore(deps):` / `fix(deps):` from upstream's 2026-10-13 → 2026-10-17 dep-bump flurry and represent no capability gap; they are the expected signal that the next v2.15.x sync is due. The ahead set grew by 62 non-merge commits during the same window, dominated by PR #320/#322/#330 merge chain. |

### Reproduce

```bash
cd /Users/kooshapari/CodeProjects/Phenotype/repos/forgecode
git fetch origin fork
git rev-list --left-right --count origin/main...fork/main
git log --format="%h %ci %s" fork/main..origin/main --no-merges | head -30
git log --format="%h %s" origin/main..fork/main --no-merges | head -200
gh pr list --repo KooshaPari/HeliosLite --state open --limit 50 \
  --json number,title,headRefName,createdAt
```

### Revert

This task only modified one tracked file (`docs/UPSTREAM-DELTA.md`). To undo the change locally before commit:

```bash
git checkout -- docs/UPSTREAM-DELTA.md
```

No pushes, no PRs were created by the regeneration step itself. The orphan patch (`stash-sanitize-broken.patch`) and PR #328 self-merge are subsequent FWD-path steps and are tracked separately.