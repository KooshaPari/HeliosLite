## UPSTREAM
`tailcallhq/forgecode` (default branch `main`, current head `ace75c107`, post-v2.14.x active release line) — connection-stable Rust/TS coding-agent CLI.

## FORK IDENTITY
`KooshaPari/HeliosLite` (GH ID `1226104769`), additive-only rebrand with no internal symbol rename; `~/.local/bin/helioslite` is the launcher and `~/.forge/` is the active config dir (legacy precedence per `docs/RENAMES-STRATEGY.md`).

## FORK INTENT
Preserve the upstream Forgecode connection-stable journey while proving fork-bound advantages (durable effect recovery, share/LSP cross-process relay, mature-first harness recovery, fail-closed sandbox enforcement, live session/ACP control) without rewriting what already works; additive-only naming until >50% divergence.

## FORK CURRENT STATE
- HEAD on `main` = `e9afd4280` ("merge: upstream 2.14.x sync — 43 dep-bump commits past v2.14.0", 2026-10-11).
- Workspace checked out on `security/h2-0.4.19` (`f7da05ebd`, RUSTSEC-2026-0258 fix, 1 commit past main).
- Working tree clean except 2 untracked: stale `docs/UPSTREAM-DELTA.md` (regen 2026-10-12T21:14Z, needs rerun) and `docs/sessions/20260917-helioslite-release-audit/stash-sanitize-broken.patch` (7882 lines, CODEOWNERS).
- Binary-swap ledger: `docs/sessions/20260808-helioslite-binary-swap-log/ledger.md` (229 lines), `_forge_swap_done.txt` kept as a breadcrumb.
- 6 fork PRs OPEN (#319 ACP live control, #320 mature-recovery spec, #322 oracle hardening, #328 sharecli relay, #330 sandbox truthful, #333 effect recovery) authored late Sep 2026.
- Gates shipped: publish-surface patch `fadb32bda` (Gate 4 partial) — README/install/badge, `crates/forge_main/Cargo.toml` bundle name `helioslite`, `.github/workflows/helioslite-release.yml` + deprecation alias, `docs/RENAMES-CHECKLIST.md` opened.

## DIVERGENCE INVENTORY
- `fork/main` vs `origin/main` = **765 ahead / 19 behind** (live `git rev-list --left-right --count origin/main...fork/main`).
- Ahead-commit surface (`docs/UPSTREAM-DELTA.md` snapshot, 707 just before current run, +~58 added since): 1955 touches in `crates/`, 376 in `.github/` (169 fork-only workflows), 173 in `docs/`, 87 in `Cargo.lock`, 56 in `Cargo.toml`, plus fork-only subsystems `agileplus/`, `shell-plugin/`, `otel/`, `desktop/`, `src-tauri/`, `ui/`. Conventional-commit mix: 257 fix / 130 feat / 95 docs / 85 ci / 71 chore / 27 test / 17 style / 5 perf / 4 refactor / 3 release / 2 security.
- Renames intentionally NOT yet applied (per Gate 4 partial commit body): `FORGE_*` env vars, `forge_*` crate names, DB schema, internal config keys, `crates/forge_*` paths, `helios-bot` crate, `forge-dev` bin target.
- Deferred / flagged: `assets/icons/helioslite.iconset` missing (bundle still points at `assets/icons/forgecode.iconset`); `release.yml` deprecation alias risks clobber on next `forge_ci` regeneration; README.md line 4 references `assets/brand/helioslite-icon.svg` vs actual `forgecode-icon.svg`.
- UPSTREAM-DELTA.md is **stale by ~5 days**; the 19-behind figure vs the cached 0-behind claim is the single biggest drift signal.

## FWD PATH
1. **10m** Regenerate `docs/UPSTREAM-DELTA.md` (`git fetch fork && git rev-list --left-right --count origin/main...fork/main && git log fork/main..origin/main | wc -l`) and commit — single-source-of-truth fix.
2. **10m** Land the 7882-line untracked `stash-sanitize-broken.patch` properly (apply, commit, drop the patch) so the release-audit session is no longer dangling in `docs/sessions/`.
3. **10m** Pick the lowest-risk OPEN PR (#328 sharecli cross-process-relay, narrowest surface) and self-merge through the canonical squash+ledger path; documents the 6-PR review queue.
4. **10m** Open a follow-up PR creating `assets/icons/helioslite.iconset` (cp from forgecode.iconset) and switching the bundle.icon path + README brand icon SVG reference (closes 3 known doc/asset drifts in one).
5. **10m** Re-run `cargo check --workspace` + the publish-surface gates (Gate 1–4) against `helioslite` binary at the new HEAD; tag a HeliosLite `h.0.x.y` release that re-syncs with `origin/main`.

## TIME-TO-FINISH
~50 min of bounded operator-driven follow-through to drop fork drift from "stale-UPSTREAM-DELTA + 7882-line orphan patch + 6 open PRs + 1 tagged release behind" to "fork fully synchronized, publish-surface clean, ready for v2.15.x upstream sync." Trivially within one operational session.

## SUNSET/KEEP CALL
**KEEP.** 765 ahead-commits is substantial, fork-only subsystems (`agileplus`, `otel`, Tauri shell) cannot be reproduced upstream without effort equivalent to the fork itself, the publish-surface patch already separates the fork's published identity from upstream, and the 6 open PRs are exactly the kind of capability work that the additive-only naming policy is designed to protect. SUNSET would throw away a year of kept-up work for zero downstream benefit. Reevaluate after the v2.15.x upstream sync lands and Gate 4 icon-asset follow-up merges.
