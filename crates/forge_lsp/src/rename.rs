//! `RenameProvider` — forwards `textDocument/rename` to a real LSP
//! server (`rust-analyzer`, `typescript-language-server`).
//!
//! Per LSP spec the server returns either `null` (no rename possible)
//! or a `WorkspaceEdit` describing the textual edits to apply across
//! the workspace. We model the shape the REPL actually needs:
//!
//!   * `RenameResult::WorkspaceEdit` carries the full edit list so the
//!     agent can preview / persist them.
//!   * `RenameResult::NoResult` is the "nothing to rename" case.
//!
//! Edits are normalised to `Vec<TextEdit>` grouped per URI so the
//! caller can apply them without re-implementing LSP's `documentChanges`
//! envelope.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::lsp_client::{LspClient, LspRequest, Position, Range, TextDocumentIdentifier};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A single text edit — `range` + replacement string. Reuses the
/// [`crate::lsp_client::TextEdit`] shape (already in use by completion).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// A document change: the URI to edit and the list of textual edits to
/// apply there (in `Range`-sorted order per the LSP spec).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentChange {
    #[serde(rename = "textDocument")]
    pub text_document: DocumentIdentifier,
    pub edits: Vec<TextEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentIdentifier {
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "version")]
    pub version: Option<i32>,
}

/// A workspace edit — a list of document changes. We only model the
/// `documentChanges: DocumentChange[]` form (the modern, preferred
/// LSP shape); the legacy `changes: { [uri]: TextEdit[] }` form is
/// transparently normalised by [`parse_workspace_edit`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct WorkspaceEdit {
    #[serde(default, rename = "documentChanges")]
    pub document_changes: Vec<DocumentChange>,
}

/// Result of a rename request. `WorkspaceEdit` when the server has a
/// concrete edit set; `NoResult` when the symbol can't be renamed
/// (e.g. a built-in).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RenameResult {
    WorkspaceEdit(WorkspaceEdit),
    NoResult(serde_json::Value),
}

/// Errors that `RenameProvider::rename` can surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// The supplied `new_name` is empty / whitespace-only, which would emit
    /// a delete-only (destructive) `TextEdit`. Rejected before forwarding.
    InvalidName(String),
    /// The LSP server returned an error response.
    ServerError(String),
    /// The LSP server returned a non-WorkspaceEdit payload.
    InvalidResponse(String),
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::InvalidName(n) => write!(f, "invalid rename target: {n:?}"),
            RenameError::ServerError(m) => write!(f, "lsp server error: {m}"),
            RenameError::InvalidResponse(m) => write!(f, "invalid rename response: {m}"),
        }
    }
}

impl std::error::Error for RenameError {}

/// Outcome of a rename request: either a `WorkspaceEdit` to apply, or
/// `None` when the server says "nothing to rename".
pub type RenameOutcome = Result<Option<WorkspaceEdit>, RenameError>;

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Forwards `textDocument/rename` to an LSP client.
///
/// Generic over `C: LspClient` (rather than `Box<dyn LspClient>`) so
/// we can share a single concrete client via `Arc` without paying for
/// dynamic dispatch. Matches the convention used by
/// [`crate::hover::HoverProvider`].
pub struct RenameProvider<C: LspClient + ?Sized> {
    client: std::sync::Arc<C>,
}

impl<C: LspClient + ?Sized> RenameProvider<C> {
    pub fn new(client: std::sync::Arc<C>) -> Self {
        Self { client }
    }

