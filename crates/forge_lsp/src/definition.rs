//! `DefinitionProvider` — forwards `textDocument/definition` to a real
//! LSP server (`rust-analyzer`, `typescript-language-server`).
//!
//! Per LSP spec the server returns either a single [`Location`], an
//! array, or (with `linkSupport: true`) one or more `LocationLink`
//! objects. We normalize all three to `Vec<Location>` so callers
//! always work with a list (a definition is usually 1 item, but
//! type-usages can produce multiple).
//!
//! [`Location`]: crate::definition::Location

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

use crate::lsp_client::{
    LspClient, LspRequest, LspResponse, Position, Range, TextDocumentIdentifier,
    TextDocumentPositionParams,
};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// LSP 3.17 response error code for `ContentModified`: the server dropped
/// the result and the spec instructs the *client* to re-issue later.
const CONTENT_MODIFIED: i64 = -32801;

/// Cold-server retry budget for [`DefinitionProvider::definition`]:
/// 20 attempts spaced 400ms apart ≈ 7.6s worst case, only ever paid when
/// the server explicitly reports `ContentModified` (a cold rust-analyzer
/// under CI load can stay in VFS churn for several seconds after
/// `didOpen`; nightly runs 36132727849 / 36318434190 failed for exactly
/// this reason before the retry existed).
const MAX_ATTEMPTS: u32 = 20;
const RETRY_DELAY: Duration = Duration::from_millis(400);

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A location in a workspace — `uri` + `range`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// Errors that `DefinitionProvider::definition` can surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    /// The LSP server returned an error response.
    ServerError(String),
    /// The LSP server returned a non-Location payload.
    InvalidResponse(String),
}

impl std::fmt::Display for DefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DefinitionError::ServerError(m) => write!(f, "lsp server error: {m}"),
            DefinitionError::InvalidResponse(m) => write!(f, "invalid definition response: {m}"),
        }
    }
}

impl std::error::Error for DefinitionError {}

pub type DefinitionResult = Result<Vec<Location>, DefinitionError>;

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Generic over `C: LspClient` (no `Box<dyn>`).
pub struct DefinitionProvider<C: LspClient + ?Sized> {
    client: std::sync::Arc<C>,
}

impl<C: LspClient + ?Sized> DefinitionProvider<C> {
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

    /// Run `textDocument/definition` for `uri` at `position`.
    ///
    /// Transient `ContentModified` (-32801) responses are re-issued on the
    /// same warm session (see [`CONTENT_MODIFIED`]) before surfacing an
    /// error, per LSP 3.17 §response error codes.
    pub fn definition(&self, uri: &str, position: Position) -> DefinitionResult {
        self.definition_with_retry(uri, position, MAX_ATTEMPTS, RETRY_DELAY)
    }

    /// Policy-parameterised core of [`Self::definition`] so tests can drive
    /// the retry loop with a short delay and a small attempt cap.
    fn definition_with_retry(
        &self,
        uri: &str,
        position: Position,
        max_attempts: u32,
        delay: Duration,
    ) -> DefinitionResult {
        let mut attempt = 1u32;
        loop {
            // IO / serialize failures surface immediately — only a server
            // `ContentModified` response is treated as transient here.
            let response = self.send_definition(uri, position)?;
            if let Some(err) = response.error.as_ref() {
                if err.code == CONTENT_MODIFIED && attempt < max_attempts {
                    attempt += 1;
                    std::thread::sleep(delay);
                    continue;
                }
                return Err(DefinitionError::ServerError(format!(
                    "{} (code {})",
                    err.message, err.code
                )));
            }
            return match response.result {
                None => Ok(Vec::new()),
                Some(ref raw) if raw.is_null() => Ok(Vec::new()),
                Some(ref raw) => parse_locations(raw),
            };
        }
    }

