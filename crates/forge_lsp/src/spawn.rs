//! Portable subprocess construction.
//!
//! On Windows, command-line tools installed by npm (e.g.
//! `typescript-language-server`) and other package managers ship as
//! `.cmd`/`.bat` shims. `CreateProcessW` — and therefore
//! `std::process::Command` — cannot launch a batch file directly, so a bare
//! `Command::new("typescript-language-server")` fails with
//! `program not found` even though `where` (PATHEXT-aware) finds the shim.
//!
//! [`build_command`] reproduces what a Windows shell does: it resolves a
//! bare name through `PATH` + `PATHEXT` and, when the target is a batch
//! file, runs it via `cmd /C`. Native executables and Unix hosts are passed
//! through unchanged, so Unix behaviour is byte-identical to a direct
//! `Command::new`.

use std::path::Path;
use std::process::Command;

/// Build a [`Command`] that can launch `program` with `args` on this host.
///
/// * **Unix** — always `Command::new(program)` with `args` appended.
/// * **Windows** — batch-file shims (`.cmd`/`.bat`) are wrapped in
///   `cmd /C` so they execute; native executables are passed through
///   unchanged.
pub fn build_command(program: &Path, args: &[&str]) -> Command {
    #[cfg(windows)]
    {
        if let Some(shim) = resolve_windows_batch_shim(program) {
            let mut cmd = Command::new("cmd");
            cmd.arg("/C").arg(shim);
            cmd.args(args);
            return cmd;
        }
    }
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd
}

/// Resolve `program` to a batch-file shim on Windows, if it is one.
///
/// Returns `Some(path)` only when the resolved target is a `.cmd`/`.bat`
/// file that must be launched through `cmd /C`. Native executables (or
/// names that do not resolve) return `None`, so the caller falls back to a
/// direct `Command::new` — which is exactly what Windows would pick.
#[cfg(windows)]
fn resolve_windows_batch_shim(program: &Path) -> Option<std::path::PathBuf> {
    fn is_batch(p: &Path) -> bool {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
    }

    // A path or name that already carries an extension: wrap only batch files.
    if program.extension().is_some() {
        return if is_batch(program) {
            Some(program.to_path_buf())
        } else {
            None
        };
    }

    // Bare name: search `PATH` in `PATHEXT` order. The first hit wins, so a
    // native `.exe` (which `Command` launches directly) correctly yields
    // `None`, while an npm `.cmd` shim yields `Some`.
    let path_var = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let exts: Vec<&str> = pathext.split(';').filter(|s| !s.is_empty()).collect();
    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{}{ext}", program.display()));
            if candidate.is_file() {
                return if is_batch(&candidate) {
                    Some(candidate)
                } else {
                    None
                };
            }
        }
    }
    None
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    /// On Unix the command must be a direct spawn — same program, same args.
    #[test]
    fn build_command_preserves_program_and_args_on_unix() {
        let cmd = build_command(Path::new("rust-analyzer"), &["--stdio"]);
        assert_eq!(cmd.get_program(), OsStr::new("rust-analyzer"));
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("--stdio")]
        );
    }
}
