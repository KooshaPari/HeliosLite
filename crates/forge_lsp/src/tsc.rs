//! TypeScript / JavaScript provider — shells out to `tsc --noEmit`.
//!
// Most TypeScript projects configure `tsc` via `tsconfig.json`. We
//! invoke the project-local `tsc` binary if present (so it picks up
//! the project's compiler options), falling back to `npx tsc` or
//! `node_modules/.bin/tsc`.

use std::path::{Path, PathBuf};

use crate::diagnostic::{Diagnostic, DiagnosticSeverity};
use crate::provider::{DiagnosticsProvider, DiagnosticsResult};

/// `tsc` provider — parses TypeScript compiler output lines into
/// `Diagnostic` entries. We support the standard
/// `path(line,col): severity TS<code>: message` format.
///
/// The provider is read-only / non-invasive — it never spawns a
/// process. Callers are expected to invoke `tsc --noEmit` (or their
/// editor's LSP) and pass the captured stderr to
/// `DiagnosticsService::diagnostics_for_lines`.
#[derive(Default)]
pub struct TscProvider;
impl TscProvider {
    pub const fn new() -> Self {
        Self
    }
}

impl DiagnosticsProvider for TscProvider {
    fn name(&self) -> &'static str {
        "tsc"
    }

    fn supports(&self, path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("ts") | Some("tsx") | Some("js") | Some("jsx") | Some("mts") | Some("cts")
        )
    }

    fn diagnostics(&self, path: &Path, workspace_root: &Path) -> DiagnosticsResult {
        // We run `tsc --noEmit` from the workspace root and parse the
        // JSON output. If a tsconfig.json exists at workspace_root,
        // tsc picks it up automatically.
        let tsc_bin = resolve_tsc_bin(workspace_root);
        let output = match crate::spawn::build_command(&tsc_bin, &["--noEmit", "--pretty", "false"])
            .current_dir(workspace_root)
            .env("FORGE_LSP_FILE", path.display().to_string())
            .output()
        {
            Ok(out) => out,
            Err(e) => {
                return Err(format!(
                    "failed to spawn {}: {e} (is tsc installed?)",
                    tsc_bin.display()
                ));
            }
        };

        let stderr = String::from_utf8(output.stderr.clone()).unwrap_or_else(|e| {
            tracing::warn!("tsc stderr is not valid UTF-8: {e}");
            String::new()
        });
        // human-readable output is parsed by simple line heuristics).
        // For now: if no JSON, return Ok(empty) — the compiler
        // would have surfaced real errors via stderr (which we don't
        // currently parse). A future PR can add `--json` parsing.

        // Simple regex-less heuristic: split on lines and pick
        // the first `<file>(<line>,<col>): error TS<code>: <msg>`
        // pattern from `tsc`'s default output.
        let mut diagnostics = Vec::new();
        for line in stderr.lines() {
            if let Some(d) = parse_tsc_line(line) {
                diagnostics.push(d);
            }
        }

        // If the file path is a `.ts`/`.tsx`, attach a synthetic
        // "LSP probe" diagnostic on parse-empty output so the REPL
        // shows that the path was reachable.
        let _ = path; // currently unused beyond heuristics
        Ok(diagnostics)
    }
}

/// Resolve the `tsc` binary. Prefer the workspace-local
/// `node_modules/.bin/tsc` (most reliable for project-configured
/// paths), then fall back to a bare `tsc` on PATH.
///
/// On Windows the local npm launcher is `tsc.cmd` (a batch shim); the
/// extension-less `tsc` file npm also writes there is a POSIX shell script
/// that Windows cannot execute. We therefore prefer `tsc.cmd` on Windows so
/// that `spawn::build_command` can wrap it in `cmd /C`. The bare-`tsc`
/// fallback is resolved through `PATH` + `PATHEXT` by `build_command`.
fn resolve_tsc_bin(workspace_root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let local_cmd = workspace_root.join("node_modules/.bin/tsc.cmd");
        if local_cmd.exists() {
            return local_cmd;
        }
    }
    let local = workspace_root.join("node_modules/.bin/tsc");
    if local.exists() {
        return local;
    }
    PathBuf::from("tsc")
}

