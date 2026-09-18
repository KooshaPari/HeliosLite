//! End-to-end test for the `forge_lsp::commands` surface — the integration
//! boundary a parent binary dispatches into.
//!
//! This drives `run_command` (the real public entry point, which builds a
//! `Server` via `Server::with_defaults`) against a real `rust-analyzer`
//! subprocess, so it exercises the same path a CLI user would take:
//! `Command::Definition { workspace, path, line, col }` -> `Server` ->
//! language server -> formatted locations.
//!
//! Skips (returns success) when neither language server is on `PATH`, mirroring
//! `e2e_rust_analyzer`.

use std::path::PathBuf;
use std::time::Duration;

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
    let path = String::from_utf8(out.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    let check = std::process::Command::new(&path)
        .arg("--version")
        .output()
        .ok()?;
    check.status.success().then_some(path)
}

#[test]
fn definition_command_reports_a_location() {
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

    // rust-analyzer loads the project asynchronously and answers requests while
    // it is still loading, so retry until it produces a location.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let outcome = match run_command(&cmd) {
            Ok(Some(out)) => out,
            Ok(None) => "<no output>".to_string(),
            Err(e) => format!("error: {e}"),
        };
        if outcome.contains("src/lib.rs") {
            eprintln!("definition command output:\n{outcome}");
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "forge_lsp CLI definition command produced no location within 60s; last: {outcome}"
            );
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
