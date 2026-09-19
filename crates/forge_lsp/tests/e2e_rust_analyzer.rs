//! End-to-end integration test for `forge_lsp::Server`.
//!
//! This test exercises the **full** `Server` facade against a real
//! `rust-analyzer` subprocess — it spawns the language server, completes
//! the LSP `initialize` handshake, drives a `textDocument/definition`
//! request over JSON-RPC over stdio, and verifies the response resolves
//! back into the source file.
//!
//! No mocks, no `MockLspClient`, no scripted responses — the only thing
//! stubbed is the absence of the binary on PATH.
//!
//! ## Skip behaviour
//!
//! If `rust-analyzer` is not on `PATH` (Windows: not discoverable via
//! `where`), the test prints
//! `e2e: skipping, rust-analyzer not on PATH` and returns success. This
//! keeps CI green on machines that don't have the language server
//! installed (the unit + mock-driven integration tests cover those
//! environments). The test is **not** `#[ignore]`'d — `cargo test`
//! invokes it by default; the early-exit branch is the skip path.
//!
//! When `rust-analyzer` IS available, the test must round-trip a real
//! `textDocument/definition` request and observe a `Location` pointing
//! into `src/lib.rs` within a 30-second wall-clock budget. A hung
//! `rust-analyzer` will trip the timeout rather than block CI.

use std::path::Path;
use std::time::Duration;

use bstr::ByteSlice;
use forge_lsp::{Location, Server};
use tempfile::TempDir;
use tokio::time::timeout;

/// Trivial single-file workspace: a single public function `add`. The
/// `definition` request targets `add` itself, so we expect the response
/// to point back into `src/lib.rs` (the same file we just opened).
const TRIVIAL_LIB_RS: &str = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";

// The round-trip includes rust-analyzer loading the project (it runs
// `cargo metadata` and builds the crate graph), which takes ~16s on a warm
// developer machine and longer on a loaded CI runner. The budget is generous
// so a slow runner does not look like a hang; a genuinely stuck server still
// trips it.
const E2E_BUDGET: Duration = Duration::from_secs(90);

