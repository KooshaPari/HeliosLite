//! `lsp_client` — minimal JSON-RPC-over-stdio LSP client.
//!
//! `forge_lsp` already shells out to `cargo check` / `tsc` for
//! diagnostics. For hover, definition, and completion we need to talk
//! JSON-RPC to a real language server (`rust-analyzer`,
//! `typescript-language-server`).
//!
//! This module defines:
//!   * the JSON-RPC message envelope (request / response / notification),
//!   * a small trait [`LspClient`] so the providers can be unit-tested
//!     against a fake (see `tests/fixtures.rs`),
//!   * a real implementation [`ProcessLspClient`] that spawns a
//!     subprocess and reads/writes framed JSON over its stdio, used
//!     by [`crate::hover`], [`crate::definition`], and
//!     [`crate::completion`].
//!
//! The crate deliberately avoids pulling in `lsp-types` — the request
//! shapes we need (hover / definition / completion) are small and the
//! LSP spec is stable, so we keep them in this file.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// JSON-RPC envelope
// ---------------------------------------------------------------------------

/// A JSON-RPC request we send to the language server.
#[derive(Debug, Clone, Serialize)]
pub struct LspRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'static str,
    pub params: Value,
}

impl LspRequest {
    pub fn new(id: u64, method: &'static str, params: Value) -> Self {
        Self { jsonrpc: "2.0", id, method, params }
    }
}

/// A JSON-RPC response from the language server. We accept either a
/// `result` or an `error` — providers surface the error verbatim.
#[derive(Debug, Clone, Deserialize)]
pub struct LspResponse {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<LspError>,
}

/// JSON-RPC error object as defined by the LSP spec.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LspError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

// ---------------------------------------------------------------------------
// LspClient trait — providers depend on this so they can be tested
// against a fake.
// ---------------------------------------------------------------------------

/// A language-server client. The methods return the raw `serde_json::Value`
/// the server returned for the request. Providers (hover / definition /
/// completion) parse that value into the appropriate domain type.
pub trait LspClient: Send + Sync {
    /// Identifier of the underlying server (`"rust-analyzer"`,
    /// `"typescript-language-server"`).
    fn server_name(&self) -> &'static str;

    /// Send `request` and wait for the matching response. Implementations
    /// are responsible for routing by `id` and for handling
    /// server-initiated notifications (which they discard while waiting
    /// for the matching response).
    fn send(&self, request: LspRequest) -> Result<LspResponse, String>;
}

// ---------------------------------------------------------------------------
// Real implementation: spawn a subprocess, read/write framed JSON
// ---------------------------------------------------------------------------

/// Standard LSP `Content-Length` header parser. Returns the body.
fn read_framed<R: Read>(reader: &mut R) -> Result<String, String> {
    // Read headers one byte at a time (LSP requires ASCII headers).
    let mut header: Vec<u8> = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        match reader.read_exact(&mut byte) {
            Ok(()) => {
                header.push(byte[0]);
                if header.ends_with(b"\r\n\r\n") {
                    break;
                }
                if header.len() > 8 * 1024 {
                    return Err("header too large".to_string());
                }
            }
            Err(e) => return Err(format!("header read failed: {e}")),
        }
    }
    let header_str = std::str::from_utf8(&header).map_err(|e| format!("header not utf-8: {e}"))?;
    let content_length = header_str
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.eq_ignore_ascii_case("content-length") {
                v.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .ok_or_else(|| "missing Content-Length header".to_string())?;
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|e| format!("body read failed: {e}"))?;
    String::from_utf8(body).map_err(|e| format!("body not utf-8: {e}"))
}

fn write_framed<W: Write>(writer: &mut W, body: &str) -> Result<(), String> {
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    writer
        .write_all(header.as_bytes())
        .map_err(|e| format!("header write failed: {e}"))?;
    writer
        .write_all(body.as_bytes())
        .map_err(|e| format!("body write failed: {e}"))?;
    writer.flush().map_err(|e| format!("flush failed: {e}"))
}

/// A capability bundle returned by the LSP `initialize` handshake.
///
/// We only carry the bits `forge_lsp` needs: the server identifier and
/// the server's own advertised capabilities (left as opaque JSON for
/// callers that care).
#[derive(Debug, Clone)]
pub struct ServerCapabilities {
    pub server_name: String,
    pub capabilities: Value,
}

struct ProcessIo {
    stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
}

struct SharedClientState {
    name: &'static str,
    child: Option<Child>,
    io: Option<ProcessIo>,
    initialized: AtomicBool,
}

