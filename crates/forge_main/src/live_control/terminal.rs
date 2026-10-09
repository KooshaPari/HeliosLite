use super::Handle;
use forge_domain::{ConversationId, InteractionAnswer, InteractionKind, InteractionResponse};
use std::collections::HashMap;
use uuid::Uuid;

/// Resolves only explicit response commands; ordinary input remains a chat turn.
pub(crate) fn command(
    text: &str,
    sessions: &HashMap<ConversationId, Handle>,
) -> anyhow::Result<bool> {
    let Some(arguments) = text.strip_prefix("/respond ") else {
        return Ok(false);
    };
    let (id, answer) = arguments
        .split_once(' ')
        .ok_or_else(|| anyhow::anyhow!("Usage: /respond REQUEST_ID ANSWER (or --cancel)"))?;
    let id = Uuid::parse_str(id)?;
    for handle in sessions.values() {
        if let Some(request) = handle
            .broker
            .snapshot()
            .into_iter()
            .find(|request| request.request_id == id)
        {
            let answer = if answer == "--cancel" {
                InteractionAnswer::Cancel
            } else if matches!(request.kind, InteractionKind::Text) {
                InteractionAnswer::Text(answer.to_owned())
            } else {
                InteractionAnswer::Choices(
                    answer
                        .split(',')
                        .map(|part| {
                            part.trim()
                                .parse::<usize>()?
                                .checked_sub(1)
                                .ok_or_else(|| anyhow::anyhow!("choices start at 1"))
                        })
                        .collect::<anyhow::Result<_>>()?,
                )
            };
            handle.broker.respond(InteractionResponse {
                request_id: id,
                session_id: request.session_id,
                runtime_id: request.runtime_id,
                turn_id: request.turn_id,
                answer,
            })?;
            return Ok(true);
        }
    }
    anyhow::bail!("request is no longer pending")
}
