//! Portable subprocess construction.
//!
//! On Windows, command-line tools installed by npm (e.g.
//! `typescript-language-server`) and other package managers ship as
//! `.cmd`/`.bat` shims. `CreateProcessW` — and therefore
//! `std::process::Command` — cannot launch a batch file directly, so a bare
//! `Command::new("typescript-language-server")` fails with `program not
//! found` even though `where` (PATHEXT-aware) finds the shim.
//!
//! The canonical implementation lives in
//! [`forge_pheno_shell::subprocess::build_command`] — a single copy shared by
//! both sides of the LSP/infra crate boundary (`forge_lsp` language servers /
//! `tsc`, and `forge_infra` MCP stdio servers). This module re-exports it so
//! the historical `forge_lsp::spawn::build_command` path used by
//! `lsp_client`, `tsc`, and the e2e `--version` probes stays stable.

pub use forge_pheno_shell::subprocess::build_command;
