//! `forge lsp` — CLI front-ends for `forge_lsp::Server`.
//!
//! Each sub-command constructs a server for the current working
//! directory (spawning rust-analyzer and typescript-language-server),
//! runs a single language query, and renders the result to stdout.
//!
//! Server construction failures (e.g. a language server is not on
//! `PATH`) surface as an `Err`, which the caller prints to stderr and
//! turns into a non-zero exit — never a panic.

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use forge_lsp::lsp_client::Position;
use forge_lsp::{Location, ReferencesOptions, Server};
use forge_main::{LspCommandGroup, LspSubcommand};

/// Run a single `forge lsp` subcommand against the current directory.
pub fn run(command: &LspCommandGroup) -> Result<String> {
    let workspace_root = std::env::current_dir()
        .map_err(|err| anyhow!("failed to determine current directory: {err}"))?;

    // Construct the server, degrading gracefully if a language server
    // binary is unavailable rather than panicking.
    let server = Server::with_defaults(&workspace_root)
        .map_err(|err| anyhow!("failed to start language servers: {err}"))?;

    match &command.command {
        LspSubcommand::Diagnostics { path } => run_diagnostics(&server, path),
        LspSubcommand::Hover { path, line, character } => {
            run_hover(&server, path, position(*line, *character))
        }
        LspSubcommand::Definition { path, line, character } => {
            let locs = server
                .definition(path, position(*line, *character))
                .map_err(|err| anyhow!("definition: {err}"))?;
            Ok(render_locations(&locs, "definition"))
        }
        LspSubcommand::Complete { path, line, character, trigger } => {
            run_complete(&server, path, position(*line, *character), *trigger)
        }
        LspSubcommand::Implementation { path, line, character } => {
            let locs = server
                .implementation(path, position(*line, *character))
                .map_err(|err| anyhow!("implementation: {err}"))?;
            Ok(render_locations(&locs, "implementation"))
        }
        LspSubcommand::References { path, line, character, include_declaration } => {
            let locs = server
                .references(
                    path,
                    position(*line, *character),
                    ReferencesOptions { include_declaration: *include_declaration },
                )
                .map_err(|err| anyhow!("references: {err}"))?;
            Ok(render_locations(&locs, "references"))
        }
        LspSubcommand::TypeDefinition { path, line, character } => {
            let locs = server
                .type_definition(path, position(*line, *character))
                .map_err(|err| anyhow!("type-definition: {err}"))?;
            Ok(render_locations(&locs, "type-definition"))
        }
        LspSubcommand::Rename { path, line, character, new_name } => {
            run_rename(&server, path, position(*line, *character), new_name)
        }
    }
}

fn position(line: u32, character: u32) -> Position {
    Position { line, character }
}

fn run_diagnostics(server: &Server, path: &Path) -> Result<String> {
    let diagnostics = server.diagnostics_for_path(path);
    if diagnostics.is_empty() {
        return Ok("No diagnostics.\n".to_string());
    }
    let mut out = String::new();
    for d in diagnostics {
        let source = d.source.as_deref().unwrap_or("???");
        writeln_into(
            &mut out,
            &format!(
                "{}:{} [{}] {} ({source})",
                d.file.display(),
                d.line,
                d.severity.as_str(),
                d.message
            ),
        );
    }
    Ok(out)
}

fn run_hover(server: &Server, path: &Path, position: Position) -> Result<String> {
    match server
        .hover(path, position)
        .map_err(|err| anyhow!("hover: {err}"))?
    {
        Some(hover) => Ok(format!("{}\n", hover.contents)),
        None => Ok("No hover information.\n".to_string()),
    }
}

fn run_complete(
    server: &Server,
    path: &Path,
    position: Position,
    trigger: Option<char>,
) -> Result<String> {
    let items = server
        .complete(path, position, trigger)
        .map_err(|err| anyhow!("complete: {err}"))?;
    if items.is_empty() {
        return Ok("No completions.\n".to_string());
    }
    let mut out = String::new();
    for item in items {
        let kind = item
            .kind
            .as_ref()
            .map(|k| format!(" ({k:?})"))
            .unwrap_or_default();
        writeln_into(&mut out, &format!("{}{}", item.label, kind));
    }
    Ok(out)
}

fn run_rename(server: &Server, path: &Path, position: Position, new_name: &str) -> Result<String> {
    match server
        .rename(path, position, new_name)
        .map_err(|err| anyhow!("rename: {err}"))?
    {
        Some(edit) => {
            let mut out = String::new();
            for change in &edit.document_changes {
                for te in &change.edits {
                    writeln_into(
                        &mut out,
                        &format!(
                            "{}:{}:{} -> {}",
                            change.text_document.uri, 0, 0, te.new_text
                        ),
                    );
                }
            }
            if out.is_empty() {
                bail!("rename: no changes produced");
            }
            Ok(out)
        }
        None => Ok("No rename result (symbol cannot be renamed).\n".to_string()),
    }
}

/// Render a `Vec<Location>` (definition / implementation / references /
/// type-definition output) as `uri:line:character` lines.
fn render_locations(locations: &[Location], label: &str) -> String {
    if locations.is_empty() {
        return format!("No {label} found.\n");
    }
    let mut out = String::new();
    for loc in locations {
        writeln_into(
            &mut out,
            &format!(
                "{}:{}:{}",
                loc.uri, loc.range.start.line, loc.range.start.character
            ),
        );
    }
    out
}

fn writeln_into(out: &mut String, line: &str) {
    use std::fmt::Write;
    let _ = writeln!(out, "{line}");
}
