# Workflow audit (read-only) — 2026-09-19

Produced by a worker session (opencode-go/deepseek-v4.1-flash) at the request of the release-CI
handoff. Scope: all 24 files in `.github/workflows/` plus `.cargo/config.toml`, `deny.toml` and
`scripts/benchmark.sh`. Nothing was edited by the auditor; the fixes below were applied separately
and verified by the coordinator.

Result: **18 CONFIRMED / 7 SPECULATIVE.** The audit's own coverage table (which checks ran, and where
it found nothing) is at the end of this file — worth reading before trusting the "none found" claims.

# Audit: forgecode `.github/workflows/` + build configs

**Read-only.** Nothing edited, committed, or pushed. Repo `KooshaPari/HeliosLite` (remotes: `fork`=HeliosLite, `origin`/`upstream`=tailcallhq/forgecode).

## Headline

Both reported bugs are **fixed in the current tree**: the `cvp.yml` guard grep exits 1 (no match) and all three rust-analyzer installers target `$RUNNER_TEMP`. The hard-coded-rev class is also fixed (`cvp.yml:57-64` compares the three pins to each other). But **the same two antipatterns survive in other forms**, plus a third class.

**18 CONFIRMED / 8 SPECULATIVE** (numbered findings A1-A7, B1-B3, C1-C8 confirmed; A8-A10, C9-C13 speculative; plus one unnumbered speculative note on `deny.toml`'s RUSTSEC ignore list, documented at the end). *Correction: the first delivery of this report stated "7 SPECULATIVE"; the numbered count is 8. The finding text is unchanged.* Highest severity: a hard-coded release version in 3 files, a Scoop `checkver` regex that provably truncates the fork version, a dead `integration/*` branch list, and a `continue-on-error` that makes cross-platform test failures invisible.

---

## A. Hard-coded expectations that break on a legitimate update

### A1 [CONFIRMED] Fork release version is a literal in three places
- `.github/workflows/ci.yml:95` — `version: 2.13.21-h.0.1.8`
- `.github/workflows/release-drafter.yml:40` — `version: 2.13.21-h.0.1.8`
- `crates/forge_ci/src/jobs/release_draft.rs:11` — `pub const FORK_RELEASE_VERSION: &str = "2.13.21-h.0.1.8";`
- `.github/release-drafter.yml:9-14` documents that this file's `version:` is not honored and the override lives in the generated workflows "sourced from `forge_ci::jobs::FORK_RELEASE_VERSION`. Bump that constant (h-block) on each fork release".

**Why it breaks.** `Cargo.toml:72` already says `version = "2.13.21"` and the file's own comment says the first HeliosLite-only tag will be `3.0.0`. Bumping the manifest without bumping the constant + regenerating leaves the draft release tagged with the previous version, and `APP_VERSION` baked into every release asset follows the draft (`ci.yml:210,219,229`). This is a recurring cost, not a hypothetical: `999b5c504 fix(release): pin HeliosLite h.0.1.7 draft version (#270)` and `555950f6a release(version): bump to 2.13.21-h.0.1.8 for P0-P3 backlog (#299)`.

**Minimal fix.** Drop the constant; derive the version from the manifest in one step (`cargo metadata` or a `Cargo.toml` parse) and feed it with `version: ${{ needs.<job>.outputs.version }}`.

### A2 [CONFIRMED] Scoop `checkver` regex truncates the fork version
- `.github/workflows/update-distribution.yml:127` — `"regex": "v([\\d.]+(?:-h\.\\d+\\.\\d+)?)"` (effective pattern `v([\d.]+(?:-h\.\d+\.\d+)?)`)

I executed it:

| input tag | captured | expected |
|---|---|---|
| `v2.13.21-h.0.1.8` | `2.13.21-h.0.1` | `2.13.21-h.0.1.8` |
| `v2.13.21-h.0.1.7` | `2.13.21-h.0.1` | `2.13.21-h.0.1.7` |
| `v2.13.21-h.0.1.10` | `2.13.21-h.0.1` | `2.13.21-h.0.1.10` |
| `v1.5.3-k0.1.2` | `1.5.3` | `1.5.3-k0.1.2` |

**Why it breaks.** The manifest's own `"version"` is `${VERSION}` = `2.13.21-h.0.1.8`, so `checkver` permanently disagrees with the manifest it generates, and `autoupdate` (lines 129-137) would build download URLs from the truncated version. The `k`/`p` fork-letter scheme from the versioning convention is not covered at all.

**Minimal fix.** Anchor on digit groups: `"v(\\d+\\.\\d+\\.\\d+(?:-[a-z]\\d+\\.\\d+\\.\\d+)?)"`.

### A3 [CONFIRMED] Stale branch lists in workflow triggers
- `.github/workflows/trunk-check.yml:11` — `branches: [main, develop]`
- `.github/workflows/lint.yml:10,14`, `.github/workflows/test.yml:10,14`, `.github/workflows/ci.yml:26` — `integration/forgecode-h0.1.7`

**Verified, not assumed.** `git ls-remote --heads` against all three remotes returned 337 (fork) / 398 (origin) branches, so the remotes are reachable and the absence is real: **no `develop`** and **no `integration/*`** anywhere. The local `remotes/fork/integration/forgecode-h0.1.7` is a stale ref, which is what makes this hard to notice. The current fork version is `h.0.1.8`, so the list is already one rename behind.

**Why it breaks.** Create `integration/forgecode-h0.1.9` and `lint`, `test`, and `ci` will not run on pushes to it or on PRs targeting it. The only remaining gates would be `cvp` and `platform-tests` on `main`. The `develop` entry is simply dead.

**Minimal fix.** `branches: [main, 'integration/**']`, drop `develop`.

### A4 [CONFIRMED] Hard-coded package list in the cross-consumption gate
- `.github/workflows/cvp.yml:47` — `cargo check -p phenotype-health -p phenotype-observability -p phenotype-telemetry` (also `:58` counts only `grep 'PhenoShared.git'` lines)

**Why it breaks.** Add a fourth PhenoShared crate and it is never compiled, while the job named "Gate 1: Cross-consumption validation" keeps passing. The comment above it even hard-codes "The three cross-consumed deps".

**Minimal fix.** Take the package names from `cargo metadata` filtered by a source containing `PhenoShared.git`.

### A5 [CONFIRMED] Hard-coded job list in the lint aggregator
- `.github/workflows/lint.yml:79` — `for result in "${{ needs.fmt.result }}" "${{ needs.clippy.result }}"; do`

**Why it breaks.** Add a third job to `needs` (line 71) without editing this loop and its failure is not counted, so the aggregate check still passes.

**Minimal fix.** `${{ toJSON(needs) }}` piped through `jq -r '.[].result'`.

### A6 [CONFIRMED] Absolute wall-clock threshold as a hard gate
- `.github/workflows/ci.yml:75` — `run: ./scripts/benchmark.sh --threshold 60 zsh rprompt`
- `scripts/benchmark.sh:123` — `if [ $AVG -gt $THRESHOLD ]; then … exit 1`

**Why it breaks.** It fails the workflow when the 10-iteration average of a debug binary exceeds 60 ms on a shared runner. Unrelated legitimate changes can breach it; it measures runner load, not correctness.

**Minimal fix.** Make the step advisory (`continue-on-error: true` plus artifact/comment output) or compare against a recorded baseline instead of an absolute constant.

### A7 [CONFIRMED] Distribution asset list drifts from the release matrix
- `.github/workflows/update-distribution.yml:25-27` lists **6** assets; `.github/workflows/release.yml:23-81` builds **9** targets (adds both musl targets and `aarch64-linux-android`).

Harmless today (brew/scoop only consume gnu/darwin/msvc) but the list must be hand-edited on any rename, and a miss degrades silently into C2. **Fix shape:** derive the distributed set from one source, or at minimum assert every checksum is non-empty before writing manifests.

### A8 [SPECULATIVE] 12 hard-coded repo slugs where `github.repository` is available
`.github/workflows/update-distribution.yml:66,72,75,82,85,110,114,119,125,132,135` and `.github/workflows/helios-bot.yml:69` hard-code `github.com/KooshaPari/forgecode`, while `Cargo.toml:79` declares `repository = "https://github.com/KooshaPari/heliosLite"` and the push remote is `KooshaPari/HeliosLite`. I verified both slugs currently resolve to the same repo (GitHub rename redirect), so **nothing is broken today** — it breaks when the redirect is dropped, when a fork runs the workflow, or for users consuming the committed brew/scoop manifests. The same file already uses `${{ github.repository }}` at line 29.

### A9 [SPECULATIVE] Hard-coded model name
`.github/workflows/helios-bot.yml:52` — `LLM_MODEL: claude-sonnet-4-5`. Retired model identifiers fail at runtime.

### A10 [SPECULATIVE] Pins whose only purpose is matching committed artifacts
`release-attestation.yml:89` (`cargo-cyclonedx --version 0.5.9`, documented at 83-85); `release-attestation.yml:58` `find crates benchmarks` — correct today since every `Cargo.toml:6-61` member is under those two roots, but a future top-level member dir would be silently omitted from the attested subject set. Also `cross-version: 0.2.5` (`ci.yml:207,216,226`; `release.yml:109,119,129`), `trufflehog.yml:24` `3.91.0`, `tsx@4.20.6`, `github-label-sync@3.0.0`. Deliberate pins, listed only as drift points.

**Nothing found in A:** no hard-coded expected file counts (`cvp.yml:58` computes it dynamically — that is the fixed pattern from the reported bug), no hard-coded dates (all timestamps use `date -u`, e.g. `release-attestation.yml:75`), no `==`/`!=` against a pinned version or rev anywhere, no hard-coded "must be exactly N files".

---

## B. Writes into package-manager / toolchain-owned directories

**No workflow writes into `$HOME/.cargo/bin`.** I re-ran the guard's own grep by hand: no match. All three installers now use `$RUNNER_TEMP` (`test.yml:106`, `platform-tests.yml:143`, `helios-lite-nightly.yml:74`).

### B1 [CONFIRMED] The guard would not catch the same bug spelled differently
`.github/workflows/cvp.yml:80`
```bash
if grep -rnE '(bin|dest|target|out)="\$HOME/\.cargo/bin/|> *"\$HOME/\.cargo/bin/' .github/workflows/*.yml; then
```
Every one of these reproduces the original failure and passes the guard:
- variable names outside the fixed set: `install_dir="$HOME/.cargo/bin"`, `DEST=`, `BIN=`
- any copy-style write rather than a redirect: `cp x "$HOME/.cargo/bin/"`, `install -m755 x "$HOME/.cargo/bin/y"`, `tee "$HOME/.cargo/bin/y"`
- `$CARGO_HOME/bin` — the same directory whenever `CARGO_HOME=$HOME/.cargo`, and the location that actually matters if it is overridden
- `$HOME/.rustup/toolchains/*/bin/`
- files outside `.github/workflows/*.yml` (`*.yaml`, any future `scripts/` helper)

It also matches text inside comments, so a future comment documenting the anti-pattern would fail the build. It does **not** self-match today (verified).

**Fix shape.** Grep for the directory as a *write target* with a broader matcher (`\$HOME/\.cargo/bin|\$\{?CARGO_HOME\}?/bin` alongside `>|>>|tee|cp|install|mv` on the same line), scan `*.yaml` too, or invert the test to assert the known-good installers use `$RUNNER_TEMP`.

### B2 [CONFIRMED] `CARGO_HOME` contradiction in the nextest step
`.github/workflows/test.yml:174-176`
```bash
# cargo-nextest is at ~/.cargo/bin; invoke directly to bypass
# cargo's subcommand lookup which fails when CARGO_HOME is overridden.
exec "$HOME/.cargo/bin/cargo-nextest" nextest run --all-features --workspace
```
The comment names an overridden `CARGO_HOME` as the failure mode, and the next line assumes `CARGO_HOME` is unset. `taiki-e/install-action` (`test.yml:140`) installs into `$CARGO_HOME/bin`, so setting `CARGO_HOME: ${{ runner.temp }}/cargo-home` at job level makes this path not exist. **Fix:** `exec "$(command -v cargo-nextest)" …`.

Related drift: `test.yml:90-93` still explains that rust-analyzer is installed before cargo-nextest "so that the bin dir is populated, then `Rust cache` snapshots the populated dir". No longer true — `cache-bin: false` at `test.yml:83`, and the installer writes to `$RUNNER_TEMP`. Only the `$GITHUB_PATH` race still justifies the ordering.

### B3 [CONFIRMED] Workflow-level secret exposed to third-party actions
`.github/workflows/ci.yml:29-31`
```yaml
env:
  RUSTFLAGS: -Dwarnings
  OPENROUTER_API_KEY: ${{secrets.OPENROUTER_API_KEY}}
```
Workflow-level `env` is inherited by every job and step, so `OPENROUTER_API_KEY` enters the environment of `ClementTsang/cargo-action` (`ci.yml:202,212,222`), `arduino/setup-protoc` (`ci.yml:46,67`), and `release-drafter/release-drafter` (`ci.yml:92`). None need it. **Fix:** move it to the one step that needs it, the way `test.yml:167-168` scopes `RUSTFLAGS`.

**Nothing found in B:** no `$HOME/.rustup` write, no `/usr/local/bin` write without sudo, no `$GITHUB_PATH` misuse (all three writes are consumed within the same job, the only scope where it works), no `GITHUB_ENV`/`GITHUB_OUTPUT` misuse. The only `sudo` uses are legitimate `apt-get` calls and the vendor Infisical setup script (`infisical.yml:24-25`).

---

## C. Other fragile coupling

### C1 [CONFIRMED] Cross-platform test failures cannot fail CI
`.github/workflows/platform-tests.yml:176-178`
```yaml
    - name: Run tests
      run: cargo test --workspace
      continue-on-error: true
```
This is the **only** test step in the **only** workflow that runs on `macos-latest` and `windows-latest` (`platform-tests.yml:20-29`) — everything else is `ubuntu-latest`. With `continue-on-error`, only `cargo clippy` (180) and `cargo fmt` (181) can fail the job. A genuine macOS/Windows test regression is invisible, precisely on the two platforms where nothing else tests. **Fix:** remove it, or gate it visibly on a label so the default is still enforced.

### C2 [CONFIRMED] A missing checksum is committed as `sha256 "MISSING"`
`.github/workflows/update-distribution.yml:28-33` `|| echo "Warning: Could not download …"`, then `:46` `else echo "MISSING"`, then `:63-140` writes `${SHA_*}` straight into the formula and JSON, and `:141-159` commits and opens a PR. Rename a distributed asset (or publish with one asset missing) and this job succeeds, commits an uninstallable `sha256 "MISSING"`, and PRs it. **Fix:** `|| { echo "::error::missing ${asset}.sha256"; exit 1; }` and make the `MISSING` branch exit non-zero.

### C3 [CONFIRMED] `cargo fetch` failure swallowed by a pipe
`.github/workflows/cvp.yml:41-44`
```bash
      - name: Verify PhenoShared deps resolve
        run: |
          cargo fetch 2>&1 | tee /tmp/cvp-fetch.log
          echo "✓ Dependencies fetched"
```
`cvp.yml` sets no `defaults.run.shell`, so this runs under the runner default `bash -e {0}` — `-e` without `pipefail`. The pipeline status is `tee`'s, so a failed `cargo fetch` prints the success line and the step passes. This is the first verification step of "Gate 1". Note the same file *does* use `set -euo pipefail` at line 70, so this is an inconsistency. **Fix:** add `set -euo pipefail` to that block, or drop the pipe.

### C4 [CONFIRMED] `cargo deny` never runs the `bans` check
`.github/workflows/cargo-deny.yml:26` — `cargo deny --log-level error check advisories licenses sources`, while `deny.toml:49-53` configures `[bans] multiple-versions`, `wildcards`, `highlight`, `workspace-default-features`. The four check kinds are advisories/bans/licenses/sources; omitting `bans` makes that whole config section dead. **Fix:** append `bans`.

### C5 [CONFIRMED] Windows benchmark artifact is always empty
`.github/workflows/benchmarks.yml:21` matrix includes `windows-latest`; `:78-80` uploads `target/release/forge` and `target/release/forge_dbd`. On Windows those are `.exe` — the file knows this at `:51-52`. `if-no-files-found` is unset so it defaults to `warn`: green job, empty artifact. **Fix:** add the `.exe` paths or glob `forge*`; set `if-no-files-found: error`.

### C6 [CONFIRMED] Benchmark timings reported for a binary that may not have run
`.github/workflows/benchmarks.yml:62` — `"$BINARY" --version > /dev/null 2>&1 || true`, then `:64-65` print and record the elapsed time regardless. A binary that crashes at startup still yields a plausible "Run 1: 12ms". **Fix:** fail on non-zero exit.

### C7 [CONFIRMED] Bot posts its own error output as the answer
`.github/workflows/helios-bot.yml:69-77` — `cargo install … || true`, then `RESPONSE=$(forge … 2>&1) || true`, then posts `$RESPONSE` as the issue comment with `|| true` on the post. If the install fails, `forge` resolves to something else or nothing, and the captured stderr is published as the bot's response; a failed comment post is also hidden. **Fix:** drop the `|| true`s and only post on success.

### C8 [CONFIRMED, security] `client_payload` interpolated into shell text
`.github/workflows/helios-bot.yml:54-56`
```bash
REQUEST="${{ github.event.inputs.request || github.event.client_payload.request }}"
REPO="${{ github.event.inputs.repo || github.event.client_payload.repo }}"
```
`${{ }}` is substituted *before* bash parses the script, so a payload containing `"; … ;"` or `$(…)` executes with `HELIOS_BOT_TOKEN` in scope (`:49`). **Fix:** pass through `env:` and reference `"$REQUEST"` — never interpolate `${{ }}` into `run:` bodies. (`bounty.yml:43,57` interpolates only a numeric PR number, so it is not exploitable.)

### C9 [SPECULATIVE] Cross-repository ordering asserted only in a comment
`.github/workflows/helios-lite-nightly.yml:3-8` claims it "Runs every day at 06:30 UTC after the Omniroute nightly (`argismonitor-nightly.yml`) so the rename cycles stay deterministic". That workflow is in a different repository and nothing (`needs`, `workflow_run`) enforces it. **Fix:** use `workflow_run`, or downgrade the comment to best-effort.

### C10 [SPECULATIVE] Per-job cache keys prevent workspace reuse across gates
`.github/workflows/cvp.yml:40,104,127,151,178` all cache `target`, but every key has a distinct prefix (`cvp-`, `cvp-build-`, `cvp-test-`, `cvp-lint-`, `cvp-cov-`) and none sets `restore-keys`, so no job restores another's `target`. All five recompile from scratch, which makes the `timeout-minutes: 45` (`:90,112`) the binding constraint rather than a safety net. I have not timed a cold build, so **no timeout is confirmed hit**. **Fix:** one shared key prefix plus `restore-keys`.

### C11 [SPECULATIVE] Timeouts are mostly absent
Verified: 19 of 24 workflow files contain no `timeout-minutes`; only `cvp.yml`, `helios-lite-nightly.yml`, `fuzz.yml`, `infisical.yml`, `trunk-check.yml`, `test.yml` do. **No instance of "timeout shorter than the work it wraps" found.** The two I would watch are `fuzz.yml:15` (15 min for `cargo install cargo-fuzz --locked` plus `-runs=1000`) and `trunk-check.yml:26` (20 min for a system-dep install plus `clippy --all-targets --all-features`), but I cannot time them here.

### C12 [SPECULATIVE] Three workflows are all named `ci`
`ci.yml:10`, `lint.yml:1`, `test.yml:1`. Job names differ (`ci / test`, `ci / lint`) so required-check contexts stay distinct, but `gh run list --workflow ci` and `github.workflow` comparisons cannot distinguish them.

### C13 [SPECULATIVE] `cvp.yml` is the only workflow using floating action tags
20 occurrences of `@v4` / `@stable` / `@v3` at `cvp.yml:28,29,30,34,92,93,94,98,115,116,117,121,137,138,141,145,164,165,166,172,184`, against SHA pinning everywhere else. Separately, the same actions carry **different** SHAs across files (`actions/checkout`: 4 distinct, `dtolnay/rust-toolchain`: 3, `actions/cache`: 2, `actions/upload-artifact`: 2). Relevant because `cvp.yml` is the workflow enforcing the `.cargo/bin` guard — a floating `@stable` can change the toolchain behaviour the guard exists to constrain, with no commit.

**Nothing found in C:** no missing job-level `needs` (all of `release.yml:182,189,226,259`, `cvp.yml:113,162`, `helios-lite-nightly.yml:127` declare their dependencies correctly); no `if-no-files-found: error` on an optional artifact (all four `error` uses cover paths the preceding steps must produce; `release-attestation.yml:98` correctly uses `warn`); no `|| true` masking a correctness result beyond C6/C7 (`release-attestation.yml:58` and the `rustc -vV` captures are diagnostics where the surrounding assertion does the real check — `test.yml:157` and `platform-tests.yml:116` assert *after* capturing, which is the right shape and avoids the `pipefail`/SIGPIPE trap documented at `test.yml:153-155`).

---

## `.cargo/config.toml`, `deny.toml`, scripts

- **`.cargo/config.toml`**: nothing found. The single `[net] git-fetch-with-cli = true` is not an expectation about project state.
- **`deny.toml`**: C4 only, plus a SPECULATIVE item — the RUSTSEC `ignore` list (`:27-48`, 6 entries) is a hard-coded allowlist nothing prunes once upstream fixes the advisory. The file's own history ("entry already removed in 40114d8dc") shows pruning is manual. **Fix shape:** periodically run `cargo deny check advisories` without the ignore list to detect stale entries, or require a re-evaluation note per entry.
- **`scripts/benchmark.sh`** (the only workflow-referenced script, at `ci.yml:75`): covered in A6. Otherwise sound — `set -euo pipefail` at line 10 makes the `cargo build | grep | tail` at line 59 correctly fail-closed.
- **No `Makefile`/`makefile`/`GNUmakefile` exists** at any level.

---

## Coverage: checks run, and where I found nothing

| # | Check | Result |
|---|---|---|
| 1 | `grep -rn '\.cargo/bin'` on all workflows | No write; reads/diagnostics only |
| 2 | Re-ran `cvp.yml` guard pattern by hand | exit 1, no match, no self-match |
| 3 | `grep -rn '\.rustup\|/usr/local/bin'` | **none found** |
| 4 | `GITHUB_PATH`/`GITHUB_ENV`/`GITHUB_OUTPUT` audit | **none misused**; all intra-job |
| 5 | Enumerate all `@<sha>` pins | Only project rev pin is PhenoShared, already fixed |
| 6 | **Executed** Scoop checkver regex on 5 tags | 4 of 5 truncate (A2) |
| 7 | `git ls-remote` all 3 remotes for `develop`, `integration/*` | Neither exists; remotes reachable (337/398 branches), so absence is real (A3) |
| 8 | `grep -rnE 'expected\|wc -l\|must be exactly\|<date>'` | **no hard-coded counts or dates** |
| 9 | Enumerate every `continue-on-error`, `\|\| true`, `if-no-files-found` | Classified; C1/C2/C6/C7 + one safe `warn` |
| 10 | Locate real pipes, cross-reference `pipefail`/`shell:` per file | Isolated `cvp.yml:43`; the four `curl \| gunzip` sites are all inside `set -euo pipefail` blocks; `sign-release.yml:41-43` sets `shell: bash` (`-eo pipefail`) |
| 11 | Release matrix (9 targets) vs distribution asset list (6) | A7 |
| 12 | `deny.toml` tables vs `cargo deny check` args | C4 |
| 13 | `Cargo.toml` members vs `find crates benchmarks` | A10 |
| 14 | `Cargo.toml` repo field + remotes vs hard-coded slugs | A8, verified both resolve today |
| 15 | Hard-coded version vs its Rust generator + git log | A1, with the two bump commits as evidence |
| 16 | `timeout-minutes` presence per file | C11; no too-short timeout found |
| 17 | Read all 24 workflows, both configs, `release-drafter.yml`, `benchmark.sh` | Complete |

**Coverage gaps I did not close:** I could not time a cold `cargo check --workspace`, `cargo test --workspace`, or a `clippy --all-targets --all-features` on this repo, so the timeout risks in C10/C11 stay SPECULATIVE by construction. I also did not audit `.github/dependabot.yml`, `.mergify.yml`, `.pre-commit-config.yaml`, or `trunk.yaml`, since they are outside the requested set.

---

## What the coordinator fixed from this audit (verified)

| Finding | Fix | Verification |
|---|---|---|
| A2 — Scoop `checkver` truncated the fork version, so `autoupdate` built non-existent URLs | `update-distribution.yml`: backslash-free pattern accepting both fork schemes (`-h.0.1.8` and `-k0.1.2`) | Ran the workflow's own heredoc to generate the manifest, then applied `checkver` to 6 tags — all captured in full; before: `v2.13.21-h.0.1.8` -> `2.13.21-h.0.1` |
| A3 — `integration/forgecode-h0.1.7` triggers (version-pinned, absent on the fork) | `ci.yml`, `lint.yml`, `test.yml` -> `integration/**` | `git ls-remote` confirmed the branch is absent; YAML revalidated |
| C3 — `cargo fetch \| tee` hid failure (no `pipefail`) | `cvp.yml`: `set -euo pipefail` | YAML validated |
| C4 — `cargo deny check` omitted `bans`, making all of `[bans]` dead config | Added `bans` to the check list | `cargo deny --log-level error check bans` -> `bans ok`, exit 0 |
| C2 — a missing checksum was committed as `sha256 "MISSING"` | `update-distribution.yml`: fail loudly (missing and empty both `::error::` + exit 1) | Exercised all three cases locally: complete -> env written; missing -> exit 1; empty -> exit 1 |
| C7/C8 — bot posted its own stderr as the answer, and payload values were interpolated into the script body with `HELIOS_BOT_TOKEN` in scope | `helios-bot.yml`: values passed via `env`, `|| true`s removed | Structural assertion: no `run:` body in that workflow contains `${{ }}` any more |
| B3 — workflow-level `OPENROUTER_API_KEY` exported to every step and third-party action | Removed from `ci.yml` (no step there used it) | YAML validated |

## Still open (recommended, not applied)

- **A1** — the fork release version is a literal in three places (`ci.yml:95`, `release-drafter.yml:40`,
  `crates/forge_ci/src/jobs/release_draft.rs:11`) and must be bumped by hand each release; the previous
  two releases needed exactly such a commit. Derive it from `Cargo.toml` instead.
- **C1** — `platform-tests.yml` runs the workspace tests on macOS and Windows (the only workflow that
  does) with `continue-on-error: true`, so a cross-platform test regression cannot fail CI. This is
  the same hole that hid the failures diagnosed in sections 4.6 and 4.7. Removing it needs evidence
  that the suite is green on those platforms first; a macOS run was in flight at handoff time.
- **B1** — the `cvp.yml` guard matches a fixed set of variable names and only `*.yml`, and it also
  matches text inside comments. It would miss `CARGO_HOME/bin`, a `cp`-style write, `*.yaml`, or a
  differently named variable. Worth broadening to test the directory as a write target.
- **B2 (partly fixed)** — `test.yml` now resolves `cargo-nextest` via `command -v`; the stale comment
  above it about caching the bin dir was also corrected.
- **C5** — the Windows benchmark artifact is always empty (it uploads `target/release/forge`, but
  Windows builds `.exe`), and `if-no-files-found` defaults to `warn`.
- **C6** — benchmarks record a timing even when the binary failed to launch (`--version > /dev/null
  2>&1 || true`).
- **A6** — the "Performance" job gates on an absolute 60 ms wall-clock threshold, which measures
  runner load as much as anything else.
- **C10/C13** — `cvp.yml` uses five distinct cache key prefixes (no `restore-keys`), so every gate
  recompiles; and it is the only workflow on floating action tags (`@v4`/`@stable`) while everything
  else is SHA-pinned.
- **A6/A7/A8/A9/A10/C9/C11/C12** — drift points and advisories; details in the report body.

