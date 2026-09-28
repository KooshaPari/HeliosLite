//! ACP v1 NDJSON adapter backed by the exact live owner through private IPC.
mod interactions;
mod projection;
mod runtime;
mod sessions;
mod wire;

use crate::live_control::protocol::{Command, Request, Snapshot, VERSION};
use forge_domain::{ConversationId, InteractionRequest, InteractionResponse};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;
use wire::schema;

pub use runtime::host;
struct Attachment {
    runtime: Uuid,
    cursor: u64,
    current_turn: Option<Uuid>,
    finished: HashMap<Uuid, String>,
}
struct Bridge {
    initialized: bool,
    forms: bool,
    cwd: PathBuf,
    sessions: HashMap<ConversationId, Attachment>,
    pending: HashMap<String, InteractionRequest>,
    prompts: HashMap<Uuid, Value>,
}

pub async fn run(cwd: PathBuf) -> anyhow::Result<()> {
    let mut bridge = Bridge {
        initialized: false,
        forms: false,
        cwd,
        sessions: HashMap::new(),
        pending: HashMap::new(),
        prompts: HashMap::new(),
    };
    let (frames, mut incoming) = tokio::sync::mpsc::channel(16);
    let reader = tokio::spawn(async move {
        let mut input = tokio::io::BufReader::new(tokio::io::stdin());
        loop {
            let frame = crate::live_control::server::read_frame(&mut input).await;
            let done = !matches!(&frame, Ok(Some(_)));
            if frames.send(frame).await.is_err() || done {
                break;
            }
        }
    });
    let mut interval = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            frame = incoming.recv() => {
                let Some(frame) = frame else { break };
                let Some(frame) = frame? else { break };
                match serde_json::from_slice::<Value>(&frame) {
                    Ok(message) => {
                        let id = message.get("method").and_then(|_| message.get("id")).cloned();
                        if let Err(error) = bridge.message(message).await {
                            if let Some(id) = id { wire::error(id, -32602, error).await?; }
                        }
                    }
                    Err(error) => wire::error(Value::Null, -32700, error).await?,
                }
            }
            _ = interval.tick(), if bridge.initialized && !bridge.sessions.is_empty() => bridge.poll().await?,
        }
    }
    reader.abort();
    // Disconnect drops only this attachment. Runtime and held calls continue
    // until their own terminal/remote response, cancellation or TTL.
    Ok(())
}

