use super::actor::{Control, Observer};
use super::protocol::Snapshot;
use forge_api::API;
use forge_domain::{
    ChatRequest, ChatResponse, ConversationId, InteractionBroker, InteractionResponse,
};
use std::fs::File;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

#[derive(Clone)]
pub struct Handle {
    pub session_id: ConversationId,
    pub runtime_id: Uuid,
    pub broker: Arc<InteractionBroker>,
    pub(super) tx: mpsc::UnboundedSender<Control>,
    pub(super) output: Arc<Mutex<Option<std::sync::mpsc::SyncSender<String>>>>,
}

pub struct TurnReceiver {
    turn: Uuid,
    handle: Handle,
    receiver: mpsc::UnboundedReceiver<anyhow::Result<ChatResponse>>,
    finished: bool,
}
impl TurnReceiver {
    pub async fn recv(&mut self) -> Option<anyhow::Result<ChatResponse>> {
        let result = self.receiver.recv().await;
        if result.is_none() {
            self.finished = true;
        }
        result
    }
}
impl Drop for TurnReceiver {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.handle.tx.send(Control::Cancel {
                turn: self.turn,
                controller: None,
                reply: None,
            });
        }
    }
}

impl Handle {
    pub fn set_output(&self, output: Option<std::sync::mpsc::SyncSender<String>>) {
        *self.output.lock().unwrap() = output;
    }

    pub fn spawn<A: API + 'static>(
        api: Arc<A>,
        session_id: ConversationId,
        lease: File,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let runtime_id = Uuid::new_v4();
        let ttl = std::env::var("FORGE_INTERACTION_TTL_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0 && *seconds <= 86400)
            .unwrap_or(300);
        let broker = InteractionBroker::new(session_id, runtime_id, Duration::from_secs(ttl));
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = Self {
            session_id,
            runtime_id,
            broker,
            tx,
            output: Arc::new(Mutex::new(None)),
        };
        let task = tokio::spawn(super::actor::run(api, handle.clone(), rx, lease));
        (handle, task)
    }
    async fn submit(
        &self,
        command: Uuid,
        request: ChatRequest,
        observer: Option<Observer>,
        controller: Option<Uuid>,
    ) -> anyhow::Result<Uuid> {
        anyhow::ensure!(
            request.conversation_id == self.session_id,
            "session_mismatch"
        );
        let (reply, receiver) = oneshot::channel();
        self.tx
            .send(Control::Prompt { command, request, observer, controller, reply })?;
        receiver.await?
    }
    pub async fn prompt(
        &self,
        controller: Uuid,
        command: Uuid,
        request: ChatRequest,
    ) -> anyhow::Result<Uuid> {
        self.submit(command, request, None, Some(controller)).await
    }
    pub async fn local_prompt(&self, request: ChatRequest) -> anyhow::Result<TurnReceiver> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let turn = self
            .submit(Uuid::new_v4(), request, Some(sender), None)
            .await?;
        Ok(TurnReceiver { turn, handle: self.clone(), receiver, finished: false })
    }
    pub async fn cancel(&self, controller: Uuid, turn: Uuid) -> anyhow::Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.tx
            .send(Control::Cancel { turn, controller: Some(controller), reply: Some(reply) })?;
        receiver.await?
    }
    pub async fn snapshot(
        &self,
        after: Option<u64>,
        controller: Option<Uuid>,
    ) -> anyhow::Result<Snapshot> {
        let (reply, receiver) = oneshot::channel();
        self.tx
            .send(Control::Snapshot { after, controller, reply })?;
        receiver.await?
    }
    pub async fn control(&self, controller: Uuid, release: bool) -> anyhow::Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.tx
            .send(Control::Lease { controller, release, reply })?;
        receiver.await?
    }

    pub async fn respond(
        &self,
        controller: Uuid,
        response: InteractionResponse,
    ) -> anyhow::Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.tx
            .send(Control::Respond { controller, response, reply })?;
        receiver.await?
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Control::Shutdown);
    }
}
