//! Minimal CLI surface for the `forge_lsp` facade.
//!
//! Exposes the language-aware `Server` capability set as a leaf
//! subcommand tree the parent `helioslite` / `forge` binary invokes
//! when a user runs `forge lsp <subcommand>`:
//!
//! - `diagnose <PATH>`            — run rustc/tsc diagnostics on one file.
//! - `hover <PATH> L C`           — hover text at line `L`, col `C`.
//! - `definition <PATH> L C`      — goto-definition locations.
//! - `implementations <PATH> L C` — implementation locations (P2.3.2).
//! - `references <PATH> L C`      — reference locations (P2.3.2).
//! - `type-definition <PATH> L C` — typeDefinition locations (P2.3.2).
//! - `rename <PATH> L C <NAME>`   — preview a workspace-rename edit.
//!
//! Each command lazily builds a `Server` on the supplied workspace
//! root. Construction requires `rust-analyzer` and
//! `typescript-language-server` on `$PATH`; if either is missing the
//! subprocess spawn is surfaced as a clean CLI error rather than a
//! panic.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use thiserror::Error;

use crate::definition::Location;
use crate::lsp_client::Position;
use crate::server::Server;

/// Errors surfaced from the LSP CLI.
#[derive(Debug, Error)]
pub enum CliError {
    /// The receiver/spawn failed — e.g. a language server is missing.
    #[error("lsp: {0}")]
    Server(String),
    /// A capability request itself failed.
    #[error("lsp {capability}: {source}")]
    Capability {
        /// Which capability errored.
        capability: &'static str,
        /// Underlying error text.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Top-level LSP CLI.
#[derive(Parser, Debug)]
#[command(
    name = "lsp",
    about = "Language-server capabilities (diagnostics, hover, definitions, rename)",
    version
)]
pub struct Cli {
    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// LSP subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Run rustc/tsc diagnostics on a single file.
    Diagnose {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file to diagnose.
        path: PathBuf,
    },
    /// Hover text at a position.
    Hover {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file.
        path: PathBuf,
        /// 0-based line.
        line: u32,
        /// 0-based character column.
        col: u32,
    },
    /// Goto-definition locations at a position.
    Definition {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file.
        path: PathBuf,
        /// 0-based line.
        line: u32,
        /// 0-based character column.
        col: u32,
    },
    /// Implementation locations at a position.
    Implementations {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file.
        path: PathBuf,
        /// 0-based line.
        line: u32,
        /// 0-based character column.
        col: u32,
    },
    /// Reference locations at a position.
    References {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file.
        path: PathBuf,
        /// 0-based line.
        line: u32,
        /// 0-based character column.
        col: u32,
    },
    /// typeDefinition locations at a position.
    TypeDefinition {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file.
        path: PathBuf,
        /// 0-based line.
        line: u32,
        /// 0-based character column.
        col: u32,
    },
    /// Preview a symbol rename at a position.
    Rename {
        /// Workspace (project) root the file belongs to.
        #[arg(long, short = 'w', default_value = ".")]
        workspace: PathBuf,
        /// Path to the file.
        path: PathBuf,
        /// 0-based line.
        line: u32,
        /// 0-based character column.
        col: u32,
        /// New symbol name.
        new_name: String,
    },
}

/// Format a `Location` list as `uri:line:col` lines.
fn format_locations(capability: &str, locs: &[Location]) -> String {
    if locs.is_empty() {
        return format!("{capability}: no locations");
    }
    let mut out = String::new();
    for loc in locs {
        out.push_str(&format!(
            "{}:{}:{}\n",
            loc.uri, loc.range.start.line, loc.range.start.character
        ));
    }
    out
}

fn pos(line: u32, col: u32) -> Position {
    Position { line, character: col }
}

fn build_server(workspace: &Path) -> Result<Server, CliError> {
    Server::with_defaults(workspace).map_err(CliError::Server)
}

