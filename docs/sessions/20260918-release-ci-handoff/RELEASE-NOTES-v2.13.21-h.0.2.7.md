## HeliosLite v2.13.21-h.0.2.7 — ships the language-server fixes

Cut from `aaef98bf0` (every workflow green; code that matters: `a8ad28703`).

### Fixes shipped since v2.13.21-h.0.2.6

These are all in the `forge` / `helioslite` binaries — `crates/forge_main/Cargo.toml:109` links
`forge_lsp`, and `forge_main` builds the released `forge` target.

- **`McpWatcherHandle::stop` no longer hangs.** The background task parks in `rx.recv()` waiting for
  the next filesystem event, and `stop()` only set a flag, so shutdown blocked until an unrelated
  file event arrived (or the caller's timeout expired). Adds a `shutdown` `Notify` and selects on it
  in both the event wait and the debounce drain (`d9a5871a9`). Observed before/after on the same
  test: a deterministic 10.29s hang vs 13/13 tests passing in 1.07s.
- **`ProcessLspClient::did_open` and `Server::open_document`.** Language servers only serve requests
  for documents they know about; without `textDocument/didOpen`, rust-analyzer answered definition
  requests with `-32603 file not found`. The LSP e2e test also writes a minimal manifest and polls,
  because rust-analyzer answers while it is still loading (`3b1ebf4e7`).
- **File-scoped CLI commands work on a cold server.** `forge_lsp::commands::run_command` now
  registers the target document (`Server::open_document_file`) and `Command::Definition` waits for
  the project to load. Before: `lsp server error: file not found (code -32603)`; after: a real
  location (`be581d8c9`, `77607f74c`).

### CI fixes carried in this release line

- `sign_release` caller permission raised to `contents: write` (reusable workflows cannot elevate
  beyond the caller grant — this was the original `startup_failure`).
- Windows signing is skipped with a warning when `SIGNPATH_*` credentials are absent, instead of
  failing the whole release; Windows binaries are therefore **unsigned** until those secrets and
  variables are configured.
- `platform-tests` / `test.yml` / `helios-lite-nightly`: the rust-analyzer installer no longer writes
  into `$HOME/.cargo/bin`, where `rust-analyzer` is a symlink to `rustup` and the redirect was
  overwriting `rustup` itself (this broke every cargo shim on macOS and ubuntu).
- `.config/nextest.toml` keeps its 30s `terminate-after` default; the two LSP e2e binaries get a 90s
  override because they legitimately wait for a language server to load a project.
- `cvp` no longer compares the PhenoShared rev against a hard-coded value; it asserts the three
  cross-consumed dependencies agree.
- Denied-lint (`clippy::indexing_slicing`) violations cleared in `forge_sharecli` and `forge_config`.

### Verified before publishing

Release `v2.13.21-h.0.2.6` (same code line minus the LSP fixes) was validated end to end: assets
downloaded anonymously, checksums matched, the macOS arm64 binaries executed, and the signature
verified with `codesign --verify --strict` under Developer ID Koosha Paridehpour (GCT2BN8WLL).
Expect the same 55 assets here (27 binaries + 27 `.sha256` + `sbom.cdx.json`) across all 9 targets.