    /// Single `textDocument/definition` round trip (no retry): returns the
    /// raw response so the caller can inspect the error code.
    fn send_definition(
        &self,
        uri: &str,
        position: Position,
    ) -> Result<LspResponse, DefinitionError> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier { uri: uri.to_string() },
            position,
        };
        let request = LspRequest::new(
            next_request_id(),
            "textDocument/definition",
            serde_json::to_value(&params)
                .map_err(|e| DefinitionError::InvalidResponse(format!("serialize params: {e}")))?,
        );
        self.client
            .send(request)
            .map_err(DefinitionError::ServerError)
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Normalize a `Location` / `Location[]` / `LocationLink[]` payload
/// into `Vec<Location>`. We try `LocationLink` first (it has
/// `targetUri` / `targetRange` keys) and fall back to plain
/// `Location`.
fn parse_locations(raw: &Value) -> DefinitionResult {
    match raw {
        Value::Null => Ok(Vec::new()),
        Value::Array(_) => {
            // Mixed array: each element might be Location or
            // LocationLink. Inspect the first key to dispatch.
            let arr = raw
                .as_array()
                .ok_or_else(|| DefinitionError::InvalidResponse("expected array".to_string()))?;
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(parse_single_location(item)?);
            }
            Ok(out)
        }
        Value::Object(_) => parse_locations_vec(raw),
        _ => Err(DefinitionError::InvalidResponse(format!(
            "unexpected payload kind: {}",
            raw
        ))),
    }
}

fn parse_locations_vec(raw: &Value) -> DefinitionResult {
    if let Some(items) = raw.get("items") {
        // Some servers wrap Location[] in `{ items: [...] }`. Match
        // both shapes (`Location[]` and `LocationList`).
        let arr = items
            .as_array()
            .ok_or_else(|| DefinitionError::InvalidResponse("items not an array".to_string()))?;
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            out.push(parse_single_location(item)?);
        }
        return Ok(out);
    }
    Ok(vec![parse_single_location(raw)?])
}

fn parse_single_location(item: &Value) -> Result<Location, DefinitionError> {
    // LocationLink: { targetUri, targetRange, targetSelectionRange,
    //                 originSelectionRange, originRange? }
    if let Some(uri) = item.get("targetUri").and_then(Value::as_str) {
        let range = item
            .get("targetRange")
            .ok_or_else(|| {
                DefinitionError::InvalidResponse("LocationLink missing 'targetRange'".to_string())
            })
            .and_then(range_from_value)?;
        return Ok(Location { uri: uri.to_string(), range });
    }
    // Location: { uri, range }
    if let (Some(uri), Some(range)) = (item.get("uri").and_then(Value::as_str), item.get("range")) {
        let range = range_from_value(range)?;
        return Ok(Location { uri: uri.to_string(), range });
    }
    Err(DefinitionError::InvalidResponse(format!(
        "object missing 'uri'+'range' or 'targetUri'+'targetRange': {}",
        item
    )))
}

fn range_from_value(v: &Value) -> Result<Range, DefinitionError> {
    let start = v
        .get("start")
        .ok_or_else(|| DefinitionError::InvalidResponse("range missing 'start'".to_string()))?;
    let end = v
        .get("end")
        .ok_or_else(|| DefinitionError::InvalidResponse("range missing 'end'".to_string()))?;
    Ok(Range {
        start: Position {
            line: start
                .get("line")
                .and_then(Value::as_u64)
                .ok_or_else(|| DefinitionError::InvalidResponse("range.start.line".to_string()))?
                as u32,
            character: start
                .get("character")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    DefinitionError::InvalidResponse("range.start.character".to_string())
                })? as u32,
        },
        end: Position {
            line: end
                .get("line")
                .and_then(Value::as_u64)
                .ok_or_else(|| DefinitionError::InvalidResponse("range.end.line".to_string()))?
                as u32,
            character: end
                .get("character")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    DefinitionError::InvalidResponse("range.end.character".to_string())
                })? as u32,
        },
    })
}

// ---------------------------------------------------------------------------
// Request id allocation (same scheme as hover.rs)
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
mod tests;
