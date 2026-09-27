use crate::{ConversationId, PermissionOperation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Original policy choices; labels and persistence behavior remain unchanged.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    strum_macros::Display,
    strum_macros::EnumIter,
    strum_macros::EnumString,
)]
pub enum PolicyPermission {
    #[strum(to_string = "Accept")]
    Accept,
    #[strum(to_string = "Reject")]
    Reject,
    #[strum(to_string = "Accept and Remember")]
    AcceptAndRemember,
}

/// Exact operation that is waiting for user input.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionKind {
    Permission { operation: PermissionOperation },
    Text,
    SingleChoice,
    MultipleChoice,
}

/// An offered choice uses its index, never an inferred display label, as identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRequest {
    pub request_id: Uuid,
    pub session_id: ConversationId,
    pub runtime_id: Uuid,
    pub turn_id: Uuid,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub message: String,
    pub choices: Vec<String>,
    pub kind: InteractionKind,
}

/// A response addresses the complete original request identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionResponse {
    pub request_id: Uuid,
    pub session_id: ConversationId,
    pub runtime_id: Uuid,
    pub turn_id: Uuid,
    pub answer: InteractionAnswer,
}

/// Cancellation is a first-class answer; permission callers interpret it as deny.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum InteractionAnswer {
    Cancel,
    Text(String),
    Choices(Vec<usize>),
}

impl InteractionRequest {
    /// Validates response shape and offered option indices before settlement.
    pub fn accepts(&self, answer: &InteractionAnswer) -> bool {
        match (answer, &self.kind) {
            (InteractionAnswer::Cancel, _) => true,
            (InteractionAnswer::Text(_), InteractionKind::Text) => true,
            (InteractionAnswer::Choices(indices), kind) => {
                let shape = match kind {
                    InteractionKind::Permission { .. } | InteractionKind::SingleChoice => {
                        indices.len() == 1
                    }
                    InteractionKind::MultipleChoice => true,
                    InteractionKind::Text => false,
                };
                shape
                    && indices.iter().enumerate().all(|(offset, index)| {
                        *index < self.choices.len() && !indices[..offset].contains(index)
                    })
            }
            _ => false,
        }
    }
}
