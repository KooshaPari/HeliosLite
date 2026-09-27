use super::*;
use crate::lsp_client::LspResponse;
use std::sync::{Arc, Mutex};

pub struct MockLspClient {
    pub name: &'static str,
    pub scripted: Mutex<Vec<Result<LspResponse, String>>>,
    pub captured: Mutex<Vec<LspRequest>>,
}

impl MockLspClient {
    pub fn new(name: &'static str, response: LspResponse) -> Self {
        Self {
            name,
            scripted: Mutex::new(vec![Ok(response)]),
            captured: Mutex::new(Vec::new()),
        }
    }

    pub fn failing(name: &'static str, err: &str) -> Self {
        Self {
            name,
            scripted: Mutex::new(vec![Err(err.to_string())]),
            captured: Mutex::new(Vec::new()),
        }
    }

    /// Scripted response *sequence*: each `send` consumes the next
    /// entry (used to exercise the ContentModified retry path).
    pub fn seq(name: &'static str, responses: Vec<Result<LspResponse, String>>) -> Self {
        Self {
            name,
            scripted: Mutex::new(responses),
            captured: Mutex::new(Vec::new()),
        }
    }

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
        let next = {
            let mut queue = self.scripted.lock().unwrap();
            if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            }
        };
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
        error: Some(crate::lsp_client::LspError { code, message: message.to_string(), data: None }),
    }
}

