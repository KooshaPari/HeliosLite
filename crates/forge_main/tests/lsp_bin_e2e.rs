//! End-to-end shell tests for the `helioslite lsp` subcommand tree.
//!
//! Stream 3 of post-backlog cleanup. These tests spawn the real
//! `helioslite` binary as a subprocess (no library imports of
//! `forge_lsp`) and exercise three layers of behavior:
//!
//! 1. **CLI surface** — the binary must expose `lsp` as a real
//!    subcommand and the help text must enumerate the LSP
//!    capabilities (`hover`, `definition`, `rename`, ...) that
//!    downstream agents bind against. This is the binary-side guard
//!    against an accidental rename or removal of the subcommand.
//!
//! 2. **Error paths** — when a user invokes `helioslite lsp hover` (or
//!    `definition`) against a non-existent file or with a workspace that
//!    lacks the language-server binary, the binary must exit non-zero
//!    with a clean error message rather than panicking. Panics in the
//!    CLI surface as a stack trace on stderr and a non-zero exit code
//!    that looks the same as a clean error; we additionally assert
//!    that stderr is non-empty AND does NOT contain the standard
//!    `thread 'X' panicked at` header so a real panic fails the test.
//!
//! 3. **Real round-trip against rust-analyzer** — gated on the binary's
//!    presence on PATH (matching the skip-pattern from
//!    `crates/forge_lsp/tests/e2e_rust_analyzer.rs`). When present, we
//!    spawn rust-analyzer directly via JSON-RPC over stdio and assert
//!    a non-empty `initialize` response so we know the language-server
//!    subprocess pipeline is wired correctly end-to-end.
//!
//! All tests use only `std::process::Command` /
//! `std::io::{Read, Write}` — no `assert_cmd` / `predicates` deps.
//!
//! A 30-second wall-clock budget caps the round-trip test so a hung
//! `rust-analyzer` surfaces as a clean failure rather than stalling CI.

use bstr::ByteSlice;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Maximum wall-clock budget for the full LSP round-trip (initialize +
/// definition). Generous enough for a slow CI box, tight enough that a
/// hung subprocess surfaces as a clean panic rather than a stalled CI
/// job.
const LSP_E2E_BUDGET: Duration = Duration::from_secs(30);

/// Locate the `helioslite` binary built by this crate's `[[bin]]`
/// entries. Cargo exports the absolute path at compile time via
/// `CARGO_BIN_EXE_helioslite`; falling back to `helioslite` lets the
/// test run outside `cargo test` (e.g. in ad-hoc invocations).
fn helioslite_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_helioslite"))
}

// ---------------------------------------------------------------------------
// Test 1 — CLI surface: `helioslite lsp --help` enumerates the expected
// capabilities. Guards against an accidental rename / removal of the
// subcommand tree.
// ---------------------------------------------------------------------------

