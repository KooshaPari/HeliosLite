//! Documentation surface for environment variables honored by the binary's
//! subcommands.
//!
//! This module is purely declarative. It does **not** read or mutate the
//! process environment — it only describes what each subcommand *will*
//! consult so the operator can `helioslite config schema` and discover the
//! contract without having to grep the source.
//!
//! The schema is intentionally additive and stable: the existing
//! [`ForgeConfig`](crate::ForgeConfig) type is untouched, and downstream
//! consumers that did not know about [`SchemaEntry`] keep compiling.
//!
//! ## Layout
//!
//! A [`SchemaSurface`] groups [`SchemaEntry`] rows by the subcommand that
//! reads them (`lsp`, `share`, `agileplus`, `tracera`). Each entry has:
//!
//! - `name` — human label shown in the printed summary.
//! - `env_var` — the literal env-var name (uppercase, ASCII, no alias).
//! - `default` — what value the subcommand falls back to when the env var
//!   is unset / empty.
//! - `description` — one-line explanation of what the env var does.
//!
//! Entries that have no meaningful default are rendered as `(unset)` so the
//! table stays a fixed width.

/// Logical owner of an env var — the binary subcommand that consults it.
///
/// Used purely for grouping in [`SchemaSurface::entries`]; it does not
/// influence behavior at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Subcommand {
    /// `helioslite lsp …` — language-server capabilities.
    Lsp,
    /// `helioslite share …` — sharecli realtime relay.
    Share,
    /// `helioslite agileplus …` — 31-pillar scorecard engine.
    Agileplus,
    /// Tracera telemetry (`TRACERA_ENDPOINT` opt-in sink).
    Tracera,
}

impl Subcommand {
    /// Stable string form used in CLI output. Lowercase, matches the
    /// subcommand name so a shell can do a 1:1 substring match.
    pub const fn as_str(self) -> &'static str {
        match self {
            Subcommand::Lsp => "lsp",
            Subcommand::Share => "share",
            Subcommand::Agileplus => "agileplus",
            Subcommand::Tracera => "tracera",
        }
    }
}

impl std::fmt::Display for Subcommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single row in the env-var schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaEntry {
    /// Owning subcommand (used for grouping).
    pub subcommand: Subcommand,
    /// Human-readable label shown in the printed summary.
    pub name: &'static str,
    /// Environment variable name (uppercase ASCII, no alias forms).
    pub env_var: &'static str,
    /// What the subcommand falls back to when this env var is unset. Use
    /// `"(unset)"` when there is no meaningful default.
    pub default: &'static str,
    /// One-line description of the env var's purpose.
    pub description: &'static str,
}

/// Top-level schema surface — every env var the binary's subcommands
/// honor, grouped by owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSurface {
    /// All [`SchemaEntry`] rows in the order they should be printed.
    pub entries: Vec<SchemaEntry>,
}

