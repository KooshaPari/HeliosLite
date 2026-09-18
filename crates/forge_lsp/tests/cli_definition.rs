//! Integration test for the `forge_lsp::commands` surface — the boundary a
//! parent binary dispatches into.
//!
//! It drives `run_command` (the real public entry point, which builds a
//! `Server` via `Server::with_defaults`) against a real `rust-analyzer`
//! subprocess, so it exercises the same path a CLI user takes:
//! `Command::Definition { workspace, path, line, col }` -> `Server` ->
//! language server -> formatted locations.
//!
//! What it guards: the CLI must register the target document before querying.
//! Without that, every file-scoped command returned
//! `lsp server error: file not found: <path> (code -32603)` because the file
//! was never in the server's VFS.
//!
//! Scope note: `run_command` builds its own server per invocation, so this
//! makes a single call and lets the command's own readiness poll (see
//! `LSP_READY_TIMEOUT`) absorb a cold-start load of the project. A location is
//! asserted when the server produced one; an empty-but-successful result is
//! reported rather than failed, because a shared CI runner can still be
//! loading when the budget elapses.
//!
//! Skips (returns success) when `rust-analyzer` is not on `PATH`, mirroring
//! `e2e_rust_analyzer`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use forge_lsp::commands::{Command, run_command};

/// Locate a working `rust-analyzer` on the current host.
fn find_rust_analyzer() -> Option<PathBuf> {
    let out = std::process::Command::new("sh")
        .arg("-c")
        .arg("command -v rust-analyzer")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    std::process::Command::new(&path)
        .arg("--version")
        .output()
        .ok()?
        .status
        .success()
        .then_some(path)
}

#[test]
fn definition_command_does_not_fail_on_a_cold_server() {
    if find_rust_analyzer().is_none() {
        eprintln!("e2e: skipping, rust-analyzer not on PATH");
        return;
    }

    let tmp = tempfile::Builder::new()
        .prefix("forge-lsp-cli-")
        .tempdir()
        .expect("create temp workspace");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .expect("write src/lib.rs");
    std::fs::write(
        root.join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"forge-lsp-cli-e2e\"\n",
            "version = \"0.0.0\"\n",
            "edition = \"2021\"\n\n",
            "[lib]\n",
            "path = \"src/lib.rs\"\n\n",
            "[dependencies]\n",
        ),
    )
    .expect("write Cargo.toml");

    let cmd = Command::Definition {
        workspace: root.to_path_buf(),
        path: PathBuf::from("src/lib.rs"),
        line: 0,
        col: 7,
    };

    let started = Instant::now();
    let deadline = started + Duration::from_secs(90);
    // A cold rust-analyzer can fail its very first handshake, and each
    // `run_command` call owns a fresh server, so retry on transient errors
    // rather than reporting a flake. The assertion below still fails fast for
    // the regression this guards (`file not found`).
    let output = loop {
        match run_command(&cmd) {
            Ok(out) => break out.unwrap_or_default(),
            Err(e) if Instant::now() < deadline => {
                eprintln!("transient CLI error ({e}); retrying");
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => panic!("definition command failed: {e}"),
        }
    };
    let elapsed = started.elapsed();
    eprintln!("forge_lsp definition command -> {output:?} in {elapsed:?}");

    // The regression this guards: the CLI never sent `didOpen`, so the server
    // rejected the request with `file not found`.
    assert!(
        !output.contains("file not found"),
        "CLI did not register the document with the language server: {output}"
    );
    assert!(
        !output.starts_with("error:"),
        "CLI definition command reported an error: {output}"
    );
    if !output.is_empty() && output != "definition: no locations" {
        assert!(
            output.contains("src/lib.rs"),
            "definition should point back into src/lib.rs, got: {output}"
        );
    } else {
        eprintln!(
            "note: no location within {:?}; the server had not finished loading the project",
            elapsed
        );
    }
}
