use chrono::{DateTime, Utc};
#[cfg(unix)]
use forge_domain::InteractionResponse;
use forge_domain::{ChatEvent, ConversationId, InteractionRequest};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use uuid::Uuid;

pub const VERSION: u16 = 1;
pub const REPLAY_LIMIT: usize = 2048;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    pub session_id: ConversationId,
    pub runtime_id: Uuid,
    pub event_id: Uuid,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub turn_id: Option<Uuid>,
    pub payload: Payload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Payload {
    TurnStarted,
    Chat {
        event: ChatEvent,
    },
    TurnFinished {
        status: String,
        error: Option<String>,
    },
    InteractionsChanged,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u16,
    pub session_id: ConversationId,
    pub runtime_id: Uuid,
    pub active_turn: Option<Uuid>,
    pub queued_turns: Vec<Uuid>,
    pub sequence: u64,
    pub events: Vec<Envelope>,
    pub resync_required: bool,
    pub pending: Vec<InteractionRequest>,
    pub conversation: Option<serde_json::Value>,
    pub controlled: bool,
}

#[cfg(unix)]
#[derive(Debug, Deserialize, Serialize)]
pub struct Request {
    pub version: u16,
    pub session_id: ConversationId,
    pub runtime_id: Option<Uuid>,
    #[serde(default)]
    pub controller_id: Option<Uuid>,
    #[serde(flatten)]
    pub command: Command,
}

#[cfg(unix)]
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Command {
    ClaimControl,
    ReleaseControl,
    Snapshot {
        after: Option<u64>,
    },
    Prompt {
        command_id: Uuid,
        event: forge_domain::Event,
    },
    Cancel {
        turn_id: Uuid,
    },
    Respond {
        response: InteractionResponse,
    },
}

pub struct Journal {
    pub session_id: ConversationId,
    pub runtime_id: Uuid,
    pub active_turn: Option<Uuid>,
    pub queued_turns: Vec<Uuid>,
    pub sequence: u64,
    events: VecDeque<Envelope>,
}

impl Journal {
    pub fn new(session_id: ConversationId, runtime_id: Uuid) -> Self {
        Self {
            session_id,
            runtime_id,
            active_turn: None,
            queued_turns: Vec::new(),
            sequence: 0,
            events: VecDeque::new(),
        }
    }
    pub fn publish(&mut self, turn_id: Option<Uuid>, payload: Payload) {
        self.sequence += 1;
        self.events.push_back(Envelope {
            version: VERSION,
            session_id: self.session_id,
            runtime_id: self.runtime_id,
            event_id: Uuid::new_v4(),
            sequence: self.sequence,
            timestamp: Utc::now(),
            turn_id,
            payload,
        });
        if self.events.len() > REPLAY_LIMIT {
            self.events.pop_front();
        }
    }
    pub fn snapshot(&self, after: Option<u64>, pending: Vec<InteractionRequest>) -> Snapshot {
        let cursor = after.unwrap_or(0);
        let resync_required = cursor > self.sequence
            || self
                .events
                .front()
                .is_some_and(|first| cursor.saturating_add(1) < first.sequence);
        Snapshot {
            version: VERSION,
            session_id: self.session_id,
            runtime_id: self.runtime_id,
            active_turn: self.active_turn,
            queued_turns: self.queued_turns.clone(),
            sequence: self.sequence,
            events: self
                .events
                .iter()
                .filter(|event| event.sequence > cursor)
                .cloned()
                .collect(),
            resync_required,
            pending,
            conversation: None,
            controlled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_and_future_cursors_require_explicit_resync() {
        let mut journal = Journal::new(ConversationId::generate(), Uuid::new_v4());
        for _ in 0..REPLAY_LIMIT + 1 {
            journal.publish(None, Payload::InteractionsChanged);
        }
        assert!(journal.snapshot(Some(0), vec![]).resync_required);
        assert!(!journal.snapshot(Some(1), vec![]).resync_required);
        assert!(journal.snapshot(Some(u64::MAX), vec![]).resync_required);
        let replay = journal.snapshot(Some(REPLAY_LIMIT as u64), vec![]);
        assert_eq!(replay.events.len(), 1);
        assert_eq!(replay.events[0].sequence, REPLAY_LIMIT as u64 + 1);
    }
}
