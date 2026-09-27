# Technical Debt Register

Living register of known, deliberately-accepted debt in HeliosLite (fork of
tailcallhq/forgecode). Reviewed: **2026-09-27**. Add dated entries; strike
through and date items when paid down. Do not use `_STATUS`/`_REPORT`
variants — this file is the canonical debt doc.

## 1. Code-size mandate violations (AGENTS.md §3: ≤500 hard, ≤350 target)

Most violations are **inherited from upstream**. Mass decomposition would
create permanent merge friction on every upstream sync, so the fork policy
is: decompose opportunistically when a file is already being edited
(e.g. `forge_lsp/definition.rs` split 590 → 296 lines on 2026-09-27), and
track the rest here rather than blind-refactor.

Worst offenders at 2026-09-27 (lines):

| Lines | File |
|---:|---|
| 6698 | `crates/forge_main/src/ui.rs` |
| 5296 | `crates/forge_repo/src/conversation/conversation_repo.rs` |
| 2881 | `crates/forge_app/src/utils.rs` |
| 2652 | `crates/forge_app/src/operation.rs` |
| 2519 | `crates/forge_main/src/cli.rs` |
| 2379 | `crates/forge_repo/src/provider/provider_repo.rs` |
| 2277 | `crates/forge_repo/src/provider/bedrock.rs` |
| 2258 | `crates/forge_repo/src/provider/openai_responses/repository.rs` |
| 567 | `crates/forge_lsp/src/lsp_client.rs` (fork-touched; next split candidate) |

## 2. `forge_lsp` test scaffolding duplicated 6×

`MockLspClient` is copy-pasted in `hover.rs`, `completion.rs`,
`implementation.rs`, `references.rs`, `rename.rs` and `definition/tests.rs`
(the last one gained response-*sequence* scripting for the ContentModified
retry tests). Consolidate into one `#[cfg(test)] pub(crate) test_support`
module — requires touching all six in one change (sequenced refactor; low
urgency).

## 3. Multi-version `rand` in Cargo.lock

`rand` 0.8.8 (via `oauth2` → `forge_infra`), 0.9.4 and 0.10.2 coexist.
Dependabot alert #7 (LOW, `>=0.7.0,<0.8.6`) cleared 2026-09-27 by bumping
0.8.5 → 0.8.8 (`acb5c54a9`). Consolidation is blocked until `oauth2`
migrates off the 0.8 line; revisit on an `oauth2` release.

## 4. Nightly LSP e2e fragility (mitigated, not eliminated)

Cold, CPU-starved runners keep rust-analyzer answering `ContentModified`
(-32801) well past the old budgets — nightly runs 35996603815, 36132727849
and 36318434190 failed this way while 36239433886 passed. Landed
mitigations: product-level retry in `DefinitionProvider::definition`
(20×400ms on the same warm session), test budgets 90s → 150s, nextest
`terminate-after` 12 → 16. Residual risk: churn exceeding every layer of
budget. Structural fix = larger runner or serializing the LSP e2e tests —
a cost decision for the operator.

## 5. cargo-deny CI tool version

Pinned `cargo-deny@0.19.0` in `.github/workflows/cargo-deny.yml` on
2026-09-27 (the `taiki-e/install-action` SHA pinned the *installer*, the
tool itself floated to latest on every run). Bump deliberately.

## 6. Dependabot re-scan behavior

Lock-file alerts re-evaluate within seconds of a push (alert #7 closed
16:23:30Z, push landed 16:23:27Z), but a stale-looking alert can also sit
open for days when the fix commit predates any scan (the 09-20 scan was
3.5h *before* the P1 fix). Always compare `updated_at` against the fix
commit time before calling an alert stale or dismissing it manually.

## 7. Windows Platform-Tests `setup-protoc` flake

`hung up by peer` on run 35980044869 (2026-09-24); green on all three
subsequent runs (35984484944, 35992570037, 36329660284). No action unless
it recurs — if it does, pin the protoc release asset URL instead of
`latest`.

## 8. Upstream drift watch

`origin/main` fully merged as of 2026-09-24 (last: `f11c1bcc7`, dirs
7.0.0). Re-check after upstream releases. Hard-won rules: never resolve
`Cargo.lock` merges with `'theirs'` (diff security entries — crossbeam-*,
ring, rustls, tokio, h2 — and let `cargo deny` be the tripwire); verify
additive doc-merge resolutions with `git diff parent..HEAD --stat`.
