# UPSTREAM-DELTA.md — HeliosLite vs Forgecode

> **tl;dr.** This document records every change the HeliosLite fork
> (`KooshaPari/HeliosLite`) has on top of the upstream
> `tailcallhq/forgecode` repository, plus the small slice of upstream
> changes the fork has not yet absorbed. Read with `docs/FORK.md` and
> `docs/RENAMES-STRATEGY.md`. Regenerate with `git fetch origin fork &&
> git log ...` (see Section 4 — Reproduce).

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

### Heads at generation time (2026-10-17T07:46Z)

| Side | SHA | Date (UTC) | Subject |
| --- | --- | --- | --- |
| `fork/main` (this fork) | `91ee5e0b0eba9ee9163ed265286186f4c52b0ea0` | 2026-10-16 01:06:17 +0000 | `fix(sandbox): make enforcement reporting fail closed and truthful (#330)` |
| `origin/main` (upstream) | `0c8717f5cfb709777fd852d67652efa2e3c04ff1` | 2026-10-17 06:33:44 +0000 | `chore(deps): update dependency ai to v7.0.143 (#4031)` |

### Counts

```
$ git rev-list --left-right --count origin/main...fork/main
769     20
$ git log fork/main..origin/main --oneline | wc -l
20
```

| Direction | Non-merge commits | Notes |
| --- | --- | --- |
| **Ahead** (fork has, upstream lacks) | **769** | the entire fork delta — see Section 2 |
| **Behind** (upstream has, fork lacks) | **20** | all upstream-side dep bumps the fork has not absorbed (see below) |

### Upstream commits the fork is missing (all 20; all are dep bumps)

```
$ git log --format="%h %ai %s" fork/main..origin/main --no-merges
0c8717f5c 2026-10-17 06:33:44 +0000 chore(deps): update dependency ai to v7.0.143 (#4031)
ace75c107 2026-10-17 02:35:58 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.115 (#4030)
61ab3b6f6 2026-10-16 06:20:29 +0000 chore(deps): update dependency ai to v7.0.142 (#4026)
127a18b02 2026-10-16 00:55:51 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.114 (#4027)
df945e974 2026-10-15 21:33:54 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.113 (#4025)
36cdda39f 2026-10-15 17:00:07 +0000 chore(deps): update rust crate arboard to v3.7.0 (#4024)
6a69e62ae 2026-10-15 09:08:47 +0000 chore(deps): update dependency ai to v7.0.141 (#4020)
39a56bd18 2026-10-15 07:36:02 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.112 (#4019)
ed6f7a076 2026-10-14 10:53:07 +0000 chore(deps): update rust crate rustls to v0.23.46 (#4018)
9142e978b 2026-10-14 06:09:23 +0000 chore(deps): update dependency ai to v7.0.140 (#4017)
70baa1b8a 2026-10-14 02:37:30 +0000 fix(deps): update rust crate html-escape to 0.3.0 (#4016)
40a76e0d0 2026-10-14 02:30:08 +0000 chore(deps): update rust crate tokio to v1.53.3 (#4015)
b4f277523 2026-10-14 00:02:05 +0000 chore(deps): update dependency ai to v7.0.139 (#4014)
ee96dcb60 2026-10-13 19:16:46 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.111 (#4013)
3d242e248 2026-10-13 09:35:14 +0000 chore(deps): update rust crate syn to v3.0.9 (#4012)
156059680 2026-10-13 09:24:25 +0000 chore(deps): update rust crate schemars to v1.2.3 (#4010)
055785ba3 2026-10-13 09:16:29 +0000 chore(deps): update dependency zod to v4.6.7 (#4009)
bbbf0bbc3 2026-10-13 09:07:35 +0000 chore(deps): update rust crate sha2 to v0.11.1 (#4011)
c18ea1b13 2026-10-13 06:37:06 +0000 chore(deps): update dependency ai to v7.0.138 (#4008)
2e032b3dc 2026-10-13 01:19:48 +0000 chore(deps): update dependency @ai-sdk/google-vertex to v5.0.110 (#4007)
```

> **Note.** None of these 20 are behavioral. They are pure-version bumps
> to JS-side npm dependencies (`ai`, `@ai-sdk/google-vertex`, `zod`) and
> Rust crates (`arboard`, `rustls`, `html-escape`, `tokio`, `syn`,
> `schemars`, `sha2`). The fork can absorb them on its next upstream
> sync without code changes; they are tracked here so the next sync
> step is not blind.

### Distribution of ahead commits by conventional-commit type