impl Bridge {
    async fn message(&mut self, message: Value) -> anyhow::Result<()> {
        anyhow::ensure!(message["jsonrpc"] == "2.0", "invalid JSON-RPC version");
        let Some(method) = message["method"].as_str() else {
            return self.answer(message).await;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let id = message.get("id").cloned();
        if method == "initialize" {
            anyhow::ensure!(!self.initialized, "already_initialized");
            let request = wire::checked::<schema::InitializeRequest>(params)?;
            self.forms = request
                .pointer("/clientCapabilities/elicitation/form")
                .is_some_and(|form| !form.is_null());
            self.initialized = true;
            return wire::result(id.ok_or_else(|| anyhow::anyhow!("request id required"))?, wire::checked::<schema::InitializeResponse>(json!({
                "protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{}}},
                "agentInfo":{"name":"helioslite","title":"Forge / HeliosLite","version":env!("CARGO_PKG_VERSION")},"authMethods":[]
            }))?).await;
        }
        anyhow::ensure!(self.initialized, "initialize_required");
        match method {
            "session/new" | "session/load" => self.open(method, params, id).await,
            "session/list" => self.list(params, id).await,
            "session/prompt" => self.prompt(params, id).await,
            "session/cancel" => {
                let params = wire::checked::<schema::CancelNotification>(params)?;
                let session =
                    ConversationId::parse(params["sessionId"].as_str().unwrap_or_default())?;
                let attachment = self
                    .sessions
                    .get(&session)
                    .ok_or_else(|| anyhow::anyhow!("session_not_loaded"))?;
                if let Some(turn) = attachment.current_turn {
                    runtime::call(&Request {
                        version: VERSION,
                        session_id: session,
                        runtime_id: Some(attachment.runtime),
                        command: Command::Cancel { turn_id: turn },
                    })
                    .await?;
                }
                Ok(())
            }
            _ => {
                if let Some(id) = id {
                    wire::error(id, -32601, "method not supported").await?;
                }
                Ok(())
            }
        }
    }

    async fn answer(&mut self, message: Value) -> anyhow::Result<()> {
        let Some(id) = message["id"].as_str() else {
            return Ok(());
        };
        let Some(request) = self.pending.get(id) else {
            return Ok(());
        };
        let answer = if message.get("error").is_some() {
            forge_domain::InteractionAnswer::Cancel
        } else {
            interactions::answer(request, message["result"].clone())?
        };
        let response = InteractionResponse {
            request_id: request.request_id,
            session_id: request.session_id,
            runtime_id: request.runtime_id,
            turn_id: request.turn_id,
            answer,
        };
        runtime::call(&Request {
            version: VERSION,
            session_id: request.session_id,
            runtime_id: Some(request.runtime_id),
            command: Command::Respond { response },
        })
        .await?;
        self.pending.remove(id);
        Ok(())
    }

    async fn poll(&mut self) -> anyhow::Result<()> {
        let requests = self.sessions.iter().map(|(session, attachment)| {
            let session = *session;
            let runtime = attachment.runtime;
            let cursor = attachment.cursor;
            async move {
                (
                    session,
                    runtime::snapshot(session, Some(runtime), Some(cursor)).await,
                )
            }
        });
        let snapshots = futures::future::join_all(requests).await;
        for (session, snapshot) in snapshots {
            match snapshot {
                Ok(snapshot) => self.consume(snapshot).await?,
                Err(error) => {
                    if let Some(attachment) = self.sessions.remove(&session) {
                        if let Some(turn) = attachment.current_turn {
                            if let Some(id) = self.prompts.remove(&turn) {
                                wire::error(id, -32000, error).await?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn consume(&mut self, snapshot: Snapshot) -> anyhow::Result<()> {
        let session = snapshot.session_id;
        let attachment = self
            .sessions
            .get_mut(&session)
            .ok_or_else(|| anyhow::anyhow!("session_not_loaded"))?;
        if snapshot.resync_required {
            wire::update(session, json!({"sessionUpdate":"session_info_update","_meta":{
                "io.phenotype/resyncRequired":true,"io.phenotype/runtimeId":snapshot.runtime_id,"io.phenotype/sequence":snapshot.sequence}})).await?;
            if let Some(conversation) = snapshot.conversation {
                projection::history(session, conversation).await?;
            }
        } else {
            for event in &snapshot.events {
                projection::event(event).await?;
            }
        }
        for event in snapshot.events {
            if let (
                Some(turn),
                crate::live_control::protocol::Payload::TurnFinished { status, error },
            ) = (event.turn_id, event.payload)
            {
                attachment.finished.insert(turn, status.clone());
                if let Some(id) = self.prompts.remove(&turn) {
                    if status == "failed" {
                        wire::error(id, -32000, error.unwrap_or_else(|| "turn failed".into()))
                            .await?;
                    } else {
                        wire::result(id, wire::checked::<schema::PromptResponse>(json!({"stopReason":if status == "cancelled" {"cancelled"} else {"end_turn"}}))?).await?;
                    }
                }
            }
        }
        attachment.cursor = snapshot.sequence;
        attachment.current_turn = snapshot.active_turn.or(attachment
            .current_turn
            .filter(|turn| !attachment.finished.contains_key(turn)));
        self.pending.retain(|_, request| {
            request.session_id != session
                || snapshot
                    .pending
                    .iter()
                    .any(|pending| pending.request_id == request.request_id)
        });
        for request in snapshot.pending {
            let id = format!("forge/{}", request.request_id);
            if !self.pending.contains_key(&id) {
                if let Some(message) = interactions::request(&request, self.forms)? {
                    wire::send(message).await?;
                    self.pending.insert(id, request);
                }
            }
        }
        Ok(())
    }
}