/// Locate a working `rust-analyzer` on the current host. Returns the
/// binary's path on success, `None` otherwise.
///
/// Two-stage probe:
/// 1. `where` (Windows) / `which` (Unix) to find a candidate on
///    `PATH`. A bare presence check catches most "not installed"
///    cases on CI.
/// 2. `--version` against the candidate, which catches the
///    pathological case where a `rustup` shim exists but the
///    underlying component isn't installed (the shim returns
///    "Unknown binary ... " on stderr and exits non-zero). If
///    `--version` doesn't succeed, we treat the binary as
///    unavailable so the test skips cleanly.
fn find_rust_analyzer() -> Option<std::path::PathBuf> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let output = std::process::Command::new(cmd)
        .arg("rust-analyzer")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = output.stdout.to_str_lossy();
    let candidate = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_string();

    // Verify the candidate responds to `--version`. A rustup shim for
    // a missing component answers "Unknown binary ..." and exits
    // non-zero; we want to treat that as "not installed" rather than
    // as a broken LSP round-trip.
    let probe = std::process::Command::new(&candidate)
        .arg("--version")
        .output()
        .ok()?;
    if !probe.status.success() {
        return None;
    }

    Some(std::path::PathBuf::from(candidate))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_definition_round_trip_against_real_rust_analyzer() {
    // ----------------------------------------------------------------
    // Skip path — rust-analyzer not on PATH. Print the documented
    // message and exit successfully so CI stays green on minimal
    // build agents.
    // ----------------------------------------------------------------
    if find_rust_analyzer().is_none() {
        eprintln!("e2e: skipping, rust-analyzer not on PATH");
        return;
    }

    // ----------------------------------------------------------------
    // Build the workspace on disk. `Server::with_defaults` will run
    // its `initialize` handshake against this root.
    // ----------------------------------------------------------------
    let tmp = TempDir::new().expect("create temp workspace");
    let workspace_root = tmp.path().to_path_buf();
    let src_dir = workspace_root.join("src");
    std::fs::create_dir_all(&src_dir).expect("create src dir");
    std::fs::write(src_dir.join("lib.rs"), TRIVIAL_LIB_RS).expect("write src/lib.rs");
    // rust-analyzer resolves semantic information for files that belong to a
    // loaded project. A bare directory is treated as an empty project and the
    // definition request returns no locations even after `didOpen`, so give it
    // a minimal dependency-free manifest. The temp workspace has no parent
    // `rust-toolchain.toml`, so the default toolchain applies.
    std::fs::write(
        workspace_root.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"forge-lsp-e2e\"\n",
            "version = \"0.0.0\"\n",
            "edition = \"2021\"\n\n",
            "[lib]\n",
            "path = \"src/lib.rs\"\n\n",
            "[dependencies]\n",
        ),
    )
    .expect("write Cargo.toml");

    // The position to query — line 0, column 7 — lands on the `add`
    // identifier in `pub fn add(...)`. We request goto-definition on
    // `add` itself, which must resolve to a `Location` covering the
    // same line.
    let query_position = forge_lsp::lsp_client::Position { line: 0, character: 7 };

    // ----------------------------------------------------------------
    // Drive the full LSP pipeline inside `spawn_blocking` so the
    // blocking stdio framing on `Server::with_defaults` /
    // `Server::definition` doesn't stall the tokio reactor. The whole
    // thing is wrapped in `tokio::time::timeout` so a hung server
    // surfaces as a clean failure rather than a hung CI job.
    // ----------------------------------------------------------------
    let workspace_for_blocking = workspace_root.clone();
    let outcome: Result<Vec<Location>, String> = match timeout(E2E_BUDGET, async {
        tokio::task::spawn_blocking(move || -> Result<Vec<Location>, String> {
            // Construct the Server. This spawns rust-analyzer +
            // typescript-language-server and runs both LSP `initialize`
            // handshakes. A failure here (e.g. tsc missing on a Rust-
            // only box) is reported verbatim — the round-trip is
            // genuinely broken in that environment, and the test
            // exists to catch exactly that.
            let server = Server::with_defaults(&workspace_for_blocking)
                .map_err(|e| format!("Server::with_defaults failed: {e}"))?;
            // Language servers only serve requests for documents they know
            // about. Without `textDocument/didOpen`, rust-analyzer answered the
            // definition request below with
            // `lsp server error: file not found: <tmp>/src/lib.rs` (code -32603)
            // because the file was not in its VFS.
            server
                .open_document(Path::new("src/lib.rs"), TRIVIAL_LIB_RS)
                .map_err(|e| format!("didOpen failed: {e}"))?;

            // Drive the real LSP pipeline: textDocument/definition
            // over the subprocess stdio. No mocks involved.
            //
            // rust-analyzer answers requests while the project is still
            // loading, returning an empty definition list until analysis is
            // ready, so poll until it produces a result. Poll for as long as
            // the outer budget allows, minus a margin, so a genuine
            // "never answers" case reports through the assertion below rather
            // than as an outer timeout. A fixed 20s deadline used to cut this
            // short: rust-analyzer's first answer on a cold, loaded CI runner
            // can take far longer, and giving up early turned runner load into
            // a red test (nextest exit 100 across ci.yml, test.yml and cvp.yml
            // with no code change).
            let deadline =
                std::time::Instant::now() + E2E_BUDGET.saturating_sub(Duration::from_secs(10));
            let locations = loop {
                let locations = server
                    .definition(Path::new("src/lib.rs"), query_position.clone())
                    .map_err(|e| format!("definition request failed: {e}"))?;
                if !locations.is_empty() || std::time::Instant::now() >= deadline {
                    break locations;
                }
                std::thread::sleep(Duration::from_millis(250));
            };
            Ok(locations)
        })
        .await
        .map_err(|e| format!("blocking task join failed: {e}"))?
    })
    .await
    {
        Ok(inner) => inner,
        Err(_) => panic!(
            "e2e: LSP round-trip exceeded {E2E_BUDGET:?} budget — \
             rust-analyzer likely hung"
        ),
    };

    let locations = match outcome {
        Ok(l) => l,
        Err(e) => panic!("{e}"),
    };

    // Verify: rust-analyzer must report at least one Location, and it
    // must point back into our generated `src/lib.rs` on the same
    // line we queried. The URI shape depends on `Url::from_file_path`
    // (different on Windows vs. Unix), so we match the trailing path
    // component rather than the full URI.
    assert!(
        !locations.is_empty(),
        "rust-analyzer returned no definition locations; \
         expected at least one back into src/lib.rs"
    );

    let found = locations
        .iter()
        .any(|loc| loc.uri.ends_with("src/lib.rs") && loc.range.start.line == 0);
    assert!(
        found,
        "expected at least one Location ending in `src/lib.rs` on line 0; \
         got: {locations:#?}"
    );
}