/// Spawn a real LSP server subprocess and talk JSON-RPC over its stdio.
///
/// `ProcessLspClient` is intentionally cheap-cloneable. Cloning the
/// `ProcessLspClient` shares the same underlying subprocess via
/// `Arc<Mutex<...>>` so multiple providers (hover, definition,
/// completion) can talk to one server.
///
/// Subprocess termination is governed by the **last clone's** `Drop`.
/// Earlier drops don't kill the process — only when the `Arc` strong
/// count returns to zero does the final `Drop` reap the child.
#[derive(Clone)]
pub struct ProcessLspClient {
    state: Arc<Mutex<SharedClientState>>,
}

impl ProcessLspClient {
    /// Spawn the binary at `bin` with `args`. The child's stdio is piped
    /// for JSON-RPC framing.
    pub fn spawn(bin: &Path, args: &[&str], name: &'static str) -> Result<Self, String> {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn {bin:?}: {e}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "child stdin unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "child stdout unavailable".to_string())?;
        Ok(Self {
            state: Arc::new(Mutex::new(SharedClientState {
                name,
                child: Some(child),
                io: Some(ProcessIo { stdin, stdout }),
                initialized: AtomicBool::new(false),
            })),
        })
    }

    /// Spawn `rust-analyzer`.
    pub fn rust_analyzer() -> Result<Self, String> {
        Self::spawn(Path::new("rust-analyzer"), &[], "rust-analyzer")
    }

    /// Spawn `typescript-language-server` (the LSP-compliant TypeScript
    /// language server). The original `tsserver` binary is the
    /// legacy/non-LSP command-server; we talk JSON-RPC here, so we
    /// require an LSP-speaking server.
    pub fn typescript_language_server() -> Result<Self, String> {
        Self::spawn(
            Path::new("typescript-language-server"),
            &["--stdio"],
            "typescript-language-server",
        )
    }

    /// Perform the LSP `initialize` handshake. The first time this is
    /// called on a `ProcessLspClient`, it sends `initialize` + waits
    /// for the response, then sends `initialized`. Subsequent calls are
    /// a no-op.
    ///
    /// `workspace_root` is the LSP `rootUri`; we send it as
    /// `file://...` per spec.
    pub fn initialize(&self, workspace_root: &Path) -> Result<ServerCapabilities, String> {
        // Fast path: another thread already initialized.
        if self
            .state
            .lock()
            .map_err(|e| format!("state mutex poisoned: {e}"))?
            .initialized
            .load(Ordering::Acquire)
        {
            return Ok(ServerCapabilities {
                server_name: self
                    .state
                    .lock()
                    .map_err(|e| format!("state mutex poisoned: {e}"))?
                    .name
                    .to_string(),
                capabilities: Value::Null,
            });
        }

        let root_uri = crate::server::path_to_uri_for(workspace_root);
        let init_params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "forge_lsp", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": root_uri,
            "capabilities": {
                "workspace": { "workspaceFolders": true },
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "completion": { "completionItem": { "snippetSupport": false } },
                    "definition": { "linkSupport": true },
                    "synchronization": { "dynamicRegistration": false }
                }
            }
        });

        let resp = self.send(LspRequest::new(0, "initialize", init_params))?;

        // Send the `initialized` notification (no `id`, no response).
        let initialized_notification = json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        });
        {
            let mut state = self
                .state
                .lock()
                .map_err(|e| format!("state mutex poisoned: {e}"))?;
            let io = state
                .io
                .as_mut()
                .ok_or_else(|| "io unavailable".to_string())?;
            let body = serde_json::to_string(&initialized_notification)
                .map_err(|e| format!("serialize initialized: {e}"))?;
            write_framed(&mut io.stdin, &body)?;
        }

        self.state
            .lock()
            .map_err(|e| format!("state mutex poisoned: {e}"))?
            .initialized
            .store(true, Ordering::Release);

        let capabilities = resp
            .result
            .as_ref()
            .and_then(|r| r.get("capabilities").cloned())
            .unwrap_or(Value::Null);
        Ok(ServerCapabilities {
            server_name: self
                .state
                .lock()
                .map_err(|e| format!("state mutex poisoned: {e}"))?
                .name
                .to_string(),
            capabilities,
        })
    }

    /// Notify the server that `uri` is open with `text`.
    ///
    /// Language servers only serve requests for documents they know about:
    /// `textDocument/didOpen` is how a client registers a buffer. Without it,
    /// `rust-analyzer` answers `textDocument/definition` with
    /// `-32603 file not found` for a file that exists on disk but is not part
    /// of a loaded project (observed in the `e2e_rust_analyzer` test before
    /// this call existed).
    ///
    /// `language_id` follows the LSP convention, e.g. `rust` or `typescript`.
    pub fn did_open(&self, uri: &str, language_id: &str, text: &str) -> Result<(), String> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": 1,
                    "text": text
                }
            }
        });
        let mut state = self
            .state
            .lock()
            .map_err(|e| format!("state mutex poisoned: {e}"))?;
        let io = state
            .io
            .as_mut()
            .ok_or_else(|| "io unavailable".to_string())?;
        let body =
            serde_json::to_string(&notification).map_err(|e| format!("serialize didOpen: {e}"))?;
        write_framed(&mut io.stdin, &body)
    }

    /// Cheap-clone the [`ProcessLspClient`] as a [`WeakProcessClient`]
    /// handle that participates in lifecycle but doesn't keep the
    /// subprocess alive on its own.
    pub fn downgrade(&self) -> WeakProcessClient {
        WeakProcessClient { state: Arc::downgrade(&self.state) }
    }

    /// Kill the underlying subprocess immediately, regardless of how
    /// many clones remain. Idempotent.
    pub fn kill(&self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| format!("state mutex poisoned: {e}"))?;
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        state.io = None;
        Ok(())
    }
}