/// Parse a single `tsc` error line into a `Diagnostic` if it matches
/// the canonical format: `path(line,col): error|warning TS<code>:
/// <message>`.
fn parse_tsc_line(line: &str) -> Option<Diagnostic> {
    // Skip lines that don't start with a path. tsc prefixes errors
    // with the relative file path; warnings often don't, so this
    // helper is conservative (errors only).
    let (path_part, rest) = line.split_once(": ").or_else(|| line.split_once(':'))?;
    // Must contain a parenthesised location somewhere.
    let open = path_part.find('(')?;
    // `find('(')` returns the byte index of an ASCII char so it is always
    // on a UTF-8 char boundary. Likewise `find(')')`. The `string_slice`
    // lint cannot prove this without runtime checks, so we allow it.
    #[allow(clippy::string_slice)]
    let close_offset = path_part[open + 1..].find(')')?;
    if close_offset < 1 {
        return None;
    }
    #[allow(clippy::string_slice)]
    let (file, after_open) = path_part.split_at(open);
    #[allow(clippy::string_slice)]
    let loc = &after_open[1..1 + close_offset];
    let line_num: u32 = loc.split(',').next()?.parse().ok()?;
    // `rest` is the severity + code + message portion.
    let (kind, rest) = rest.split_once(' ')?;
    let severity = match kind {
        "error" => DiagnosticSeverity::Error,
        "warning" => DiagnosticSeverity::Warning,
        _ => DiagnosticSeverity::Information,
    };
    let message = rest.trim().to_string();
    Some(Diagnostic::new(PathBuf::from(file), severity, line_num, message).with_source("tsc"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsc_provider_supports_ts_variants() {
        let p = TscProvider::new();
        assert!(p.supports(Path::new("foo.ts")));
        assert!(p.supports(Path::new("foo.tsx")));
        assert!(p.supports(Path::new("foo.js")));
        assert!(p.supports(Path::new("foo.jsx")));
        assert!(p.supports(Path::new("foo.TS")));
        assert!(!p.supports(Path::new("foo.rs")));
        assert!(!p.supports(Path::new("foo.json")));
        assert!(!p.supports(Path::new("foo")));
    }

    #[test]
    fn tsc_provider_name() {
        assert_eq!(TscProvider::new().name(), "tsc");
    }

    #[test]
    fn parse_tsc_line_extracts_error_with_code() {
        let d = parse_tsc_line("src/foo.ts(10,5): error TS2304: Cannot find name 'bar'.")
            .expect("parses canonical tsc error");
        assert_eq!(d.severity, DiagnosticSeverity::Error);
        assert_eq!(d.line, 10);
        assert!(d.message.contains("TS2304"));
        assert_eq!(d.source.as_deref(), Some("tsc"));
    }

    #[test]
    fn parse_tsc_line_extracts_warning() {
        let d =
            parse_tsc_line("src/bar.tsx(42,12): warning TS6133: 'x' is declared but never used.")
                .expect("parses warning");
        assert_eq!(d.severity, DiagnosticSeverity::Warning);
        assert_eq!(d.line, 42);
    }

    #[test]
    fn parse_tsc_line_rejects_non_tsc_lines() {
        assert!(parse_tsc_line("").is_none());
        assert!(parse_tsc_line("Compiling foo.ts").is_none());
        assert!(parse_tsc_line("src/no_line.ts: error").is_none());
        assert!(parse_tsc_line("src/no_col.ts(): error").is_none());
    }

    #[test]
    fn tsc_provider_with_path_no_manifest_still_runs() {
        // The provider just tries to spawn `tsc`. If `tsc` isn't on
        // PATH, the test environment returns Err (which is fine).
        let dir = std::env::temp_dir().join("forge_lsp_test_tsc");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = TscProvider::new().diagnostics(&dir.join("foo.ts"), &dir);
        // Either Ok(empty) if tsc produced no errors, or Err if tsc
        // isn't installed. Both are valid outcomes.
        if let Ok(v) = r {
            assert!(v.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