    pub fn name(&self) -> &'static str {
        self.client.server_name()
    }

    pub fn supports(&self, path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .as_deref(),
            Some("rs")
                | Some("ts")
                | Some("tsx")
                | Some("mts")
                | Some("cts")
                | Some("js")
                | Some("jsx")
                | Some("mjs")
                | Some("cjs")
        )
    }

    /// Run `textDocument/rename` for `uri` at `position` to `new_name`.
    pub fn rename(&self, uri: &str, position: Position, new_name: &str) -> RenameOutcome {
        if new_name.trim().is_empty() {
            return Err(RenameError::InvalidName(new_name.to_string()));
        }
        let params = RenameParams {
            text_document: TextDocumentIdentifier { uri: uri.to_string() },
            position,
            new_name: new_name.to_string(),
        };
        let request = LspRequest::new(
            next_request_id(),
            "textDocument/rename",
            serde_json::to_value(&params)
                .map_err(|e| RenameError::InvalidResponse(format!("serialize params: {e}")))?,
        );
        let response = self
            .client
            .send(request)
            .map_err(RenameError::ServerError)?;
        if let Some(err) = response.error {
            return Err(RenameError::ServerError(format!(
                "{} (code {})",
                err.message, err.code
            )));
        }
        match response.result {
            None => Ok(None),
            Some(ref raw) if raw.is_null() => Ok(None),
            Some(ref raw) => parse_workspace_edit(raw),
        }
    }
}

/// LSP `RenameParams` — adds `newName` on top of
/// `TextDocumentPositionParams`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RenameParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
    position: Position,
    #[serde(rename = "newName")]
    new_name: String,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Normalise a `WorkspaceEdit` payload. The LSP spec allows:
///   * the modern form: `{ documentChanges: DocumentChange[] }`
///   * the legacy form: `{ changes: { [uri]: TextEdit[] } }`
///   * `null` (no rename possible)
fn parse_workspace_edit(raw: &Value) -> RenameOutcome {
    if raw.is_null() {
        return Ok(None);
    }
    // Modern form: an explicit `documentChanges` key. We deliberately
    // require the key to be present (not just `WorkspaceEdit`'s
    // default-derived empty `Vec`) so the legacy form below wins when
    // only `changes` is set.
    if let Some(doc_changes) = raw.get("documentChanges") {
        let edit: WorkspaceEdit = serde_json::from_value(raw.clone())
            .map_err(|e| RenameError::InvalidResponse(format!("documentChanges: {e}")))?;
        // `serde_json` will already have populated the field. The
        // explicit return is just for clarity.
        let _ = doc_changes;
        return Ok(Some(edit));
    }
    // Legacy form: `changes: { [uri]: TextEdit[] }`.
    if let Some(Value::Object(changes)) = raw.get("changes") {
        let mut doc_changes = Vec::new();
        for (uri, edits_value) in changes {
            // Each value can be TextEdit[] or null.
            let edits: Vec<TextEdit> = match edits_value {
                Value::Array(_) => serde_json::from_value(edits_value.clone())
                    .map_err(|e| RenameError::InvalidResponse(format!("legacy edits: {e}")))?,
                Value::Null => continue,
                _ => {
                    return Err(RenameError::InvalidResponse(
                        "legacy edits value is not array".to_string(),
                    ));
                }
            };
            doc_changes.push(DocumentChange {
                text_document: DocumentIdentifier { uri: uri.clone(), version: None },
                edits,
            });
        }
        return Ok(Some(WorkspaceEdit { document_changes: doc_changes }));
    }
    Err(RenameError::InvalidResponse(format!(
        "could not parse WorkspaceEdit: {raw}"
    )))
}

// ---------------------------------------------------------------------------
// Request id allocation (same scheme as the other providers)
// ---------------------------------------------------------------------------

fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_client::LspResponse;
    use std::sync::{Arc, Mutex};

    pub struct MockLspClient {
        pub name: &'static str,
        pub scripted: Mutex<Option<Result<LspResponse, String>>>,
        pub captured: Mutex<Vec<LspRequest>>,
    }

    impl MockLspClient {
        pub fn new(name: &'static str, response: LspResponse) -> Self {
            Self {
                name,
                scripted: Mutex::new(Some(Ok(response))),
                captured: Mutex::new(Vec::new()),
            }
        }

        pub fn failing(name: &'static str, err: &str) -> Self {
            Self {
                name,
                scripted: Mutex::new(Some(Err(err.to_string()))),
                captured: Mutex::new(Vec::new()),
            }
        }

        #[allow(dead_code)]
        pub fn last_request(&self) -> Option<LspRequest> {
            self.captured.lock().unwrap().last().cloned()
        }
    }

    impl crate::lsp_client::LspClient for MockLspClient {
        fn server_name(&self) -> &'static str {
            self.name
        }

        fn send(&self, request: LspRequest) -> Result<LspResponse, String> {
            self.captured.lock().unwrap().push(request);
            let next = self.scripted.lock().unwrap().take();
            match next {
                Some(r) => r,
                None => Ok(LspResponse {
                    jsonrpc: Some("2.0".into()),
                    id: Some(0),
                    result: Some(Value::Null),
                    error: None,
                }),
            }
        }
    }

    fn ok_response(result: Value) -> LspResponse {
        LspResponse {
            jsonrpc: Some("2.0".into()),
            id: Some(1),
            result: Some(result),
            error: None,
        }
    }

    fn err_response(code: i64, message: &str) -> LspResponse {
        LspResponse {
            jsonrpc: Some("2.0".into()),
            id: Some(1),
            result: None,
            error: Some(crate::lsp_client::LspError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }

    /// LspClient wrapper that funnels every captured request through a
    /// shared `Arc<Mutex<Vec<_>>>` so the test can read requests even
    /// after the owning `Box<dyn LspClient>` is dropped.
    struct CaptureClient {
        name: &'static str,
        response: LspResponse,
        captured: Arc<Mutex<Vec<LspRequest>>>,
    }
    impl crate::lsp_client::LspClient for CaptureClient {
        fn server_name(&self) -> &'static str {
            self.name
        }
        fn send(&self, request: LspRequest) -> Result<LspResponse, String> {
            self.captured.lock().unwrap().push(request);
            Ok(self.response.clone())
        }
    }

    #[test]
    fn rename_supports_common_languages() {
        let client = Arc::new(MockLspClient::new(
            "rust-analyzer",
            ok_response(Value::Null),
        ));
        let p = RenameProvider::new(client);
        assert!(p.supports(Path::new("foo.rs")));
        assert!(p.supports(Path::new("foo.ts")));
        assert!(p.supports(Path::new("foo.tsx")));
        assert!(p.supports(Path::new("foo.JS")));
        assert!(p.supports(Path::new("foo.mts")));
        assert!(p.supports(Path::new("foo.cts")));
        assert!(!p.supports(Path::new("foo.py")));
        assert!(!p.supports(Path::new("foo")));
    }

    #[test]
    fn rename_returns_none_when_server_returns_null() {
        let client = Arc::new(MockLspClient::new(
            "rust-analyzer",
            ok_response(Value::Null),
        ));
        let p = RenameProvider::new(client);
        let r = p
            .rename(
                "file:///foo.rs",
                Position { line: 0, character: 0 },
                "new_name",
            )
            .unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn rename_rejects_empty_and_whitespace_new_name_before_forwarding() {
        let client = Arc::new(MockLspClient::new(
            "rust-analyzer",
            ok_response(json!({
                "changes": {},
            })),
        ));
        let p = RenameProvider::new(client);
        for bad in ["", "   ", "\t", "\n"] {
            let err = p
                .rename("file:///foo.rs", Position { line: 0, character: 0 }, bad)
                .unwrap_err();
            assert_eq!(err, RenameError::InvalidName(bad.to_string()));
        }
        // Nothing should have been forwarded to the server.
        assert!(p.client.captured.lock().unwrap().is_empty());
    }

    #[test]
    fn rename_parses_modern_document_changes_payload() {
        let client = Arc::new(MockLspClient::new(
            "rust-analyzer",
            ok_response(json!({
                "documentChanges": [
                    {
                        "textDocument": {"uri": "file:///a.rs", "version": 1},
                        "edits": [
                            {"range": {"start":{"line":1,"character":0},"end":{"line":1,"character":3}}, "newText": "new_name"}
                        ]
                    },
                    {
                        "textDocument": {"uri": "file:///b.rs"},
                        "edits": [
                            {"range": {"start":{"line":2,"character":0},"end":{"line":2,"character":3}}, "newText": "new_name"},
                            {"range": {"start":{"line":3,"character":0},"end":{"line":3,"character":3}}, "newText": "new_name"}
                        ]
                    }
                ]
            })),
        ));
        let p = RenameProvider::new(client);
        let edit = p
            .rename(
                "file:///foo.rs",
                Position { line: 5, character: 2 },
                "new_name",
            )
            .unwrap()
            .expect("expected WorkspaceEdit");
        assert_eq!(edit.document_changes.len(), 2);
        let first = &edit.document_changes[0];
        assert_eq!(first.text_document.uri, "file:///a.rs");
        assert_eq!(first.text_document.version, Some(1));
        assert_eq!(first.edits.len(), 1);
        assert_eq!(first.edits[0].new_text, "new_name");
        let second = &edit.document_changes[1];
        assert_eq!(second.edits.len(), 2);
        assert_eq!(second.text_document.uri, "file:///b.rs");
        assert_eq!(second.text_document.version, None);
    }

    #[test]
    fn rename_parses_legacy_changes_payload() {
        let client = Arc::new(MockLspClient::new(
            "tsserver",
            ok_response(json!({
                "changes": {
                    "file:///a.ts": [
                        {"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}},"newText":"x"}
                    ],
                    "file:///b.ts": []
                }
            })),
        ));
        let p = RenameProvider::new(client);
        let edit = p
            .rename("file:///foo.ts", Position { line: 0, character: 0 }, "x")
            .unwrap()
            .expect("expected WorkspaceEdit");
        assert_eq!(edit.document_changes.len(), 2);
        // Empty edit list still produces a DocumentChange entry.
        let empty = edit
            .document_changes
            .iter()
            .find(|d| d.text_document.uri == "file:///b.ts")
            .unwrap();
        assert!(empty.edits.is_empty());
    }

    #[test]
    fn rename_request_method_and_new_name() {
        let captured: Arc<Mutex<Vec<LspRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let client = CaptureClient {
            name: "rust-analyzer",
            response: ok_response(Value::Null),
            captured: captured.clone(),
        };
        let p = RenameProvider::new(Arc::new(client));
        let _ = p
            .rename(
                "file:///x.rs",
                Position { line: 1, character: 2 },
                "renamed",
            )
            .unwrap();
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured.first().map(|c| c.method),
            Some("textDocument/rename")
        );
        let params_str = serde_json::to_string(&captured[0].params).unwrap();
        assert!(params_str.contains("\"newName\":\"renamed\""));
    }

    #[test]
    fn rename_propagates_server_error() {
        let client = Arc::new(MockLspClient::new(
            "rust-analyzer",
            err_response(-32601, "method not found"),
        ));
        let p = RenameProvider::new(client);
        let r = p.rename("file:///foo.rs", Position { line: 0, character: 0 }, "new");
        assert!(matches!(r, Err(RenameError::ServerError(_))));
    }

    #[test]
    fn rename_surfaces_transport_failure() {
        let client = Arc::new(MockLspClient::failing("rust-analyzer", "io: closed"));
        let p = RenameProvider::new(client);
        let r = p.rename("file:///foo.rs", Position { line: 0, character: 0 }, "new");
        assert!(matches!(r, Err(RenameError::ServerError(m)) if m == "io: closed"));
    }

    #[test]
    fn rename_rejects_malformed_payload() {
        let client = Arc::new(MockLspClient::new("rust-analyzer", ok_response(json!(42))));
        let p = RenameProvider::new(client);
        let r = p.rename("file:///foo.rs", Position { line: 0, character: 0 }, "new");
        assert!(matches!(r, Err(RenameError::InvalidResponse(_))));
    }
}