impl SchemaSurface {
    /// Build the canonical schema surface for the binary's subcommands.
    ///
    /// The order is stable: subcommands in alphabetical order (`agileplus`,
    /// `lsp`, `share`, `tracera`), with each subcommand's entries in the
    /// order they were documented in `crates/forge_config/src/schema.rs`.
    pub fn new() -> Self {
        Self {
            entries: vec![
                // --- agileplus -----------------------------------------------------
                SchemaEntry {
                    subcommand: Subcommand::Agileplus,
                    name: "default config",
                    env_var: "AGILEPLUS_DEFAULT_CONFIG",
                    default: "(unset)",
                    description: "Path to a baseline scorecard JSON loaded before any --scorecard arg.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Agileplus,
                    name: "default sprint length",
                    env_var: "AGILEPLUS_DEFAULT_SPRINT_LENGTH_DAYS",
                    default: "14",
                    description: "Default sprint length (days) when a scorecard omits the sprint field.",
                },
                // --- lsp -----------------------------------------------------------
                SchemaEntry {
                    subcommand: Subcommand::Lsp,
                    name: "rust-analyzer binary",
                    env_var: "RUST_ANALYZER_BIN",
                    default: "rust-analyzer",
                    description: "Absolute path or $PATH-resolvable name of the rust-analyzer executable.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Lsp,
                    name: "typescript language-server binary",
                    env_var: "TYPESCRIPT_LANGUAGE_SERVER_BIN",
                    default: "typescript-language-server",
                    description: "Absolute path or $PATH-resolvable name of the typescript-language-server executable.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Lsp,
                    name: "default request timeout",
                    env_var: "LSP_DEFAULT_TIMEOUT_MS",
                    default: "30000",
                    description: "Per-request timeout in milliseconds applied to every LSP capability call.",
                },
                // --- share ---------------------------------------------------------
                SchemaEntry {
                    subcommand: Subcommand::Share,
                    name: "default bind address",
                    env_var: "SHARE_DEFAULT_BIND",
                    default: "127.0.0.1:0",
                    description: "Default host:port for `share serve` when --bind is not supplied (0 = ephemeral).",
                },
                SchemaEntry {
                    subcommand: Subcommand::Share,
                    name: "default topic",
                    env_var: "SHARE_DEFAULT_TOPIC",
                    default: "default",
                    description: "Default pub/sub topic used by publish/subscribe when --topic is omitted.",
                },
                // --- tracera -------------------------------------------------------
                SchemaEntry {
                    subcommand: Subcommand::Tracera,
                    name: "collector endpoint",
                    env_var: "TRACERA_ENDPOINT",
                    default: "(unset)",
                    description: "HTTP endpoint of the Tracera collector. Telemetry is disabled when unset/empty.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Tracera,
                    name: "bearer token",
                    env_var: "TRACERA_TOKEN",
                    default: "(unset)",
                    description: "Optional bearer token sent as `Authorization: Bearer …` on every event.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Tracera,
                    name: "HMAC secret",
                    env_var: "TRACERA_HMAC_SECRET",
                    default: "(unset)",
                    description: "Optional HMAC secret used to sign event bodies for tamper detection.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Tracera,
                    name: "compression algorithm",
                    env_var: "TRACERA_COMPRESSION",
                    default: "none",
                    description: "Compression applied to event bodies: `none`, `gzip`, or `zstd`.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Tracera,
                    name: "auth rotation window",
                    env_var: "TRACERA_AUTH_ROTATION_WINDOW_MS",
                    default: "3600000",
                    description: "Milliseconds between forced credential rotations when a token or HMAC secret is configured.",
                },
                SchemaEntry {
                    subcommand: Subcommand::Tracera,
                    name: "disk store path",
                    env_var: "TRACERA_DISK_STORE_PATH",
                    default: "(unset)",
                    description: "Optional directory where events are queued on disk when the collector is unreachable.",
                },
            ],
        }
    }

    /// Iterate entries owned by `subcommand`, preserving the canonical
    /// order.
    pub fn entries_for(&self, subcommand: Subcommand) -> impl Iterator<Item = &SchemaEntry> {
        self.entries
            .iter()
            .filter(move |e| e.subcommand == subcommand)
    }

    /// Render the surface as a tabbed text table.
    ///
    /// Columns: `subcommand`, `name`, `env_var`, `default`, `description`.
    /// Column widths are computed from the data so the output stays aligned
    /// even if a description is unusually long.
    pub fn render_table(&self) -> String {
        let mut out = String::new();

        // Column widths derived from data.
        let headers = ["subcommand", "name", "env_var", "default", "description"];
        let mut widths = [0usize; 5];
        for (w, h) in widths.iter_mut().zip(headers.iter()) {
            *w = h.len();
        }
        for entry in &self.entries {
            widths[0] = widths[0].max(entry.subcommand.as_str().len());
            widths[1] = widths[1].max(entry.name.len());
            widths[2] = widths[2].max(entry.env_var.len());
            widths[3] = widths[3].max(entry.default.len());
            widths[4] = widths[4].max(entry.description.len());
        }

        // Header row.
        out.push_str(&format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}\n",
            headers[0],
            headers[1],
            headers[2],
            headers[3],
            headers[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
            w4 = widths[4],
        ));
        // Separator.
        let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
        out.push_str(&sep.join("  "));
        out.push('\n');

        // Body rows.
        for entry in &self.entries {
            out.push_str(&format!(
                "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {:<w4$}\n",
                entry.subcommand.as_str(),
                entry.name,
                entry.env_var,
                entry.default,
                entry.description,
                w0 = widths[0],
                w1 = widths[1],
                w2 = widths[2],
                w3 = widths[3],
                w4 = widths[4],
            ));
        }

        out
    }
}