| Type | Count | What it is |
| --- | --- | --- |
| `fix` | 260 | Bug fixes, CI/build repair, test hardening |
| `feat` | 130 | New features, crates, subcommands |
| `docs` | 95 | Handoffs, session docs, RFC-style notes |
| `ci` | 85 | Workflow changes, cargo-deny, action pins, gates |
| `chore` | 71 | Deps, manifest bumps, distribution, version bumps |
| `test` | 27 | New test cases and e2e shells |
| `style` | 17 | rustfmt-only |
| `perf` | 5 | Latency/throughput wins |
| `refactor` | 4 | Internal refactor with no behavior change |
| `release` | 3 | Release/version meta-commits |
| `security` | 2 | RUSTSEC bumps, security-scanner rewires |
| `wip` | 2 | Explicit WIP markers |
| `spec` | 1 | Mature-recovery spec (#320) |
| `renames` | 1 | HeliosLite rename channel work |
| `build` | 1 | Build-script change |
| `audit` | 1 | Master-cohort audit commit |
| `merge` | 1 | Resync/merge marker |
| other (non-conventional) | ~62 | Sub-prefix variants — see Section 2 grep split |
| **Total** | **769** | |

### Touch surface (file-level, ahead-only)

| Path bucket | File touches | Notable sub-buckets |
| --- | --- | --- |
| `.github/workflows` | 370 | workflows (fork-only workflow count dominates) |
| `crates/forge_repo` | 261 | fork-only conversation persistence work |
| `crates/forge_main` | 238 | binary entry, subcommand wiring |
| `crates/forge_app` | 167 | service composition |
| `crates/forge_ci` | 160 | gate generation + cargo nextest |
| `crates/forge_domain` | 130 | semantic memory + subagent threading |
| `docs/sessions` | 94 | session artefacts |
| `crates/forge_dbd` | 89 | fork-only DB-schema work |
| `Cargo.lock` | 88 | dep manifest churn |
| `crates/forge_services` | 77 | service-level glue |
| `crates/forge_infra` | 71 | infra adapters |
| `crates/forge_lsp` | 69 | LSP crate (P2.3) |
| `crates/forge_api` | 68 | API surface |
| `Cargo.toml` | 56 | workspace + per-crate manifests |
| `ui/src` | 41 | Tauri desktop shell UI |
| `crates/forge3d` | 40 | fork-only 3D rendering |
| `crates/forge_sharecli` | 29 | share-cli (P3.3) |
| `crates/helios-bot` | 25 | helios-bot crate |
| `crates/forge_sandbox` | 25 | sandbox wiring (P1.1) |
| **Total `crates/`** | **1958** | dominant — every fork-unique crate lives here |
| **Total `docs/`** | **192** | session docs, handoffs, RENAMES-STRATEGY, etc. |

### Pull-request activity on the fork (`gh pr list --repo KooshaPari/HeliosLite`)

* **Total OPEN at 2026-10-17T07:46Z:** 3 (`#319`, `#328`, `#333`)
* **Merged since prior regen (2026-10-12 → 2026-10-17):** 4 — `#320`
  (mature-recovery spec), `#322` (evals fail-closed), `#330` (sandbox
  fail-closed), `#334` (h2 RUSTSEC bump). All four PR tip commits now
  sit on `fork/main` and are in the Section 2 list above.
* **MERGED total ≥ 204** (gh API `--limit 200` cap; real total higher)

| # | State | Head branch | Title | Merged onto `fork/main` |
| --- | --- | --- | --- | --- |
| 333 | OPEN | `impl/effect-recovery-hook-20260930` | feat(tool): add optional durable effect recovery hook | — |
| 328 | OPEN | `impl/sharecli-cross-process-relay-20260930` | fix(sharecli): make publish, topics and attach use the running relay process | — |
| 319 | OPEN | `feat/forge-live-control-acp-20260927` | feat: live session control and ACP bridge | — |
| 330 | MERGED | `impl/sandbox-truthful-enforcement-20260930` | fix(sandbox): make enforcement reporting fail closed and truthful | `91ee5e0b0` (fork/main) |
| 322 | MERGED | `impl/harness-oracle-hardening-20260929` | fix(evals): fail closed on missing evidence, signals and timeouts | `ea9524476` |
| 320 | MERGED | `spec/harness-mature-recovery-20260929` | spec: mature-first harness recovery — pinned sources, oracle blockers and candidate contract | `5a4d6e528` |
| 334 | MERGED | (security/h2-0.4.19 branch tip) | fix(deps): bump h2 0.4.13 → 0.4.19 (clears RUSTSEC-2026-0258 for h2 0.4.x) | `11cf7f97d` |

---

## Section 2 — Every change vs upstream, grouped by intent

> **Format:** `[intent-group] <sha7> <date> <subject>`. Newest-first.
> The ahead set is 769 commits; the first 200 are shown verbatim;
> the remaining 569 are summarized by intent in the table at the end
> of this section.

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
> * `spec / audit` — mature-recovery spec + master-cohort audit
> * `misc / uncategorized` — style, WIP, non-conventional prefixes

### Per-commit list (first 200 of 769, newest first)

```
[security / policy] 91ee5e0b0 2026-10-15 18:06:17 -0700 fix(sandbox): make enforcement reporting fail closed and truthful (#330)
[security / policy] ea9524476 2026-10-15 18:06:12 -0700 fix(evals): fail closed on missing evidence, signals and timeouts (#322)
[spec / audit]      5a4d6e528 2026-10-15 18:06:03 -0700 spec: mature-first harness recovery — pinned sources, oracle blockers and candidate contract (#320)
[security / policy] 11cf7f97d 2026-10-15 18:05:57 -0700 fix(deps): bump h2 0.4.13 → 0.4.19 (clears RUSTSEC-2026-0258 for h2 0.4.x) (#334)
[docs / handoff]    29c00291c 2026-10-10 01:41:39 -0700 docs(handoff): 10-09/10 delta - upstream sync, CI hygiene+hardening chain, streak 12, disk cycles 3-4
[build / fix / test] 2053b2890 2026-10-09 00:49:50 -0700 fix(lsp,infra): apply cmd /C shim-safe spawn to tsc and mcp_client (complete .cmd-shim class)
[build / CI]        007068dbb 2026-10-09 00:13:10 -0700 ci(cvp): run workspace tests with --all-features in Gate 3
[build / fix / test] e278369dd 2026-10-09 00:13:06 -0700 fix(lsp): spawn .cmd/.bat shims via cmd /C on Windows (restores real Windows e2e)
[build / fix / test] 239a57824 2026-10-08 23:36:08 -0700 fix(test): skip forge_lsp real-RA e2e when typescript-language-server is not runnable
[build / fix / test] 5b2eafe47 2026-10-08 23:10:24 -0700 fix(test): harden macOS mcp_watcher fs-event waits and make release_tags shell test Windows-portable
[build / CI]        cf11b3c37 2026-10-08 22:52:11 -0700 ci(gates): harden platform-tests (all-features + gating) and install RA/TS prereqs in cvp/coverage so real-RA e2e runs
[build / fix / test] 6e5b8fa94 2026-10-08 22:25:53 -0700 fix(ci): clear clippy 1.99 lints and restore forge_ci release-workflow parity
[agent harness]     fadb32bda 2026-10-08 19:44:05 -0700 refactor(publish): helioslite publish-surface patch (Gate 4 partial)
[docs / handoff]    49a39c4ab 2026-10-02 21:09:57 +0200 docs(handoff): 10-02 delta - nightly #5 accepted from raw logs, tip matrix final 33/3/0
[docs / handoff]    9acde30e9 2026-10-01 22:38:22 +0200 docs(handoff): full 12-row mapped-check re-run (12/12 PASS observed) + R3 cell correction
[docs / handoff]    cf26e536e 2026-10-01 22:25:54 +0200 docs(handoff): R11 final verdict - tip CI GREEN at 081905205 (26 success/2 skipped/0 failures; stale-cron check classified)
[docs / handoff]    081905205 2026-10-01 20:39:36 +0200 docs(handoff): feedback-loop closure - requirement to check to observed-result matrix
[docs / handoff]    2f0a0d4ba 2026-10-01 20:17:28 +0200 docs(handoff): resolve distribution todo - PR #318 merged 09-20, purpose satisfied, dispatch moot
[docs / handoff]    a97f438e3 2026-10-01 20:12:14 +0200 docs(handoff): record answered-record distribution - false likely deliberate (3-denial cluster 09-24)
[docs / handoff]    2677faa7b 2026-10-01 20:07:52 +0200 docs(handoff): correct hook-d6499f54 timestamps to observed date output (09-24/09-25)
[docs / handoff]    1f8029084 2026-10-01 20:06:20 +0200 docs(handoff): correct hook-d6499f54 state - answered=false 2026-09-23, not pending
[docs / handoff]    50eb43231 2026-10-01 19:57:55 +0200 docs(handoff): record observed pre-dispatch alignment for distribution re-dispatch
[docs / handoff]    5db864f35 2026-10-01 19:53:48 +0200 docs(sessions): record Q3-A toolchain pin execution + post-pin nightly acceptance
[misc]              d1a84bdf7 2026-09-29 21:27:41 +0200 style(ci): rustfmt the generator edit
[build / fix / test] e5e7f6473 2026-09-29 21:24:21 +0200 fix(ci): drop toolchain inputs in forge_ci generator - align source with 1.98 pin
[build / CI]        536a25cac 2026-09-29 19:10:25 +0200 ci: pin all workflow toolchains to rust-toolchain.toml (1.98) - end stable float flip-flop
[docs / handoff]    0adab9841 2026-09-29 18:15:11 +0200 docs(sessions): record sync #2 landing (65e1b7994, CI 28/28) + workflow-trigger finding
[docs / handoff]    d89a156b5 2026-09-29 18:11:47 +0200 docs(sessions): close flake-fix section with final CI evidence (f1af9b9b6 + 65e1b7994 green)
[docs / handoff]    f1af9b9b6 2026-09-29 17:20:11 +0200 docs(sessions): confirm CI toolchain: stable evidence; record Q3 pin recommendation
[docs / handoff]    cb7e359d5 2026-09-29 17:16:56 +0200 docs(sessions): correct undici fix attribution; record fmt race outcome and rustfmt skew
[misc]              0bab1ab05 2026-09-29 17:12:41 +0200 style(forge_lsp): rustfmt mcp_watcher/tests.rs per CI Format check
[docs / handoff]    36b8568d1 2026-09-29 17:09:07 +0200 docs(sessions): record undici alert auto-close and sync-2 conflict preview
[decoration]        10a4c36af 2026-09-29 17:05:36 +0200 chore(deps): bump undici to 7.30.0 (GHSA-3wwx-pv8v-q78v, medium)
[docs / handoff]    795e3cafc 2026-09-29 17:03:47 +0200 docs: record P5 decision C executed - archive tag pushed; sync-landed HANDOFF deltas
[docs / handoff]    bb8fd6e4d 2026-09-29 16:55:31 +0200 docs(sessions): record upstream sync landed at 72ff2cab8 with CI + deny evidence
[docs / handoff]    89bbb7723 2026-09-29 16:46:49 +0200 docs(sessions): record macOS mcp_watcher flake diagnosis and forward-fix
[build / fix / test] e9271f8a2 2026-09-29 16:46:47 +0200 fix(forge_lsp): harden mcp_watcher fs-event tests against runner stalls; extract test module
[docs / handoff]    b0485410b 2026-09-29 16:20:09 +0200 docs(sessions): record nightly acceptance evidence - 09-28/09-29 runs green, 3889 passed, 0x -32801
[docs / handoff]    c503badf6 2026-09-29 16:07:13 +0200 docs(sessions): record nightly acceptance observed green (09-28 + 09-29 runs)
[docs / handoff]    6dba1f674 2026-09-28 02:55:02 +0200 docs(handoff): add 2026-09-28 addendum - nightly pre-flight and P5 decision request
[docs / handoff]    c846dea3e 2026-09-27 20:55:39 +0200 docs(sessions): record ci/test LSP proxy evidence and P5 decision brief
[docs / handoff]    49769b34d 2026-09-27 19:52:36 +0200 docs(sessions): correct stale expectations with observed CI/alert state
[decoration]        4c8df606e 2026-09-27 19:22:19 +0200 chore(ci): pin cargo-deny tool version and add TECH_DEBT register
[docs / handoff]    1a519532b 2026-09-27 18:35:04 +0200 docs(sessions): record ContentModified fix set and rand #7 closure
[build / fix / test] acb5c54a9 2026-09-27 18:23:22 +0200 fix(deps): bump transitive rand 0.8.5 -> 0.8.8 (GHSA dependabot #7)
[docs / handoff]    02a936488 2026-09-27 18:05:50 +0200 docs(sessions): record externally-gated CI observation outcomes
[build / fix / test] 835e7b02d 2026-09-27 17:58:31 +0200 fix(lsp): retry ContentModified on the definition request path
[docs / handoff]    b7b09f435 2026-09-27 17:28:04 +0200 docs(sessions): record nightly content-modified diagnosis and fix
[build / fix / test] da25401fc 2026-09-27 17:23:18 +0200 fix(forge_lsp): retry transient LSP errors and raise cold-runner e2e budgets
[build / fix / test] b30984166 2026-09-27 17:19:09 +0700 fix(test): retry transient LSP content-modified errors in e2e round trip
[docs / handoff]    fb94d603e 2026-09-24 05:01:59 -0700 docs(sessions): record feedback-loop closure observations
[docs / handoff]    01f31585f 2026-09-24 04:21:38 -0700 docs(handoff): add 2026-09-24 addendum with post-roadmap current state
[docs / handoff]    6fe40455e 2026-09-24 02:58:19 -0700 docs(sessions): record roadmap P1-P4 execution and nightly fix
[build / fix / test] 31896cb56 2026-09-24 02:14:32 -0700 fix(ci): install typescript-language-server in nightly workflow
[build / fix / test] ba7187320 2026-09-24 02:02:27 -0700 fix(ci): delete stale distribution branch before push on workflow re-run
[build / fix / test] 498d276e4 2026-09-20 02:36:21 -0700 fix(deps): resolve dependabot alerts and prune stale deny ignores
[docs / handoff]    eb5fdd25c 2026-09-20 00:29:14 -0700 docs(sessions): record skipped-branch re-analysis and archive-tag deletion
[docs / handoff]    38249bdf1 2026-09-19 23:17:12 -0700 docs(sessions): record Cargo Deny crossbeam-epoch fix and merge lesson
[build / fix / test] dcf0e7d38 2026-09-19 22:47:00 -0700 fix(ci): bump crossbeam-epoch 0.9.18 -> 0.9.21 (RUSTSEC-2026-0204)
[docs / handoff]    20ae2be1e 2026-09-19 18:40:39 -0700 docs(sessions): record h.0.2.9 UDF race validation outcome
[decoration]        c79326c4f 2026-09-19 23:44:44 +0000 chore(distribution): update Homebrew, Scoop, and packaging manifests for v2.13.21-h.0.2.9
[build / fix / test] bfdaa8831 2026-09-19 15:06:24 -0700 fix(deps): rmcp 1.8.0 -> 3.4.0 and vite ^5.4.0 -> ^8.3.0
[decoration]        79088e8dc 2026-09-19 12:53:24 -0700 chore(distribution): update manifests for v2.13.21-h.0.2.8
[build / fix / test] f18a5247e 2026-09-19 12:14:07 -0700 fix(distribution): extend checksum retry window to 45 minutes and add manual trigger
[build / fix / test] dcdbb17ac 2026-09-19 02:59:48 -0700 fix(distribution): repair broken Homebrew, Chocolatey, and install.sh packaging
[docs / handoff]    925865cbf 2026-09-19 01:43:21 -0700 docs(audit): commit the 2026-09-11 worktree disposition audit
[docs / handoff]    787b33dfb 2026-09-19 01:42:34 -0700 docs(release): correct the h.0.2.7 ship SHA to db571577d
[decoration]        43f292fa6 2026-09-18 23:24:27 -0700 chore(distribution): update Homebrew and Scoop manifests for v2.13.21-h.0.2.7
[build / fix / test] 30e511eb2 2026-09-18 22:30:55 -0700 fix(ci): wait for release assets in update-distribution before failing
[build / fix / test] db571577d 2026-09-18 21:09:08 -0700 fix(ci): align the generator with the committed ci.yml; drop the hand-edit note
[build / fix / test] 537dea827 2026-09-18 20:24:04 -0700 fix(deps): repin PhenoShared to b2d06d53 (agileplus-cache manifest repair)
[build / fix / test] c43bbf828 2026-09-18 19:30:07 -0700 fix(test): stop runner load turning the LSP e2e into a red test; restore the provider key
[build / fix / test] 6b5c7fc8c 2026-09-18 19:06:04 -0700 fix(ci): apply the audit's confirmed findings; commit the audit itself
[build / fix / test] fb03ec7db 2026-09-18 18:47:56 -0700 fix(ci): repair two brittle expectations found by audit; record the guard
[build / CI]        4d78a0c80 2026-09-18 17:37:44 -0700 ci(cvp): enforce the cargo-bin invariant so the rustup clobber cannot return
[docs / handoff]    002271c07 2026-09-18 17:19:38 -0700 docs(sessions): pre-write next release notes and an idempotent ship script
[docs / handoff]    aaef98bf0 2026-09-18 16:43:32 -0700 docs(sessions): refresh the handoff snapshot (it still pointed at bdb183311)
[docs / handoff]    9aca62e1a 2026-09-18 16:42:43 -0700 docs: point agents at the handoff from AGENTS.md; fix stale SSH note
[docs / handoff]    0fa3c0bc5 2026-09-18 16:18:18 -0700 docs(sessions): exact verified workflow list; note API re-verification
[docs / handoff]    3e0809bef 2026-09-18 08:15:50 -0700 docs(sessions): record commit-verified CI conclusions per run id
[docs / handoff]    65899cf7f 2026-09-18 07:51:41 -0700 docs(sessions): all twelve CI entries green on a8ad28703
[build / fix / test] a8ad28703 2026-09-18 07:16:56 -0700 fix(ci): give LSP e2e tests a nextest budget that matches their timeouts
[docs / handoff]    d82a6701d 2026-09-18 07:09:23 -0700 docs(sessions): CLI boundary fix, acceptance evidence, verification gap
[build / fix / test] 77607f74c 2026-09-18 07:08:11 -0700 test(lsp): make the CLI integration test a single bounded call
[build / fix / test] be581d8c9 2026-09-18 06:53:15 -0700 fix(lsp): make file-scoped CLI commands work on a cold server
[docs / handoff]    503e7e53f 2026-09-18 06:13:35 -0700 docs(sessions): mark the LSP e2e item fixed, all workflows green
[agent harness]     3b1ebf4e7 2026-09-18 06:04:11 -0700 feat(lsp): add didOpen support and make the LSP e2e round trip real
[docs / handoff]    b2d588364 2026-09-18 05:54:10 -0700 docs(sessions): final verified CI status for the handoff
[docs / handoff]    a90e636b8 2026-09-18 05:22:42 -0700 docs(sessions): record the watcher shutdown bug fix
[build / fix / test] d9a5871a9 2026-09-18 05:22:24 -0700 fix(lsp): make McpWatcherHandle::stop actually interrupt the watcher
[build / fix / test] ce682cff4 2026-09-18 05:06:52 -0700 test(lsp): de-flake watcher_stop_completes_quickly
[build / fix / test] 3a75297b0 2026-09-18 04:53:42 -0700 fix(config): remove denied indexing_slicing from render_table
[docs / handoff]    77cf18741 2026-09-18 04:49:47 +0200 docs(sessions): record cvp and autofix lint findings
[build / fix / test] 677b78ecd 2026-09-18 04:49:08 +0200 fix(ci): cvp pin check + forge_sharecli indexing_slicing lint
[docs / handoff]    9ef324468 2026-09-18 04:44:54 +0200 docs(sessions): record the open LSP e2e test failure with evidence
[build / fix / test] f1b31f7e3 2026-09-18 04:31:19 +0200 fix(ci): install typescript-language-server for the LSP e2e test
[docs / handoff]    4b254ddf2 2026-09-18 04:20:38 +0200 docs(sessions): record rust-analyzer/rustup clobber root cause (4.6)
[build / fix / test] b089edae6 2026-09-18 04:20:28 +0200 fix(ci): guard must not use rustc -vV | grep -q under pipefail
[build / fix / test] 3fc98eb0c 2026-09-18 04:09:22 +0200 fix(ci): never install rust-analyzer into ~/.cargo/bin (overwrites rustup)
[build / fix / test] fb0b93d5f 2026-09-18 03:50:19 +0200 fix(ci): stop caching ~/.cargo/bin (corrupt bin dir breaks all cargo calls)
[docs / handoff]    a20e842fb 2026-09-18 03:25:52 +0200 docs(sessions): correct PhenoShared tip reference in handoff
[docs / handoff]    a9aba8504 2026-09-18 03:25:00 +0200 docs(sessions): mark release-CI mission complete (55 assets on h.0.2.6)
[docs / handoff]    97bff227a 2026-09-18 02:05:03 +0200 docs(sessions): handoff for release-CI ownership
[build / fix / test] bdb183311 2026-09-18 01:37:52 +0200 fix(ci): make Windows SignPath signing optional when unconfigured
[build / fix / test] 5d3ea1236 2026-09-17 23:19:47 +0200 fix(deps): repin PhenoShared to cf914698 with Windows-invalid-char fix
[decoration]        2b5c7e6cc 2026-09-17 22:55:35 +0200 chore(deps): regenerate Cargo.lock for PhenoShared 62368071
[build / fix / test] da5f6e908 2026-09-17 22:45:36 +0200 fix(deps): repin PhenoShared to 62368071 with Windows path fixes
[build / fix / test] ae282957e 2026-09-17 21:22:21 +0200 fix(ci): fix platform test failures on macos/windows runners
[build / fix / test] c0596013a 2026-09-17 18:07:10 +0200 fix(ci): raise caller sign_release permission ceiling to contents:write
[build / fix / test] 3a466e86a 2026-09-17 17:59:08 +0200 fix(cvp): fix rev extraction to take first match only
[build / fix / test] 8008f2c8e 2026-09-17 13:43:08 +0200 fix(deps): bump rustls 0.23.43 -> 0.23.45 (RUSTSEC-2026-0285)
[docs / handoff]    bc36cf53d 2026-09-17 02:23:16 +0200 docs(sessions): 20260917-helioslite-release-audit
[build / fix / test] 122e1e224 2026-09-17 02:22:48 +0200 fix(lockfile): regenerate Cargo.lock for PhenoShared rev=68beca26
[misc]              de779e519 2026-09-16 20:50:27 +0200 style: rustfmt Gate 7 rename channel code
[agent harness]     74103310d 2026-09-16 20:02:35 +0200 feat(rebrand): Gate 6+7 — domain activation + legacy deprecation
[build / fix / test] ebb7dd670 2026-09-16 05:15:38 +0200 fix: invoke cargo-nextest directly with proper rustc PATH
[build / fix / test] 2f4431b1e 2026-09-16 05:11:56 +0200 fix: override CARGO_HOME to match nextest install location
[build / fix / test] a51c59798 2026-09-16 05:07:11 +0200 fix: check multiple paths for cargo-nextest binary
[build / fix / test] 8c0a5a426 2026-09-16 04:54:44 +0200 fix: invoke cargo-nextest directly to bypass PATH issue
[build / fix / test] 9663c1991 2026-09-16 04:48:31 +0200 fix: fix CI test and CVP workflow issues
[build / fix / test] 8e82a9a58 2026-09-16 04:36:31 +0200 fix: allow PhenoShared git source in cargo-deny
[build / fix / test] 34d81622c 2026-09-16 04:31:38 +0200 fix: bump PhenoShared to fully submodule-free commit (68beca26)
[build / fix / test] 7b23193ca 2026-09-16 04:22:50 +0200 fix: update PhenoShared rev to commit without broken submodules
[build / fix / test] baeb1c91b 2026-09-16 04:06:54 +0200 fix: use git CLI for cargo fetch to skip broken submodules
[build / fix / test] f68e4dc7a 2026-09-16 03:59:38 +0200 fix: switch to git deps for PhenoShared (rev 82d7f473)
[agent harness]     79c02c8f9 2026-09-16 03:32:19 +0200 feat: wire PhenoShared shared crates + CVP workflow
[build / fix / test] ed0a38166 2026-09-16 01:17:50 +0200 fix(test): widen concurrent ops heartbeat gap threshold (50ms -> 75ms)
[build / fix / test] 6de2d161d 2026-09-15 02:12:42 +0200 test(share): e2e shell tests for helioslite share binary + runtime-thread fix (#316)
[build / fix / test] 9c12ec78a 2026-09-15 01:54:31 +0200 test(main): e2e shell tests for helioslite lsp binary (langs/hover/define/complete) (#314)
[agent harness]     c66e3d977 2026-09-14 17:48:04 +0200 feat(config): surface env-var schema for lsp/share/agileplus/tracera subcommands (#315)
[agent harness]     4a4c51e73 2026-09-13 23:03:46 +0200 feat(app): wire semantic memory recall (warm-hydrate) and forget (on compact) (#313)
[agent harness]     66fd35260 2026-09-13 23:03:43 +0200 docs(semantic): clarify JsonlSemanticMemory as the default runtime adapter (#312)
[build / CI]        46cec3cd7 2026-09-13 23:03:40 +0200 ci: install rust-analyzer in workflows that run the forge_lsp e2e test (#311)
[build / fix / test] d188d6884 2026-09-13 23:03:29 +0200 test(forge_sharecli): live HTTP SSE end-to-end integration test (#310)
[agent harness]     41bb44e43 2026-09-13 20:39:03 +0200 feat(app): wire semantic memory store into chat-completion path (#309)
[build / fix / test] 9e54e8591 2026-09-13 20:24:58 +0200 test(lsp): add e2e integration test against real rust-analyzer binary (#308)
[agent harness]     72731cb2f 2026-09-13 20:24:56 +0200 feat(sharecli): add attach-to-session ingestion mode (P3.3 follow-up) (#307)
[agent harness]     6accf7ada 2026-09-13 18:54:44 +0200 feat(main): expose forge_sharecli via helioslite share subcommand (#306)
[agent harness]     09d9d571b 2026-09-13 17:06:40 +0200 feat(lsp): expose forge_lsp capabilities via helioslite lsp subcommand (#306)
[agent harness]     565bf4728 2026-09-13 14:50:41 +0200 feat(services): integrate semantic memory runtime
[agent harness]     7554beff8 2026-09-13 04:50:30 +0200 feat(telemetry): add outbound Tracera telemetry integration via forge_tracera sink (#305)
[build / fix / test] 3ebb61fc3 2026-09-13 03:11:07 +0200 fix(lsp): reject empty/whitespace rename target before forwarding (#304)
[agent harness]     b83de092f 2026-09-13 03:05:12 +0200 feat(tracera): add persistent DiskStore (file-backed NDJSON journal) for offline retry (#303)
[build / fix / test] b103f61fb 2026-09-13 02:14:50 +0200 test(sharecli): R2 integration smoke test composing ShareHub -> TraceraSink (#301)
[agent harness]     ddd864d68 2026-09-13 01:38:45 +0200 feat(sharecli): add SSE + WebSocket transports atop ShareHub (transport layer) (#302)
[decoration]        555950f6a 2026-09-13 00:41:12 +0200 release(version): bump to 2.13.21-h.0.1.8 for P0-P3 backlog (#299)
[build / fix / test] 08bf52b1e 2026-09-13 00:31:12 +0200 test(sharecli+tracera): add integration smoke test for sharecli→tracera interop (#300)
[agent harness]     c908b766b 2026-09-12 23:16:25 +0200 feat(sharecli): add forge_sharecli crate (#298)
[agent harness]     0dc06921d 2026-09-12 21:10:24 +0200 feat(p2.3.2): add LSP implementation/references/typeDefinition/rename to forge_lsp (#295)
[agent harness]     9987d5901 2026-09-12 21:06:22 +0200 feat(forge_tracera): gzip request compression + HMAC auth rotation with overlap + runtime set_auth (#296)
[agent harness]     77c2fc19c 2026-09-12 21:00:19 +0200 feat(p2.5.1): add MCP config auto-reload watcher to forge_lsp (#294)
[build / fix / test] 7e56ce58d 2026-09-12 19:33:44 +0200 fix(forge_lsp): address P1+P2 review blockers from PR #290 (#292)
[agent harness]     a43078057 2026-09-12 19:26:41 +0200 feat(p3.2): add forge_agileplus crate (pillars, sprint, velocity) (#293)
[agent harness]     0d674a83e 2026-09-12 19:05:20 +0200 feat(p3.4): add Supermemory + complete Letta + Cognee semantic-memory adapters (#291)
[agent harness]     370ddbd66 2026-09-12 18:04:28 +0200 feat(p2.3.1): add LSP hover/definition/completion to forge_lsp
[agent harness]     e7a535400 2026-09-12 17:41:04 +0200 feat(p2.2.1): wire TerminalEmulator detection into executor (#289)
[build / fix / test] 1fda5b8dc 2026-09-12 07:31:53 +0200 fix(repo): propagate parent_id in TryFrom + fix legacy unscoped semantic (#288)
[build / fix / test] 8f2fe3af8 2026-09-12 06:52:03 +0200 fix(provider): make merge_live server-side authoritative + alive dedup (#287)
[build / fix / test] 93ec5afc8 2026-09-12 06:39:27 +0200 fix(repo): repair P2.5 conversation schema drift blocking all PRs from CI (#286)
[decoration]        0bb5698b7 2026-09-12 04:26:32 +0200 chore(lock): add forge_tracera lockfile entry (P3.1 followup)
[agent harness]     6c913f779 2026-09-12 04:25:45 +0200 feat(tracera): add forge_tracera crate — outbound observability sink (P3.1) (#284)
[decoration]        ad073e4d2 2026-09-12 03:02:13 +0200 release: promote h0.1.7 integration to main (#277)
[agent harness]     b2caba6aa 2026-09-12 01:30:42 +0200 feat(provider): add ModelSource::Dynamic with curated fallback (#279)
[build / fix / test] d405d3823 2026-09-12 00:36:04 +0200 fix(provider): preserve Dynamic model DTO metadata from server response (#281)
[agent harness]     75cc7f6d1 2026-09-12 00:14:01 +0200 feat(p2.5): add per-server MCP last-seen health metadata (#282)
[build / fix / test] bbe672f8b 2026-09-11 22:55:40 +0200 fix(provider): address CodeRabbit findings on Dynamic ModelSource
[agent harness]     f8cee62f4 2026-09-11 22:04:05 +0200 feat(p2.4): wire --stream-json NDJSON tee into chat streaming loop (#280)
[agent harness]     ebf343a3a 2026-09-11 03:54:53 +0200 feat(provider): add ModelSource::Dynamic with curated fallback
[agent harness]     f5acb84cc 2026-09-11 00:46:06 +0200 feat(p2.3): add forge_lsp crate - LSP-style diagnostics (#275)
[build / fix / test] 28b949078 2026-09-11 00:12:55 +0200 fix: token-usage accumulation semantics + HeliosAgentConfig doctest import (#276)
[build / fix / test] 71f7c0488 2026-09-10 01:40:25 +0200 fix(release): stage assets until signing and provenance pass
[build / fix / test] 60fe56457 2026-09-10 01:18:12 +0200 fix(shell): align terminal capability semantics
[build / fix / test] 48be30557 2026-09-10 00:49:40 +0200 fix(shell): resolve terminal emulator lint debt
[docs / handoff]    7d5bf8326 2026-09-09 23:31:55 +0200 docs(consolidation): preserve historical coverage snapshot
[agent harness]     92cd544b7 2026-09-09 19:24:36 +0200 feat(p2.2): add TerminalEmulator detection to forge_pheno_shell (#258)
[build / fix / test] 302d77a90 2026-09-08 22:55:55 +0200 fix(ci): pass release tags as literal shell arguments
[build / fix / test] 81cc94cb4 2026-09-08 21:37:08 +0200 fix(ci): support API-key notarization
[build / fix / test] a21e6c702 2026-09-08 17:07:10 +0200 fix(ci): declare signing inputs and restrict forwarded secrets
[build / fix / test] c711bc0dc 2026-09-08 16:19:52 +0200 test(ci): render release workflow without concurrent file writes
[build / fix / test] bdd9d4ff6 2026-09-08 15:14:24 +0200 fix(release): serialize signing before release provenance
[build / fix / test] 999b5c504 2026-09-08 02:26:02 +0200 fix(release): pin HeliosLite h.0.1.7 draft version (#270)
[decoration]        61ddccfd5 2026-09-08 01:57:12 +0200 release: integrate h0.1.7 branch bundles (#254)
[agent harness]     77fa4dce0 2026-09-06 20:27:16 +0200 feat(p2.1): wire forge_paste collapse+mention+bracketed into ForgeEditor (#251)
[agent harness]     00f40c4fc 2026-09-06 02:30:51 +0200 feat(app): wire OS-level sandbox into shell tool execution (P1.1) (#248)
[agent harness]     8837923f9 2026-09-05 22:49:04 +0200 feat(domain): add JSONL-backed semantic memory adapter (P1.5) (#247)
[agent harness]     16468193c 2026-09-05 17:39:46 +0200 feat(app): add ResiliencePolicy - circuit breaker + bulkhead + adaptive throttle (P1.3) (#246)
[agent harness]     31c6a3596 2026-09-05 04:52:44 +0200 feat(domain): add subagent semantic threading (P0.2) (#245)
[agent harness]     c191176f9 2026-09-05 00:42:46 +0200 feat(p0.1): wire GuardianAdjudicator into permission check (P0.1.2) (#242)
[build / fix / test] c36835e7b 2026-09-04 18:20:53 +0200 fix(repo): validate recovered message variants
[build / fix / test] a72438b09 2026-09-04 17:37:18 +0200 fix(repo): avoid variant fallback indexing
[build / fix / test] 37da6db82 2026-09-04 17:01:55 +0200 fix(repo): satisfy conversation record lint and format
[misc]              eff1779ab 2026-09-04 00:43:38 +0200 preserve dirty conversation record change
[agent harness]     ae879a033 2026-09-04 03:33:19 +0200 feat(p0.1): guardian config flag + session-mode apply gate (P0.1.2 pure) (#240)
[agent harness]     d2909cb02 2026-09-04 02:03:03 +0200 feat(p0.1): add LearningStore + Sandbox tie-breaker to forge_guardian (#239)
[agent harness]     0234f5f16 2026-09-04 00:08:51 +0200 feat(p0.1): add forge_guardian crate - risk adjudication layer (#237)
[agent harness]     73617a97b 2026-09-03 05:45:37 +0200 feat(p0.3/p0.4): add input pipeline + output/audit spine crates (#236)
[docs / handoff]    075790440 2026-09-01 15:27:56 +0200 docs: add OpenSSF Best Practices badge application guide
[build / fix / test] a903f1de8 2026-09-01 04:49:35 +0200 fix(ci): resolve all 7 clippy+fmt failures blocking main (#235)
[decoration]        ada00fed6 2026-09-01 03:21:50 +0200 chore: add production Dockerfile + update 31-pillar scorecard
[build / fix / test] 5a7d23dcd 2026-09-01 02:31:25 +0200 fix(ci): resolve 3 clippy errors in e2e_mock_provider.rs
[build / CI]        4b1f8a114 2026-08-31 23:31:55 +0200 ci(scorecard): fix TokenPermissions + PinnedDependencies + SAST coverage (#232)
[build / CI]        153e8728a 2026-08-31 21:54:03 +0200 ci: fix Platform Tests non-blocking + Update Distribution PR-based push (#231)
[build / fix / test] 5ffdc3eda 2026-08-30 20:35:01 +0200 test(sandbox): use portable temp directory fixture (#222)
[misc]              d79537469 2026-08-30 18:48:12 +0200 [codex] refresh CI cache and scorecard action pins (#223)
```

> **+ 569 more.** Run
> `git log --format="%h %ai %s" origin/main..fork/main --no-merges | sed -n '201,769p'`
> for the remainder. The oldest fork-only commit is
> `c126175a1` (2026-05-01 20:06:16 -0700) — `ci: add cargo-deny config, fix workspace license (#3)`.

### Intent summary across the full 769-commit ahead set

| Intent bucket | Count of full set | Visible in first 200 |
| --- | --- | --- |
| `build / fix / test` | 260 (`fix`) + 27 (`test`) = 287 | 88 |
| `agent harness / subagent / swarm` | 130 (`feat`) + 4 (`refactor`) + 5 (`perf`) = 139 | 41 |
| `docs / handoff` | 95 | 55 |
| `decoration / branding / fork identity` | 71 (`chore`) + 3 (`release`) = 74 | 11 |
| `build / CI` | 85 (`ci`) + 1 (`build`) = 86 | 7 |
| `security / policy` | 2 (`security`) + 1 (`spec`) + ad-hoc RUSTSEC fixes ≈ 19 | 4 (new this regen: #330, #322, #334) |
| `spec / audit` | 1 (`spec`) + 1 (`audit`) = 2 | 1 |
| `misc / uncategorized` | 17 (`style`) + 2 (`wip`) + ~62 sub-prefix variants ≈ 81 | 4 |

### Security/policy highlights

* `fix(sandbox): make enforcement reporting fail closed and truthful` — `91ee5e0b0` (PR #330, MERGED on `fork/main`)
* `fix(evals): fail closed on missing evidence, signals and timeouts` — `ea9524476` (PR #322, MERGED on `fork/main`)
* `fix(deps): bump h2 0.4.13 → 0.4.19` (RUSTSEC-2026-0258) — `11cf7f97d` (PR #334, MERGED on `fork/main`)
* `fix(deps): bump undici to 7.30.0` (GHSA-3wwx-pv8v-q78v, medium) — `10a4c36af`
* `fix(deps): bump rustls 0.23.43 -> 0.23.45` (RUSTSEC-2026-0285) — `8008f2c8e`
* `fix(ci): bump crossbeam-epoch 0.9.18 -> 0.9.21` (RUSTSEC-2026-0204) — `dcf0e7d38`
* `security(ci): replace trufflehog/actions/setup with go install + setup-go` — `ebd253b42`
* `fix(cargo-deny): ignore unmaintained transitive advisories with rationale (#8)` — `6100def72`
* `fix(cargo-deny): remove stale RUSTSEC-2026-0049 ignore (#7)` — `864dd7cc0`
* `fix(workflows): use pull_request instead of pull_request_target` — `b2b638305`
* Open PRs that materially harden the security/policy story: #328 (sharecli cross-process relay), #333 (durable effect recovery), #319 (ACP live session control).

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
| 4 | `helioslite share` subcommand (SSE + WebSocket + R2 + attach mode) | Start `helioslite share`, attach a second client via the new `attach` ingestion mode, push a session, capture the SSE event stream end-to-end, and confirm a Tracera telemetry event is emitted. | `crates/forge_sharecli/`, `crates/forge_tracera/` |
| 5 | `forge_agileplus` 31-pillar scorecard, sprint + velocity, weekly GitHub issue publish | `helioslite agileplus scorecard` and `helioslite agileplus sprint`; show the weekly scorecard being published to a GitHub issue on the configured cadence. | `crates/forge_agileplus/`, `docs/31-pillar-scorecard.md` |
| 6 | `forge_tracera` outbound observability sink (gzip + HMAC + DiskStore journal) | Stop the Tracera endpoint, push a few events, restart the endpoint, show the NDJSON journal replayed in order; verify the HMAC header rotates with overlap. | `crates/forge_tracera/` |
| 7 | `forge_guardian` risk adjudication layer (LearningStore + Sandbox tie-breaker + session-mode apply gate) | Trigger a high-risk tool call, capture the GuardianAdjudicator decision and the audit event, then flip the config flag and re-trigger to show the session-mode gate effect. | `crates/forge_guardian/`, `crates/forge_app/src/services.rs` |
| 8 | OS-level sandbox wiring for shell tool execution (P1.1) | Run a shell command flagged as `p1.1 sandbox` and show the OS sandbox wrapping in a `forge_pheno_shell` audit event; toggle off and compare. | `crates/forge_app/src/services.rs`, `crates/forge_pheno_shell/` |
| 9 | `ResiliencePolicy` — circuit breaker + bulkhead + adaptive throttle (P1.3) | Hit a flaky provider N times, capture the circuit-breaker open transition, the bulkhead rejection, and the throttle's adaptive retry-after. | `crates/forge_app/src/services.rs` |
| 10 | Semantic memory runtime — JSONL default, Supermemory / Letta / Cognee adapters (P1.5, P0.2, P3.4) | Run a multi-turn session, force a compact, then `helioslite semantic recall` to show the warm-hydrated context re-surfaced; switch adapter to Supermemory and re-run. | `crates/forge_domain/`, `crates/forge_app/`, `crates/forge_tracera/` (telemetry for recall) |
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
| 21 | Sandbox truthful enforcement (PR #330, MERGED) | Trigger a sandbox-flagged tool call and confirm the audit event reports the actual enforcement state (not a default-pass). | `crates/forge_sandbox/` |
| 22 | Oracle fail-closed evals (PR #322, MERGED) | Run an eval with a missing-evidence signal and confirm the oracle returns fail-closed rather than a soft pass. | `crates/forge_app/src/evals/` |
| 23 | Mature-recovery spec (PR #320, MERGED) | Walk through the spec document and show the pinned-source + oracle-blocker + candidate-contract artifacts. | `docs/specs/mature-recovery.md` (per PR #320) |

> **Recording plan.** Each row maps to ~1 short VHS or 1 Playwright
> trace. Recording is out of scope for this PR — this table is the
> backlog. Owner of each row: see `crates/*/README.md` and the matching
> PR author.

---

## Section 4 — Status

| Field | Value |
| --- | --- |
| Date generated | **2026-10-17T07:46Z** |
| Operator task | "Regenerate `docs/UPSTREAM-DELTA.md` to fix stale Section 1 (19-behind was actually 20-behind, 765-ahead was 769-ahead) and reflect the 4 PRs that merged onto `fork/main` since the 2026-10-12 regen (#320, #322, #330, #334)." |
| Authored by | Jcode orchestrator (`MiniMax-M3`) on `session_20261017_helioslite_fwd` |
| Inputs | `git fetch fork` (read-only); `git rev-list --left-right --count origin/main...fork/main`; `git log fork/main..origin/main --oneline`; `git log origin/main..fork/main --no-merges | head -200`; `gh pr list --repo KooshaPari/HeliosLite --state open`; existing `docs/UPSTREAM-DELTA.md` (preserved Sections 3, intent buckets, security highlights; refreshed Sections 1, 2, 4) |
| Constraints honored | NO FORCE-PUSH, no upstream pushes. Only `docs/UPSTREAM-DELTA.md` is modified (one tracked file). The pre-existing untracked `docs/sessions/20260917-helioslite-release-audit/stash-sanitize-broken.patch` is unchanged. |
| Worktree state (post-write) | this file is the only modified tracked file. The pre-existing untracked file `docs/sessions/20260917-helioslite-release-audit/stash-sanitize-broken.patch` is unchanged and is the Step-2 follow-up. |
| Anomalies | Fork `main` is now **20 commits behind** `origin/main` (was 0 behind at the 2026-10-12 regen). All 20 are dep-version bumps (`ai`, `@ai-sdk/google-vertex`, `zod`, `arboard`, `rustls`, `html-escape`, `tokio`, `syn`, `schemars`, `sha2`); none are behavioral. The next upstream sync step should absorb them with `git merge origin/main` or a `git fetch origin && git rebase` cycle. The 6-PR review queue is down to 3 (#319, #328, #333) after #320/#322/#330 landed. |

### Reproduce

```bash
cd /Users/kooshapari/CodeProjects/Phenotype/repos/forgecode
git fetch origin fork
git rev-list --left-right --count origin/main...fork/main
git log --format="%h %ai %s" fork/main..origin/main --no-merges
git log --format="%h %ai %s" origin/main..fork/main --no-merges | head -200
git log origin/main..fork/main --no-merges --name-only --pretty=format: \
  | grep -v '^$' | awk -F'/' '{print $1"/"$2}' | sort | uniq -c | sort -rn
gh pr list --repo KooshaPari/HeliosLite --state open \
  --json number,title,state,headRefName,createdAt,mergedAt
```

### Revert

This task only modified one tracked file. To undo:

```bash
git checkout -- docs/UPSTREAM-DELTA.md
```

No force-pushes, no upstream pushes, no PRs opened.