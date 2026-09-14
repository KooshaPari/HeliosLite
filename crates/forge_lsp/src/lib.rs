//! `forge_lsp` — language-server-style services for the agent's
//! REPL. The crate exposes:
//!
//!   * P2.3 — `DiagnosticsProvider` trait + `RustcProvider` and
//!     `TscProvider` (cargo check / tsc --noEmit) wired through a
//!     bounded LRU cache (`DiagnosticsService`).
//!   * P2.3.1 — `HoverProvider`, `DefinitionProvider`,
//!     `CompletionProvider` (LSP `textDocument/hover`,
//!     `textDocument/definition`, `textDocument/completion`) over a
//!     process-backed JSON-RPC client (`lsp_client::ProcessLspClient`),
//!     plus a `Server` facade that bundles everything behind a single
//!     per-workspace handle.
//!   * P2.3.2 — `ImplementationProvider`, `ReferencesProvider` +
//!     `TypeDefinitionProvider`, and `RenameProvider` (LSP
//!     `textDocument/implementation`, `textDocument/references`,
//!     `textDocument/typeDefinition`, `textDocument/rename`) over the
//!     same JSON-RPC client. `Server` exposes them as
//!     `Server::implementation`, `Server::references`,
//!     `Server::type_definition`, and `Server::rename`.
//!
//! All providers degrade gracefully when no language server is
//! installed: they still parse whatever the subprocess returns, and
//! `Server::hover` / `Server::implementation` / etc. return
//! `Ok(None)` / `Ok(vec![])` for unsupported files so the REPL
//! doesn't have to special-case errors.

#![allow(missing_docs, dead_code, unused)]

pub mod commands;
pub mod completion;
pub mod definition;
pub mod diagnostic;
pub mod hover;
pub mod implementation;
pub mod lsp_client;
pub mod mcp_watcher;
pub mod provider;
pub mod references;
pub mod rename;
pub mod rustc;
pub mod server;
pub mod service;
pub mod tsc;

pub use completion::{CompletionItem, CompletionKind, CompletionProvider};
pub use definition::{DefinitionProvider, Location};
pub use diagnostic::{Diagnostic, DiagnosticSeverity};
pub use hover::{Hover, HoverProvider};
pub use implementation::{ImplementationError, ImplementationProvider, ImplementationResult};
pub use lsp_client::{LspClient, ProcessLspClient, ServerCapabilities, WeakProcessClient};
pub use mcp_watcher::{
    DEFAULT_DEBOUNCE, McpAutoReloadConfig, McpReloadFn, McpWatcher, McpWatcherError,
    McpWatcherHandle, default_mcp_config_path,
};
pub use provider::{DiagnosticsError, DiagnosticsProvider, DiagnosticsResult};
pub use references::{
    ReferencesError, ReferencesOptions, ReferencesProvider, ReferencesResult,
    TypeDefinitionProvider,
};
pub use rename::{
    DocumentChange, DocumentIdentifier, RenameError, RenameOutcome, RenameProvider, RenameResult,
    TextEdit as RenameTextEdit, WorkspaceEdit,
};
pub use rustc::RustcProvider;
pub use server::{Server, SharedServer};
pub use service::{DiagnosticsService, LruCache, SharedDiagnosticsService};
pub use tsc::TscProvider;