impl Default for SchemaSurface {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience constructor — returns a freshly built [`SchemaSurface`].
///
/// Equivalent to `SchemaSurface::new()`; exists so callers can write
/// `forge_config::schema()` without naming the type.
pub fn schema() -> SchemaSurface {
    SchemaSurface::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_all_four_subcommands() {
        let s = SchemaSurface::new();
        let mut seen = std::collections::BTreeSet::new();
        for entry in &s.entries {
            seen.insert(entry.subcommand);
        }
        assert!(seen.contains(&Subcommand::Lsp));
        assert!(seen.contains(&Subcommand::Share));
        assert!(seen.contains(&Subcommand::Agileplus));
        assert!(seen.contains(&Subcommand::Tracera));
    }

    #[test]
    fn schema_includes_documented_env_vars() {
        let s = SchemaSurface::new();
        let env_vars: std::collections::BTreeSet<&str> =
            s.entries.iter().map(|e| e.env_var).collect();

        // LSP trio
        assert!(env_vars.contains("RUST_ANALYZER_BIN"));
        assert!(env_vars.contains("TYPESCRIPT_LANGUAGE_SERVER_BIN"));
        assert!(env_vars.contains("LSP_DEFAULT_TIMEOUT_MS"));
        // Share duo
        assert!(env_vars.contains("SHARE_DEFAULT_BIND"));
        assert!(env_vars.contains("SHARE_DEFAULT_TOPIC"));
        // Agileplus duo
        assert!(env_vars.contains("AGILEPLUS_DEFAULT_CONFIG"));
        assert!(env_vars.contains("AGILEPLUS_DEFAULT_SPRINT_LENGTH_DAYS"));
        // Tracera hex
        assert!(env_vars.contains("TRACERA_ENDPOINT"));
        assert!(env_vars.contains("TRACERA_TOKEN"));
        assert!(env_vars.contains("TRACERA_HMAC_SECRET"));
        assert!(env_vars.contains("TRACERA_COMPRESSION"));
        assert!(env_vars.contains("TRACERA_AUTH_ROTATION_WINDOW_MS"));
        assert!(env_vars.contains("TRACERA_DISK_STORE_PATH"));
    }

    #[test]
    fn env_var_names_are_uppercase_ascii() {
        for entry in SchemaSurface::new().entries {
            assert!(
                entry
                    .env_var
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'),
                "env_var {:?} must be uppercase ASCII with underscores",
                entry.env_var
            );
            assert!(!entry.env_var.is_empty());
        }
    }

    #[test]
    fn entries_for_filters_by_subcommand() {
        let s = SchemaSurface::new();
        let lsp: Vec<&str> = s.entries_for(Subcommand::Lsp).map(|e| e.env_var).collect();
        assert_eq!(lsp.len(), 3);
        assert!(lsp.contains(&"RUST_ANALYZER_BIN"));
        assert!(lsp.contains(&"TYPESCRIPT_LANGUAGE_SERVER_BIN"));
        assert!(lsp.contains(&"LSP_DEFAULT_TIMEOUT_MS"));
    }

    #[test]
    fn render_table_emits_a_header_row_and_body_rows() {
        let out = SchemaSurface::new().render_table();
        // Header columns present
        assert!(out.contains("subcommand"));
        assert!(out.contains("env_var"));
        assert!(out.contains("default"));
        assert!(out.contains("description"));
        // At least one body row per documented env var.
        assert!(out.contains("RUST_ANALYZER_BIN"));
        assert!(out.contains("TRACERA_ENDPOINT"));
        assert!(out.contains("AGILEPLUS_DEFAULT_SPRINT_LENGTH_DAYS"));
        // Default values shown as their literal strings.
        assert!(out.contains("30000"));
        assert!(out.contains("(unset)"));
    }

    #[test]
    fn render_table_starts_with_subcommand_header() {
        let out = SchemaSurface::new().render_table();
        let first_line = out.lines().next().expect("non-empty");
        assert!(
            first_line.starts_with("subcommand"),
            "expected header row first, got {first_line:?}"
        );
    }

    #[test]
    fn subcommand_display_matches_as_str() {
        assert_eq!(Subcommand::Lsp.to_string(), "lsp");
        assert_eq!(Subcommand::Share.to_string(), "share");
        assert_eq!(Subcommand::Agileplus.to_string(), "agileplus");
        assert_eq!(Subcommand::Tracera.to_string(), "tracera");
    }

    #[test]
    fn schema_helper_returns_same_as_new() {
        let from_helper = schema();
        let from_new = SchemaSurface::new();
        assert_eq!(from_helper.entries.len(), from_new.entries.len());
        for (a, b) in from_helper.entries.iter().zip(from_new.entries.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn default_impl_matches_new() {
        let from_default = SchemaSurface::default();
        let from_new = SchemaSurface::new();
        assert_eq!(from_default.entries.len(), from_new.entries.len());
    }
}
