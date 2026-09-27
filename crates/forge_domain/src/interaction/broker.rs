use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::ConversationId;
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

struct Pending {
    request: InteractionRequest,
    deadline: Instant,
    sender: oneshot::Sender<InteractionAnswer>,
}

/// One runtime's pending calls. The mutex makes all responders compete atomically.
pub struct InteractionBroker {
    session_id: ConversationId,
    runtime_id: Uuid,
    ttl: Duration,
    pending: Mutex<HashMap<Uuid, Pending>>,
    changed: watch::Sender<u64>,
}

/// Drop guard: aborted turns settle their held calls as cancellation.
pub struct PendingInteraction {
    pub request: InteractionRequest,
    broker: Arc<InteractionBroker>,
    receiver: Option<oneshot::Receiver<InteractionAnswer>>,
}

impl InteractionBroker {
    /// Creates a runtime-scoped broker with an explicit positive TTL.
    pub fn new(session_id: ConversationId, runtime_id: Uuid, ttl: Duration) -> Arc<Self> {
        let (changed, _) = watch::channel(0);
        Arc::new(Self {
            session_id,
            runtime_id,
            ttl,
            pending: Mutex::new(HashMap::new()),
            changed,
        })
    }

    /// Subscribes to changes; obtain a fresh snapshot after every notification.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn notify(&self) {
        self.changed
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    /// Opens a held operation. The returned guard must live until it is settled.
    pub fn open(
        self: &Arc<Self>,
        turn_id: Uuid,
        kind: InteractionKind,
        message: String,
        choices: Vec<String>,
    ) -> PendingInteraction {
        let request = InteractionRequest {
            request_id: Uuid::new_v4(),
            session_id: self.session_id,
            runtime_id: self.runtime_id,
            turn_id,
            expires_at: chrono::Utc::now()
                + chrono::Duration::from_std(self.ttl).unwrap_or_default(),
            message,
            choices,
            kind,
        };
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().unwrap().insert(
            request.request_id,
            Pending {
                request: request.clone(),
                deadline: Instant::now() + self.ttl,
                sender,
            },
        );
        self.notify();
        PendingInteraction { request, broker: self.clone(), receiver: Some(receiver) }
    }

    /// Returns recoverable requests; expired calls are settled, never replayed.
    pub fn snapshot(&self) -> Vec<InteractionRequest> {
        self.expire();
        self.pending
            .lock()
            .unwrap()
            .values()
            .map(|pending| pending.request.clone())
            .collect()
    }

    /// Settles only a valid, unexpired response to the exact session/runtime/turn.
    /// Invalid, duplicate and stale responses leave other pending calls untouched.
    pub fn respond(&self, response: InteractionResponse) -> anyhow::Result<()> {
        let mut pending = self.pending.lock().unwrap();
        let Some(call) = pending.get(&response.request_id) else {
            anyhow::bail!("request_not_pending")
        };
        anyhow::ensure!(
            response.session_id == self.session_id
                && response.runtime_id == self.runtime_id
                && response.turn_id == call.request.turn_id,
            "request_identity_mismatch"
        );
        if call.deadline <= Instant::now() {
            let call = pending.remove(&response.request_id).unwrap();
            let _ = call.sender.send(InteractionAnswer::Cancel);
            drop(pending);
            self.notify();
            anyhow::bail!("request_expired");
        }
        anyhow::ensure!(call.request.accepts(&response.answer), "invalid_answer");
        let call = pending.remove(&response.request_id).unwrap();
        let result = call.sender.send(response.answer);
        drop(pending);
        self.notify();
        result.map_err(|_| anyhow::anyhow!("request_cancelled"))
    }

    fn cancel(&self, request_id: Uuid) {
        if let Some(call) = self.pending.lock().unwrap().remove(&request_id) {
            let _ = call.sender.send(InteractionAnswer::Cancel);
            self.notify();
        }
    }

    /// Cancels only the named turn's requests.
    pub fn cancel_turn(&self, turn_id: Uuid) {
        let ids: Vec<_> = self
            .pending
            .lock()
            .unwrap()
            .values()
            .filter(|call| call.request.turn_id == turn_id)
            .map(|call| call.request.request_id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }

    fn expire(&self) {
        let ids: Vec<_> = self
            .pending
            .lock()
            .unwrap()
            .values()
            .filter(|call| call.deadline <= Instant::now())
            .map(|call| call.request.request_id)
            .collect();
        for id in ids {
            self.cancel(id);
        }
    }
}

impl PendingInteraction {
    /// Waits for one answer; timeout and sender loss fail closed.
    pub async fn wait(mut self) -> InteractionAnswer {
        let receiver = self.receiver.take().unwrap();
        tokio::time::timeout(self.broker.ttl, receiver)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(InteractionAnswer::Cancel)
    }
}

impl Drop for PendingInteraction {
    fn drop(&mut self) {
        self.broker.cancel(self.request.request_id);
    }
}