/// A non-owning handle to a [`ProcessLspClient`]. Cloning does not keep
/// the subprocess alive.
#[derive(Clone)]
pub struct WeakProcessClient {
    state: Weak<Mutex<SharedClientState>>,
}

impl WeakProcessClient {
    /// Try to upgrade to a strong reference. Returns `None` if the
    /// underlying client has already been dropped.
    pub fn upgrade(&self) -> Option<ProcessLspClient> {
        self.state.upgrade().map(|state| ProcessLspClient { state })
    }
}

impl LspClient for ProcessLspClient {
    fn server_name(&self) -> &'static str {
        self.state.lock().map(|s| s.name).unwrap_or("<poisoned>")
    }

    fn send(&self, request: LspRequest) -> Result<LspResponse, String> {
        let body =
            serde_json::to_string(&request).map_err(|e| format!("serialize request: {e}"))?;
        let request_id = request.id;

        // Write under the same lock we read under to avoid interleaved
        // framing. Because the lock is process-wide, concurrent senders
        // will serialize — fine for our low-volume use.
        let mut state = self
            .state
            .lock()
            .map_err(|e| format!("state mutex poisoned: {e}"))?;
        let io = state
            .io
            .as_mut()
            .ok_or_else(|| "subprocess not running".to_string())?;
        write_framed(&mut io.stdin, &body)?;

        // Read responses until we see one whose `id` matches our request.
        // Server-initiated notifications (no `id`) and out-of-order
        // responses are discarded.
        loop {
            let response_body = read_framed(&mut io.stdout)?;
            let resp: LspResponse = serde_json::from_str(&response_body)
                .map_err(|e| format!("parse response: {e} (body={response_body:?})"))?;
            match resp.id {
                Some(id) if id == request_id => return Ok(resp),
                // Same logical request seen before — duplicates or
                // stray responses. Drop and continue.
                Some(_) => continue,
                // No `id` => notification; discard and keep reading.
                None => continue,
            }
        }
    }
}

impl Drop for SharedClientState {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// Shared LSP request parameter shapes (hover / definition / completion)
// ---------------------------------------------------------------------------

/// LSP `TextDocumentPositionParams` — used by hover, definition, completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentPositionParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// LSP `Position` — 0-based line and UTF-16 column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// LSP `Range`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// LSP `TextEdit` (used by completion `textEdit`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// LSP `CompletionContext` (optional, omitted in our requests).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompletionContext {
    #[serde(rename = "triggerKind", default)]
    pub trigger_kind: i32,
    #[serde(
        rename = "triggerCharacter",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub trigger_character: Option<String>,
}

/// LSP `CompletionParams`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionParams {
    #[serde(rename = "textDocument")]
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<CompletionContext>,
}

// ---------------------------------------------------------------------------
// Tests for the framing helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_framed_extracts_body_after_content_length() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"result":42}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{}", payload.len(), payload);
        let mut cursor = Cursor::new(framed.into_bytes());
        let body = read_framed(&mut cursor).expect("reads framed body");
        assert_eq!(body, payload);
    }

    #[test]
    fn read_framed_rejects_missing_header() {
        let mut cursor = Cursor::new(b"junk".to_vec());
        let r = read_framed(&mut cursor);
        assert!(r.is_err());
    }

    #[test]
    fn write_framed_emits_correct_header() {
        let mut buf = Vec::new();
        let body = r#"{"x":1}"#;
        write_framed(&mut buf, body).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with(&format!("Content-Length: {}\r\n\r\n", body.len())));
        assert!(s.ends_with(body));
    }

    #[test]
    fn lsp_request_serializes_with_envelope() {
        let req = LspRequest::new(7, "textDocument/hover", json!({"foo": 1}));
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(s.contains("\"id\":7"));
        assert!(s.contains("\"method\":\"textDocument/hover\""));
    }

    #[test]
    fn lsp_response_deserializes_result_variant() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"value":"hi"}}"#;
        let resp: LspResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn lsp_response_deserializes_error_variant() {
        let raw =
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: LspResponse = serde_json::from_str(raw).unwrap();
        assert!(resp.result.is_none());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }
}