#[test]
fn definition_supports_common_languages() {
    let client = Arc::new(MockLspClient::new(
        "rust-analyzer",
        ok_response(
            json!({"uri":"file:///foo.rs","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}}}),
        ),
    ));
    let p = DefinitionProvider::new(client);
    assert!(p.supports(Path::new("foo.rs")));
    assert!(p.supports(Path::new("foo.tsx")));
    assert!(p.supports(Path::new("foo.JS")));
    assert!(p.supports(Path::new("foo.mts")));
    assert!(p.supports(Path::new("foo.cts")));
    assert!(!p.supports(Path::new("foo.py")));
}

#[test]
fn definition_returns_empty_when_server_returns_null() {
    let client = Arc::new(MockLspClient::new(
        "rust-analyzer",
        ok_response(Value::Null),
    ));
    let p = DefinitionProvider::new(client);
    let locs = p
        .definition("file:///foo.rs", Position { line: 0, character: 0 })
        .unwrap();
    assert!(locs.is_empty());
}

#[test]
fn definition_parses_single_location() {
    let client = Arc::new(MockLspClient::new(
        "rust-analyzer",
        ok_response(json!({
            "uri": "file:///lib.rs",
            "range": {
                "start": {"line": 10, "character": 0},
                "end":   {"line": 10, "character": 4}
            }
        })),
    ));
    let p = DefinitionProvider::new(client);
    let locs = p
        .definition("file:///foo.rs", Position { line: 5, character: 2 })
        .unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs.first().map(|x| x.uri.as_str()), Some("file:///lib.rs"));
    assert_eq!(locs.first().map(|x| x.range.start.line), Some(10));
    assert_eq!(locs.first().map(|x| x.range.end.character), Some(4));
}

#[test]
fn definition_parses_array_of_locations() {
    let client = Arc::new(MockLspClient::new(
        "tsserver",
        ok_response(json!([
            {"uri":"file:///a.ts","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}}},
            {"uri":"file:///b.ts","range":{"start":{"line":2,"character":0},"end":{"line":2,"character":3}}}
        ])),
    ));
    let p = DefinitionProvider::new(client);
    let locs = p
        .definition("file:///foo.ts", Position { line: 9, character: 0 })
        .unwrap();
    assert_eq!(locs.len(), 2);
    assert_eq!(locs.first().map(|x| x.uri.as_str()), Some("file:///a.ts"));
    assert_eq!(locs.get(1).map(|x| x.uri.as_str()), Some("file:///b.ts"));
}

#[test]
fn definition_parses_location_link_payload() {
    // rust-analyzer with linkSupport returns LocationLink[].
    let client = Arc::new(MockLspClient::new(
        "rust-analyzer",
        ok_response(json!([
            {
                "originSelectionRange": {
                    "start": {"line": 5, "character": 4},
                    "end":   {"line": 5, "character": 8}
                },
                "targetUri": "file:///lib.rs",
                "targetRange": {
                    "start": {"line": 10, "character": 0},
                    "end":   {"line": 10, "character": 8}
                },
                "targetSelectionRange": {
                    "start": {"line": 10, "character": 3},
                    "end":   {"line": 10, "character": 6}
                }
            }
        ])),
    ));
    let p = DefinitionProvider::new(client);
    let locs = p
        .definition("file:///foo.rs", Position { line: 5, character: 5 })
        .unwrap();
    assert_eq!(locs.len(), 1);
    assert_eq!(locs.first().map(|x| x.uri.as_str()), Some("file:///lib.rs"));
    assert_eq!(locs.first().map(|x| x.range.start.line), Some(10));
    assert_eq!(locs.first().map(|x| x.range.end.character), Some(8));
}

#[test]
fn definition_propagates_server_error() {
    let client = Arc::new(MockLspClient::new(
        "rust-analyzer",
        err_response(-32601, "method not found"),
    ));
    let p = DefinitionProvider::new(client);
    let r = p.definition("file:///foo.rs", Position { line: 0, character: 0 });
    assert!(matches!(r, Err(DefinitionError::ServerError(_))));
}

#[test]
fn definition_surfaces_transport_failure() {
    let client = Arc::new(MockLspClient::failing("rust-analyzer", "io: broken pipe"));
    let p = DefinitionProvider::new(client);
    let r = p.definition("file:///foo.rs", Position { line: 0, character: 0 });
    assert!(matches!(r, Err(DefinitionError::ServerError(m)) if m == "io: broken pipe"));
}

#[test]
fn definition_rejects_malformed_payload() {
    let client = Arc::new(MockLspClient::new(
        "rust-analyzer",
        ok_response(json!("just a string")),
    ));
    let p = DefinitionProvider::new(client);
    let r = p.definition("file:///foo.rs", Position { line: 0, character: 0 });
    assert!(matches!(r, Err(DefinitionError::InvalidResponse(_))));
}

#[test]
fn definition_retries_content_modified_then_succeeds() {
    // A cold rust-analyzer answers the first definition request with
    // ContentModified; the re-issued request on the same session lands.
    let location = json!([{
        "uri": "file:///lib.rs",
        "range": {
            "start": {"line": 10, "character": 0},
            "end":   {"line": 10, "character": 8}
        }
    }]);
    let client = Arc::new(MockLspClient::seq(
        "rust-analyzer",
        vec![
            Ok(err_response(CONTENT_MODIFIED, "content modified")),
            Ok(ok_response(location)),
        ],
    ));
    let p = DefinitionProvider::new(client.clone());
    let locs = p
        .definition_with_retry(
            "file:///foo.rs",
            Position { line: 5, character: 5 },
            5,
            Duration::from_millis(1),
        )
        .expect("retry should recover from ContentModified");
    assert_eq!(locs.len(), 1);
    assert_eq!(
        client.captured.lock().unwrap().len(),
        2,
        "request re-issued"
    );
}

#[test]
fn definition_surfaces_content_modified_after_retry_budget() {
    let client = Arc::new(MockLspClient::seq(
        "rust-analyzer",
        vec![
            Ok(err_response(CONTENT_MODIFIED, "content modified")),
            Ok(err_response(CONTENT_MODIFIED, "content modified")),
            Ok(err_response(CONTENT_MODIFIED, "content modified")),
        ],
    ));
    let p = DefinitionProvider::new(client.clone());
    let r = p.definition_with_retry(
        "file:///foo.rs",
        Position { line: 0, character: 0 },
        3,
        Duration::from_millis(1),
    );
    assert!(matches!(
        r,
        Err(DefinitionError::ServerError(m)) if m.contains("(code -32801)")
    ));
    assert_eq!(
        client.captured.lock().unwrap().len(),
        3,
        "one send per attempt"
    );
}

#[test]
fn definition_does_not_retry_other_error_codes() {
    let client = Arc::new(MockLspClient::seq(
        "rust-analyzer",
        vec![Ok(err_response(-32601, "method not found"))],
    ));
    let p = DefinitionProvider::new(client.clone());
    let r = p.definition_with_retry(
        "file:///foo.rs",
        Position { line: 0, character: 0 },
        20,
        Duration::from_millis(400),
    );
    assert!(matches!(
        r,
        Err(DefinitionError::ServerError(m)) if m == "method not found (code -32601)"
    ));
    assert_eq!(
        client.captured.lock().unwrap().len(),
        1,
        "non-transient errors must not be re-issued"
    );
}
