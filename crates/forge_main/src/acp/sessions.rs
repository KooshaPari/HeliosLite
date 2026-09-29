use super::{
    Attachment, Bridge, projection, runtime,
    wire::{self, schema},
};
use crate::live_control::protocol::{Command, Request, VERSION};
use forge_api::{API, ForgeAPI};
use forge_domain::{ConversationId, Event};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

impl Bridge {
    pub(super) async fn open(
        &mut self,
        method: &str,
        params: Value,
        id: Option<Value>,
    ) -> anyhow::Result<()> {
        let id = id.ok_or_else(|| anyhow::anyhow!("request id required"))?;
        let create = method == "session/new";
        let params = if create {
            wire::checked::<schema::NewSessionRequest>(params)?
        } else {
            wire::checked::<schema::LoadSessionRequest>(params)?
        };
        anyhow::ensure!(
            params
                .get("mcpServers")
                .unwrap_or(&serde_json::Value::Null)
                .as_array()
                .is_none_or(Vec::is_empty),
            "per-client MCP servers not supported"
        );
        let cwd = PathBuf::from(
            params
                .get("cwd")
                .unwrap_or(&serde_json::Value::Null)
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("cwd required"))?,
        );
        anyhow::ensure!(
            cwd.is_absolute() && cwd.is_dir(),
            "cwd must be an existing absolute directory"
        );
        let cwd = cwd.canonicalize()?;
        let session = if create {
            ConversationId::generate()
        } else {
            ConversationId::parse(
                params
                    .get("sessionId")
                    .unwrap_or(&serde_json::Value::Null)
                    .as_str()
                    .unwrap_or_default(),
            )?
        };
        anyhow::ensure!(
            self.sessions.len() < 32 || self.sessions.contains_key(&session),
            "attachment_capacity"
        );
        let wants_control = create
            || params
                .pointer("/_meta/io.phenotype~1interactionController")
                .and_then(Value::as_bool)
                == Some(true);
        let (snapshot, state) = runtime::attach(session, &cwd, create).await?;
        if let Some(path) = snapshot.conversation.as_ref().and_then(|value| {
            value
                .get("cwd")
                .unwrap_or(&serde_json::Value::Null)
                .as_str()
        }) {
            anyhow::ensure!(Path::new(path).canonicalize()? == cwd, "workspace_mismatch");
        }
        if wants_control {
            runtime::control(session, snapshot.runtime_id, self.controller, false).await?;
        } else if self
            .sessions
            .get(&session)
            .is_some_and(|attachment| attachment.controls)
        {
            let _ = runtime::control(session, snapshot.runtime_id, self.controller, true).await;
            self.pending
                .retain(|_, request| request.session_id != session);
        }
        if let Some(conversation) = snapshot.conversation {
            projection::history(session, conversation).await?;
        }
        let mut finished = HashMap::new();
        for event in &snapshot.events {
            if let (
                Some(turn),
                crate::live_control::protocol::Payload::TurnFinished { status, .. },
            ) = (event.turn_id, &event.payload)
            {
                finished.insert(turn, status.clone());
            }
            if event.turn_id.is_some() && event.turn_id == snapshot.active_turn {
                projection::event(event).await?;
            }
        }
        self.sessions.insert(
            session,
            Attachment {
                runtime: snapshot.runtime_id,
                controls: wants_control,
                cursor: snapshot.sequence,
                current_turn: snapshot.active_turn,
                finished,
            },
        );
        let metadata = json!({"io.phenotype/runtimeId":snapshot.runtime_id,"io.phenotype/state":state,
            "io.phenotype/sequence":snapshot.sequence,"io.phenotype/interactionController":wants_control});
        let result = if create {
            wire::checked::<schema::NewSessionResponse>(
                json!({"sessionId":session,"_meta":metadata}),
            )?
        } else {
            wire::checked::<schema::LoadSessionResponse>(json!({"_meta":metadata}))?
        };
        wire::result(id, result).await
    }

    pub(super) async fn list(&self, params: Value, id: Option<Value>) -> anyhow::Result<()> {
        let id = id.ok_or_else(|| anyhow::anyhow!("request id required"))?;
        let params = wire::checked::<schema::ListSessionsRequest>(params)?;
        anyhow::ensure!(
            params.get("cursor").is_none_or(Value::is_null),
            "invalid_cursor"
        );
        let cwd = params
            .get("cwd")
            .unwrap_or(&serde_json::Value::Null)
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.cwd.clone());
        anyhow::ensure!(cwd.is_absolute() && cwd.is_dir(), "invalid cwd");
        let api = Arc::new(ForgeAPI::init(
            cwd.clone(),
            forge_config::ForgeConfig::read()?,
        ));
        let conversations = api.get_conversations(None).await?;
        let sessions: Vec<_> = conversations.into_iter().filter(|conversation| {
            params.get("cwd").is_none_or(Value::is_null) || conversation.cwd.as_deref() == cwd.to_str()
        }).map(|conversation| {
            let inferred = conversation.cwd.is_none();
            json!({"sessionId":conversation.id,"cwd":conversation.cwd.unwrap_or_else(|| cwd.to_string_lossy().into_owned()),
                "title":conversation.title,"_meta":{"io.phenotype/cwdInferred":inferred}})
        }).collect();
        wire::result(
            id,
            wire::checked::<schema::ListSessionsResponse>(json!({"sessions":sessions}))?,
        )
        .await
    }

    pub(super) async fn prompt(&mut self, params: Value, id: Option<Value>) -> anyhow::Result<()> {
        let id = id.ok_or_else(|| anyhow::anyhow!("request id required"))?;
        let params = wire::checked::<schema::PromptRequest>(params)?;
        let session = ConversationId::parse(
            params
                .get("sessionId")
                .unwrap_or(&serde_json::Value::Null)
                .as_str()
                .unwrap_or_default(),
        )?;
        let attachment = self
            .sessions
            .get_mut(&session)
            .ok_or_else(|| anyhow::anyhow!("session_not_loaded"))?;
        anyhow::ensure!(attachment.controls, "controller_required");
        let content = params
            .get("prompt")
            .unwrap_or(&serde_json::Value::Null)
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("prompt required"))?;
        let text = content
            .iter()
            .map(|block| {
                match block
                    .get("type")
                    .unwrap_or(&serde_json::Value::Null)
                    .as_str()
                {
                    Some("text") => Ok(block
                        .get("text")
                        .unwrap_or(&serde_json::Value::Null)
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()),
                    Some("resource_link") => Ok(format!(
                        "{}: {}",
                        block
                            .get("name")
                            .unwrap_or(&serde_json::Value::Null)
                            .as_str()
                            .unwrap_or("Resource"),
                        block
                            .get("uri")
                            .unwrap_or(&serde_json::Value::Null)
                            .as_str()
                            .unwrap_or_default()
                    )),
                    _ => Err(anyhow::anyhow!("unsupported prompt content")),
                }
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .join("\n");
        let command_id = match params
            .pointer("/_meta/io.phenotype~1commandId")
            .and_then(Value::as_str)
        {
            Some(id) => Uuid::parse_str(id)?,
            None => Uuid::new_v4(),
        };
        let result = runtime::call(&Request {
            version: VERSION,
            session_id: session,
            runtime_id: Some(attachment.runtime),
            controller_id: Some(self.controller),
            command: Command::Prompt { command_id, event: Event::new(text) },
        })
        .await?;
        let turn = Uuid::parse_str(
            result
                .get("turn_id")
                .unwrap_or(&serde_json::Value::Null)
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing turn identity"))?,
        )?;
        if let Some(status) = attachment.finished.get(&turn) {
            anyhow::ensure!(status != "failed", "previous command failed");
            return wire::result(
                id,
                wire::checked::<schema::PromptResponse>(
                    json!({"stopReason":if status == "cancelled" {"cancelled"} else {"end_turn"}}),
                )?,
            )
            .await;
        }
        anyhow::ensure!(
            !self.prompts.contains_key(&turn),
            "prompt already pending on this connection"
        );
        self.prompts.insert(turn, (session, id));
        attachment.current_turn = Some(turn);
        Ok(())
    }
}
