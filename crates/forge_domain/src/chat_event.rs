use serde::{Deserialize, Serialize};

use crate::{ChatResponse, ChatResponseContent, ToolCallFull, ToolResult};

/// Versioned, transport-safe projection of an observed agent response.
/// Runtime control tokens and tool-render acknowledgements never cross the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub version: u16,
    #[serde(flatten)]
    pub payload: ChatEventPayload,
}

/// Rich response payload shared by NDJSON and live session transports.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEventPayload {
    Message {
        text: String,
        partial: bool,
    },
    ToolInput {
        title: String,
        subtitle: Option<String>,
        category: String,
    },
    ToolOutput {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Complete,
    ToolStart {
        tool_call: ToolCallFull,
    },
    ToolEnd {
        result: ToolResult,
    },
    Retry {
        cause: String,
        delay_ms: u128,
    },
    Interrupt {
        reason: String,
    },
}

impl From<&ChatResponse> for ChatEvent {
    fn from(response: &ChatResponse) -> Self {
        let payload = match response {
            ChatResponse::TaskMessage { content } => match content {
                ChatResponseContent::Markdown { text, partial } => {
                    ChatEventPayload::Message { text: text.clone(), partial: *partial }
                }
                ChatResponseContent::ToolInput(title) => ChatEventPayload::ToolInput {
                    title: title.title.clone(),
                    subtitle: title.sub_title.clone(),
                    category: format!("{:?}", title.category).to_lowercase(),
                },
                ChatResponseContent::ToolOutput(text) => {
                    ChatEventPayload::ToolOutput { text: text.clone() }
                }
            },
            ChatResponse::TaskReasoning { content } => {
                ChatEventPayload::Reasoning { text: content.clone() }
            }
            ChatResponse::TaskComplete => ChatEventPayload::Complete,
            ChatResponse::ToolCallStart { tool_call, .. } => {
                ChatEventPayload::ToolStart { tool_call: tool_call.clone() }
            }
            ChatResponse::ToolCallEnd(result) => {
                ChatEventPayload::ToolEnd { result: result.clone() }
            }
            ChatResponse::RetryAttempt { cause, duration } => ChatEventPayload::Retry {
                cause: cause.as_str().to_owned(),
                delay_ms: duration.as_millis(),
            },
            ChatResponse::Interrupt { reason } => {
                ChatEventPayload::Interrupt { reason: format!("{reason:?}") }
            }
        };
        Self { version: 1, payload }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use std::sync::Arc;
    use tokio::sync::Notify;

    #[test]
    fn preserves_message_payload() {
        let fixture = ChatResponse::TaskMessage {
            content: ChatResponseContent::Markdown { text: "hello".into(), partial: true },
        };
        let actual = serde_json::to_value(ChatEvent::from(&fixture)).unwrap();
        assert_eq!(
            actual,
            serde_json::json!({
                "version": 1, "type": "message", "text": "hello", "partial": true
            })
        );
    }

    #[test]
    fn projecting_tool_does_not_acknowledge_execution() {
        let notifier = Arc::new(Notify::new());
        let fixture = ChatResponse::ToolCallStart {
            tool_call: ToolCallFull::new("test"),
            notifier: notifier.clone(),
        };
        let actual = serde_json::to_value(ChatEvent::from(&fixture)).unwrap();
        assert_eq!(actual["tool_call"]["name"], "test");
        assert!(actual.get("notifier").is_none());
        let mut pending = std::pin::pin!(notifier.notified());
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(pending.as_mut(), &mut context).is_pending());
    }
}