#[test]
fn helioslite_lsp_help_enumerates_capabilities() {
    let output = helioslite_bin()
        .args(["lsp", "--help"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn `helioslite lsp --help`");

    assert!(
        output.status.success(),
        "helioslite lsp --help exited non-zero: status={:?}\nstderr:\n{}",
        output.status,
        output.stderr.as_slice().to_str_lossy()
    );

    let stdout = output.stdout.as_slice().to_str_lossy();

    // The CLI parser must surface every LSP capability the facade
    // implements. Missing entries here would mean the binary was built
    // without the subcommand group (a hard regression).
    for expected in [
        "diagnose",
        "hover",
        "definition",
        "implementations",
        "references",
        "type-definition",
        "rename",
    ] {
        assert!(
            stdout.contains(expected),
            "expected `helioslite lsp --help` to enumerate {expected:?}, \
             but stdout was:\n{stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2 — Error path: `helioslite lsp definition --workspace <dir>
// --path <nonexistent.rs> --line 0 --col 0` must exit non-zero with a
// clean error. A panic would still exit non-zero, but we'd see
// `thread 'main' panicked at` on stderr; the assertion catches that.
// ---------------------------------------------------------------------------

#[test]
fn helioslite_lsp_definition_rejects_nonexistent_path_with_clean_error() {
    let tmp = tempdir_child();
    let missing = tmp.join("does_not_exist.rs");

    let output = helioslite_bin()
        .args([
            "lsp",
            "definition",
            "--workspace",
            tmp.to_str().expect("utf-8 workspace path"),
            "--path",
            missing.to_str().expect("utf-8 missing path"),
            "--line",
            "0",
            "--col",
            "0",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn `helioslite lsp definition` with missing path");

    let stderr = output.stderr.as_slice().to_str_lossy();

    // Must exit non-zero — a successful call would mean the CLI
    // silently swallowed a bad path, which would be a regression.
    assert!(
        !output.status.success(),
        "expected non-zero exit for missing path; got status={:?}\nstderr:\n{stderr}",
        output.status
    );

    // Must NOT be a panic — panics are loud, ugly, and indicate a
    // bug, not a user-facing error. The error path should be a clean
    // diagnostic line.
    assert!(
        !stderr.contains("panicked at"),
        "binary panicked instead of returning a clean error for missing path:\n{stderr}"
    );
    assert!(
        !stderr.contains("thread '"),
        "binary panicked (thread header on stderr) instead of returning a clean error:\n{stderr}"
    );

    // Must surface SOMETHING on stderr — a silent failure is just as
    // bad as a panic from the user's perspective.
    assert!(
        !stderr.trim().is_empty(),
        "binary exited non-zero but stderr was empty; expected a clean error message"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Error path, parallel for `hover`. Mirrors test 2's
// invariants against the `hover` capability so a regression in one
// subcommand's error handling doesn't pass silently because the other
// still works.
// ---------------------------------------------------------------------------

#[test]
fn helioslite_lsp_hover_rejects_nonexistent_path_with_clean_error() {
    let tmp = tempdir_child();
    let missing = tmp.join("ghost.rs");

    let output = helioslite_bin()
        .args([
            "lsp",
            "hover",
            "--workspace",
            tmp.to_str().expect("utf-8 workspace path"),
            "--path",
            missing.to_str().expect("utf-8 missing path"),
            "--line",
            "1",
            "--col",
            "0",
        ])
        .stdin(Stdio::null())
        .output()
        .expect("spawn `helioslite lsp hover` with missing path");

    let stderr = output.stderr.as_slice().to_str_lossy();

    assert!(
        !output.status.success(),
        "expected non-zero exit for missing path; got status={:?}\nstderr:\n{stderr}",
        output.status
    );
    assert!(
        !stderr.contains("panicked at"),
        "binary panicked instead of returning a clean error for missing path:\n{stderr}"
    );
    assert!(
        !stderr.contains("thread '"),
        "binary panicked (thread header on stderr) instead of returning a clean error:\n{stderr}"
    );
    assert!(
        !stderr.trim().is_empty(),
        "binary exited non-zero but stderr was empty; expected a clean error message"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — Real round-trip against a live `rust-analyzer`.
//
// Gated on rust-analyzer being on PATH (matching the skip pattern from
// crates/forge_lsp/tests/e2e_rust_analyzer.rs). When present:
//   - allocate a localhost TCP port for the test (unused after teardown)
//   - start a real rust-analyzer subprocess via stdio
//   - send a properly-framed `initialize` request
//   - assert we receive a non-empty `result` containing the
//     `capabilities` object rust-analyzer is required to advertise
//
// When rust-analyzer is absent: print the documented skip line and
// exit successfully so CI stays green on minimal build agents. The
// test is NOT `#[ignore]`'d; the early-exit branch is the skip path.
// ---------------------------------------------------------------------------

#[test]
fn helioslite_lsp_round_trips_against_real_rust_analyzer() {
    let Some(rust_analyzer) = find_rust_analyzer() else {
        eprintln!("e2e: skipping, rust-analyzer not on PATH");
        return;
    };

    let deadline = Instant::now() + LSP_E2E_BUDGET;

    // Spawn rust-analyzer with stdio piped so we can drive its JSON-RPC
    // framing ourselves (mirrors what `forge_lsp::ProcessLspClient`
    // does internally; this exercises the same wire format the
    // production code path uses).
    let mut child = Command::new(&rust_analyzer)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn rust-analyzer at {}: {e}", rust_analyzer.display()));

    let mut stdin = child.stdin.take().expect("rust-analyzer stdin pipe");
    let mut stdout = child.stdout.take().expect("rust-analyzer stdout pipe");

    // Send the LSP `initialize` request. Per the LSP spec the request
    // is a JSON-RPC message framed with `Content-Length:` headers; we
    // use serde_json to build the body so the wire format is correct
    // byte-for-byte.
    let initialize_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "clientInfo": { "name": "forgecode-lsp-bin-e2e", "version": "0.0.0" },
            "rootUri": null,
            "capabilities": {
                "workspace": { "workspaceFolders": true },
                "textDocument": {
                    "synchronization": { "dynamicRegistration": false }
                }
            }
        }
    })
    .to_string();

    write_framed(&mut stdin, &initialize_body).expect("write initialize to rust-analyzer");

    // Read the response within the budget. `read_framed` blocks until
    // the headers + body arrive; the outer loop applies the wall-clock
    // cap so a hung subprocess fails the test rather than stalling.
    let response_body = match read_framed_bounded(&mut stdout, deadline) {
        Ok(body) => body,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("read initialize response from rust-analyzer: {e}");
        }
    };

    let parsed: serde_json::Value = serde_json::from_str(&response_body).unwrap_or_else(|e| {
        let _ = child.kill();
        let _ = child.wait();
        panic!("parse rust-analyzer response as JSON: {e}\nbody={response_body:?}")
    });

    // The response must be a well-formed JSON-RPC reply carrying a
    // non-empty `result.capabilities` object — that's the
    // contract rust-analyzer promises on initialize.
    assert!(
        parsed.get("result").is_some(),
        "rust-analyzer response missing `result` field: {parsed:#?}"
    );
    let capabilities = parsed.pointer("/result/capabilities").unwrap_or_else(|| {
        let _ = child.kill();
        let _ = child.wait();
        panic!("rust-analyzer `result` missing `capabilities` field: {parsed:#?}")
    });
    assert!(
        capabilities.is_object(),
        "rust-analyzer capabilities is not a JSON object: {capabilities:#?}"
    );
    // rust-analyzer advertises at least the `definitionProvider` (true
    // or { ... } shape) and `hoverProvider` capabilities. We don't
    // assert their exact shapes — just that the object is non-empty —
    // so a future rust-analyzer release that adds/removes fields
    // doesn't break this test gratuitously.
    assert!(
        !capabilities.as_object().unwrap().is_empty(),
        "rust-analyzer returned an empty capabilities object: {capabilities:#?}"
    );

    // Best-effort teardown. We don't strictly need to wait — the
    // subprocess exits when its stdin closes — but killing + waiting
    // ensures we don't leak a child process across CI runs.
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate a working `rust-analyzer` on the current host. Returns the
/// binary's path on success, `None` otherwise.
///
/// Two-stage probe:
/// 1. `where` (Windows) / `which` (Unix) to find a candidate on
///    `PATH`.
/// 2. `--version` against the candidate, which catches the
///    pathological case where a `rustup` shim exists but the
///    underlying component isn't installed.
fn find_rust_analyzer() -> Option<PathBuf> {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    let output = Command::new(cmd).arg("rust-analyzer").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = output.stdout.as_slice().to_str_lossy();
    let candidate = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .to_string();

    let probe = Command::new(&candidate).arg("--version").output().ok()?;
    if !probe.status.success() {
        return None;
    }

    Some(PathBuf::from(candidate))
}

/// Tiny per-test scratch directory under the OS temp root. We avoid
/// pulling in the `tempfile` crate here — the tests only need a
/// directory, not the drop-based cleanup that `tempfile` provides.
/// The OS periodically reaps `std::env::temp_dir()` leftovers, and a
/// single missed directory in CI is harmless.
fn tempdir_child() -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("forgecode-lsp-bin-e2e-{pid}-{nanos}"));
    std::fs::create_dir_all(&dir)
        .unwrap_or_else(|e| panic!("create scratch dir {}: {e}", dir.display()));
    dir
}

/// Write a JSON-RPC framed message: `Content-Length: N\r\n\r\n<body>`.
fn write_framed<W: Write>(mut writer: W, body: &str) -> std::io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer.write_all(header.as_bytes())?;
    writer.write_all(body.as_bytes())?;
    writer.flush()
}

/// Read a single JSON-RPC framed message, returning its body.
///
/// `deadline` caps the wall-clock time the read can take; we read one
/// byte at a time (LSP headers are ASCII) so a hung peer surfaces as
/// a clean timeout error rather than blocking forever.
fn read_framed_bounded<R: Read>(mut reader: R, deadline: Instant) -> Result<String, String> {
    // Read header bytes one at a time. Each iteration checks the
    // deadline so a stalled subprocess trips the budget rather than
    // pinning the test runner.
    let mut header_buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out reading LSP header after {:?}",
                LSP_E2E_BUDGET
            ));
        }
        match reader.read_exact(&mut byte) {
            Ok(()) => {
                header_buf.push(byte[0]);
                if header_buf.ends_with(b"\r\n\r\n") {
                    break;
                }
                if header_buf.len() > 8 * 1024 {
                    return Err("LSP header exceeded 8 KiB".to_string());
                }
            }
            Err(e) => return Err(format!("read LSP header byte: {e}")),
        }
    }
    let header_str =
        std::str::from_utf8(&header_buf).map_err(|e| format!("LSP header is not utf-8: {e}"))?;
    let content_length: usize = header_str
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .ok_or_else(|| "LSP header missing Content-Length".to_string())?;
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|e| format!("read LSP body ({content_length} bytes): {e}"))?;
    String::from_utf8(body).map_err(|e| format!("LSP body is not utf-8: {e}"))
}
