use super::wire;
use crate::live_control::protocol::{Envelope, Payload};
use forge_domain::{ChatEventPayload, ContextMessage, Conversation, ConversationId, Role};
use serde_json::json;

pub async fn history(
    session: ConversationId,
    conversation: serde_json::Value,
) -> anyhow::Result<()> {
    let conversation: Conversation = serde_json::from_value(conversation)?;
    if let Some(context) = conversation.context {
        for entry in context.messages {
            match entry.message {
                ContextMessage::Text(message) => {
                    let kind = match message.role {
                        Role::User => "user_message_chunk",
                        Role::Assistant => "agent_message_chunk",
                        _ => continue,
                    };
                    wire::update(session, json!({"sessionUpdate":kind,"content":{"type":"text","text":message.content},
                        "_meta":{"io.phenotype/history":true}})).await?;
                }
                ContextMessage::Tool(result) => {
                    if let Some(id) = &result.call_id {
                        wire::update(session, json!({"sessionUpdate":"tool_call_update","toolCallId":id.to_string(),
                            "status":if result.is_error() {"failed"} else {"completed"},"rawOutput":result.output,
                            "_meta":{"io.phenotype/history":true}})).await?;
                    }
                }
                ContextMessage::Image(_) => {}
            }
        }
    }
    Ok(())
}

pub async fn event(event: &Envelope) -> anyhow::Result<()> {
    let metadata = json!({"io.phenotype/runtimeId":event.runtime_id,"io.phenotype/turnId":event.turn_id,
        "io.phenotype/sequence":event.sequence,"io.phenotype/eventId":event.event_id});
    let Payload::Chat { event: chat } = &event.payload else {
        return Ok(());
    };
    let update = match &chat.payload {
        ChatEventPayload::Message { text, .. } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}})
        }
        ChatEventPayload::Reasoning { text } => {
            json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":text}})
        }
        ChatEventPayload::ToolStart { tool_call } => {
            let Some(id) = &tool_call.call_id else {
                return Ok(());
            };
            json!({"sessionUpdate":"tool_call","toolCallId":id.to_string(),"title":tool_call.name.to_string(),
                "name":tool_call.name.to_string(),"status":"in_progress","rawInput":tool_call.arguments})
        }
        ChatEventPayload::ToolEnd { result } => {
            let Some(id) = &result.call_id else {
                return Ok(());
            };
            json!({"sessionUpdate":"tool_call_update","toolCallId":id.to_string(),
                "status":if result.is_error() {"failed"} else {"completed"},"rawOutput":result.output})
        }
        ChatEventPayload::ToolInput { title, .. } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":title}})
        }
        ChatEventPayload::ToolOutput { text } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}})
        }
        ChatEventPayload::Retry { cause, delay_ms } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("Retry in {delay_ms}ms: {cause}")}})
        }
        ChatEventPayload::Interrupt { reason } => {
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":reason}})
        }
        ChatEventPayload::Complete => return Ok(()),
    };
    let mut update = update;
    update["_meta"] = metadata;
    wire::update(event.session_id, update).await
}