/// Run a single subcommand by reference (mirrors the `agileplus`
/// precedent so the parent binary can dispatch without rebuilding
/// [`Cli`]).
pub fn run_command(cmd: &Command) -> Result<Option<String>, CliError> {
    match cmd {
        Command::Diagnose { workspace, path } => {
            let server = build_server(workspace)?;
            let diags = server.diagnostics_for_path(path);
            if diags.is_empty() {
                return Ok(Some("diagnose: no diagnostics".to_string()));
            }
            let mut out = String::new();
            for d in &diags {
                let sev = format!("{:?}", d.severity).to_lowercase();
                let src = d.source.as_deref().unwrap_or("?");
                out.push_str(&format!(
                    "{}:{}:{} {}: {}\n",
                    path.display(),
                    d.line,
                    sev,
                    src,
                    d.message
                ));
            }
            Ok(Some(out))
        }
        Command::Hover { workspace, path, line, col } => {
            let server = build_server(workspace)?;
            match server.hover(path, pos(*line, *col)) {
                Ok(Some(hover)) => Ok(Some(hover.contents)),
                Ok(None) => Ok(Some("hover: no content".to_string())),
                Err(e) => Err(CliError::Capability { capability: "hover", source: Box::new(e) }),
            }
        }
        Command::Definition { workspace, path, line, col } => {
            let server = build_server(workspace)?;
            server
                .definition(path, pos(*line, *col))
                .map(|locs| Some(format_locations("definition", &locs)))
                .map_err(|e| CliError::Capability { capability: "definition", source: Box::new(e) })
        }
        Command::Implementations { workspace, path, line, col } => {
            let server = build_server(workspace)?;
            server
                .implementation(path, pos(*line, *col))
                .map(|locs| Some(format_locations("implementation", &locs)))
                .map_err(|e| CliError::Capability {
                    capability: "implementation",
                    source: Box::new(e),
                })
        }
        Command::References { workspace, path, line, col } => {
            let server = build_server(workspace)?;
            server
                .references(path, pos(*line, *col), Default::default())
                .map(|locs| Some(format_locations("references", &locs)))
                .map_err(|e| CliError::Capability { capability: "references", source: Box::new(e) })
        }
        Command::TypeDefinition { workspace, path, line, col } => {
            let server = build_server(workspace)?;
            server
                .type_definition(path, pos(*line, *col))
                .map(|locs| Some(format_locations("typeDefinition", &locs)))
                .map_err(|e| CliError::Capability {
                    capability: "typeDefinition",
                    source: Box::new(e),
                })
        }
        Command::Rename { workspace, path, line, col, new_name } => {
            if new_name.trim().is_empty() {
                return Err(CliError::Server(
                    "rename: empty target name rejected".to_string(),
                ));
            }
            let server = build_server(workspace)?;
            match server.rename(path, pos(*line, *col), new_name) {
                Ok(Some(edit)) => {
                    let mut out = String::new();
                    out.push_str("rename preview:\n");
                    for change in &edit.document_changes {
                        let uri = &change.text_document.uri;
                        for t in &change.edits {
                            out.push_str(&format!(
                                "{uri}:{}-{} → {:?}\n",
                                t.range.start.line, t.range.end.line, t.new_text
                            ));
                        }
                    }
                    Ok(Some(out))
                }
                Ok(None) => Ok(Some("rename: nothing to rename".to_string())),
                Err(e) => Err(CliError::Capability { capability: "rename", source: Box::new(e) }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_zero_based() {
        let p = pos(3, 12);
        assert_eq!((p.line, p.character), (3, 12));
    }

    #[test]
    fn empty_locations_render_nicely() {
        assert_eq!(
            format_locations("definition", &[]),
            "definition: no locations"
        );
    }

    #[test]
    fn single_location_renders_uri_line_col() {
        let loc = Location {
            uri: "file:///src/lib.rs".to_string(),
            range: crate::lsp_client::Range {
                start: Position { line: 3, character: 7 },
                end: Position { line: 3, character: 12 },
            },
        };
        assert_eq!(
            format_locations("definition", &[loc]),
            "file:///src/lib.rs:3:7\n"
        );
    }
}
