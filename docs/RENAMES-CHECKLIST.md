# HeliosLite publish-surface rename checklist (Gate 4)

Track progress on the external-rename surface. Internal identifiers
(env vars, crate names, schema) are deliberately preserved per
[`docs/RENAMES-STRATEGY.md`](./RENAMES-STRATEGY.md) until >50% fork
divergence; they are **NOT** on this list.

This file is the operator-facing companion to the publish-surface
PR (Gates 4-7 of the strategy doc). Internal items the agent cannot
publish (registry credentials, domain registrar access) live here so
the human side of the operation can drive them to completion.

## crates.io

- [ ] Verify KooshaPari owns the `helioslite` crate name on crates.io (squat protection)
- [ ] Publish new `helioslite` 2.13.21 release on crates.io
- [ ] Mark `forge-dev` as deprecated on crates.io with a pointer to `helioslite`
- [ ] Wait one release cycle, then yank `forge-dev` (Gate 5)

## Homebrew

- [ ] Rename `forge-dev` tap formula to `helioslite` (or add `helioslite` as a new formula in a personal tap)
- [ ] Update brew command in README.md install section (lines 110-148)
- [ ] Push formula to `homebrew-core` (or keep in personal tap if not yet popular)

## Chocolatey

- [ ] Create `helioslite` chocolatey package
- [ ] Submit to `chocolatey.org` moderated repo
- [ ] After approval, update README.md Windows install section

## winget

- [ ] Create `KooshaPari.helioslite` winget manifest
- [ ] Submit PR to `microsoft/winget-pkgs`
- [ ] After merge, update README.md Windows install section

## Snap / Flatpak / AppImage

- [ ] Decide whether to publish on snapcraft.io (Linux desktop)
- [ ] Decide whether to publish on flathub (Linux desktop)
- [ ] Update AppImage naming from `forge-dev-x86_64.AppImage` to `helioslite-x86_64.AppImage`

## Docs / Domains

- [ ] Confirm `helioslite.phenotype.space` DNS record is current
- [ ] Confirm `helioslite.pheno.studio` DNS record is current
- [ ] Update any external links pointing to old `forgecode.*` URL paths
- [ ] Update any external links pointing to old `forge-dev.*` URL paths

## GitHub repo

- [x] Confirm `KooshaPari/heliosLite` (note the camelCase) is the canonical remote — **verified 2026-10-08**
- [x] Update README badge URL `KooshaPari/forgecode` → `KooshaPari/heliosLite` — **done in this PR**
- [x] Update README install URL `KooshaPari/forgecode/releases` → `KooshaPari/heliosLite/releases` — **done in this PR**
- [ ] Update any other README or doc refs that still use `KooshaPari/forgecode`
- [ ] Update `.git/config` remote topology doc if any (see audit #20 in audit report)

## Macros / assets

- [ ] Create `assets/icons/helioslite.iconset/` (current iconset is still `forgecode.iconset`)
  - Suggested: `cp -r assets/icons/forgecode.iconset assets/icons/helioslite.iconset`
  - Then update internal icon PNG references inside the new iconset
- [ ] Create `assets/icons/helioslite.ico` (Windows icon, currently `forgecode.ico`)
- [ ] Create `assets/icons/helioslite-256x256.png` (readme/render icon)
- [ ] Verify `assets/brand/helioslite-icon.svg` exists (README.md line 4 references it but the actual file is `forgecode-icon.svg` — pre-existing doc drift, NOT in this PR's scope)
- [ ] Update any non-Cargo icon refs (DEB build, RPM spec, NSIS script)

## Workflow file rename (Gate 4 — publish-side)

- [x] `.github/workflows/release.yml` → `.github/workflows/helioslite-release.yml` (with `name:` updated) — **done in this PR**
- [x] Deprecation alias `.github/workflows/release.yml` that dispatches to `helioslite-release.yml` — **done in this PR**
- [x] **CRITICAL:** update the `forge_ci` private workflow model so it does **NOT** clobber the deprecation alias on next regeneration. — **done in n5-ci-hygiene**: `release_publish()` now emits the canonical workflow to `helioslite-release.yml` and no longer writes `release.yml` (option (b): dropped from the emit list). `crates/forge_ci/tests/ci.rs` snapshots `helioslite-release.yml`; the alias is hand-maintained.

## Bundle metadata (Gate 4 — publish-side)

- [x] `crates/forge_main/Cargo.toml` `name` → `"helioslite"` — **done in this PR**
- [x] `crates/forge_main/Cargo.toml` `identifier` → `"ai.kooshapari.helioslite"` — **done in this PR**
- [ ] `crates/forge_main/Cargo.toml` `icon` path → `"../../assets/icons/helioslite.iconset"` — **DEFERRED** (waiting for icon asset creation above; current `forgecode.iconset` path preserved so build still works)

## README.md patches (Gate 4 — publish-side)

- [x] Badge URL `KooshaPari/forgecode` → `KooshaPari/heliosLite` (line 9) — **done in this PR**
- [x] Status table `Binary: forge` → `Binary: helioslite` (line 62) — **done in this PR**
- [x] Status table `Version: 2.10.0` → `Version: 2.13.21` (line 63) — **done in this PR**
- [x] Install section heading `## Install forge-dev` → `## Install helioslite` (line 110) — **done in this PR**
- [x] Install intro `Grab the latest forge-dev binary` → `Grab the latest helioslite binary` (line 112) — **done in this PR**
- [x] Install URL (line 115) — **done in this PR**
- [x] Install paths `/usr/local/bin/forge-dev` and `~/.local/bin/forge-dev` (lines 119-120) — **done in this PR**
- [x] Source build commands `--bin forge-dev` (lines 124, 131) — **done in this PR**

## Session-doc hygiene (per AGENTS.md)

- [x] Move `_forge_swap_done.txt` (root) → `docs/sessions/20260808-helioslite-binary-swap-log/ledger.md` — **done in this PR**
- [x] Leave a one-line marker at the root explaining the move — **done in this PR**

## Internal identifiers — DELIBERATELY NOT ON THIS LIST

The following are blocked per [`docs/RENAMES-STRATEGY.md:35`](./RENAMES-STRATEGY.md) and
[`docs/RENAMES-STRATEGY.md:94-99`](./RENAMES-STRATEGY.md) until fork divergence is >50%.
Renaming them now would generate hundreds of merge conflicts on every upstream rebase.

- `FORGE_*` env vars → `HELIOSLITE_*` : **BLOCKED** until >50% divergence
- `forge_*` crate names → `helioslite_*` : **BLOCKED** until >50% divergence
- DB schema names (e.g. `forge_sessions`, `forge_workspaces`) : **BLOCKED** until >50% divergence
- Internal config keys (e.g. `~/.forge/`, `forge.*` YAML keys) : **BLOCKED** until >50% divergence
- `crates/forge_*` directory paths : **BLOCKED** until >50% divergence
- `helios-bot` fork-only crate : **PRESERVED** (key fork differentiator, never renamed)

The current PR (this one) only touches the **publish surface** (lines 154-156 of
`crates/forge_main/Cargo.toml`, README.md user-facing copy, workflow file name,
session-doc hygiene). Internal identifiers remain additive-only per Gate 1b.
