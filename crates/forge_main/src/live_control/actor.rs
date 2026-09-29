use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::sync::{Arc, Mutex};

use forge_api::API;
use forge_domain::{
    ChatEvent, ChatRequest, ChatResponse, INTERACTION_CONTEXT, InteractionContext,
    InteractionResponse,
};
use futures::StreamExt;
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;

use super::controller::Controller;
use super::handle::Handle;
use super::protocol::{Journal, Payload, Snapshot};

pub(super) type Observer = mpsc::UnboundedSender<anyhow::Result<ChatResponse>>;
struct Turn {
    id: Uuid,
    request: ChatRequest,
    observer: Option<Observer>,
}

pub(super) enum Control {
    Prompt {
        command: Uuid,
        request: ChatRequest,
        observer: Option<Observer>,
        controller: Option<Uuid>,
        reply: oneshot::Sender<anyhow::Result<Uuid>>,
    },
    Cancel {
        turn: Uuid,
        controller: Option<Uuid>,
        reply: Option<oneshot::Sender<anyhow::Result<()>>>,
    },
    Snapshot {
        after: Option<u64>,
        controller: Option<Uuid>,
        reply: oneshot::Sender<anyhow::Result<Snapshot>>,
    },
    Lease {
        controller: Uuid,
        release: bool,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Respond {
        controller: Uuid,
        response: InteractionResponse,
        reply: oneshot::Sender<anyhow::Result<()>>,
    },
    Shutdown,
}

pub(super) async fn run<A: API + 'static>(
    api: Arc<A>,
    handle: Handle,
    mut commands: mpsc::UnboundedReceiver<Control>,
    _lease: File,
) {
    let journal = Arc::new(Mutex::new(Journal::new(
        handle.session_id,
        handle.runtime_id,
    )));
    let mut queue = VecDeque::<Turn>::new();
    let mut accepted = HashMap::<Uuid, (String, Uuid)>::new();
    let (completed, mut completions) = mpsc::unbounded_channel();
    let mut active: Option<(Uuid, watch::Sender<bool>, tokio::task::JoinHandle<()>)> = None;
    let mut interaction_changes = handle.broker.subscribe();
    let mut shutdown = false;
    let mut remote_turns = std::collections::HashSet::new();
    let mut shown = std::collections::HashSet::new();
    let mut controller_lease = Controller::default();
    loop {
        if active.is_none() {
            if shutdown {
                break;
            }
            if let Some(turn) = queue.pop_front() {
                let (cancel, cancellation) = watch::channel(false);
                let id = turn.id;
                {
                    let mut journal = journal.lock().unwrap();
                    journal.active_turn = Some(id);
                    journal.queued_turns.retain(|queued| *queued != id);
                    journal.publish(Some(id), Payload::TurnStarted);
                }
                if turn.observer.is_none() {
                    remote_turns.insert(id);
                }
                let context = InteractionContext {
                    broker: handle.broker.clone(),
                    turn_id: id,
                    cancel: cancellation,
                    terminal: turn.observer.is_some(),
                    tool_call: None,
                };
                let api = api.clone();
                let journal = journal.clone();
                let completed = completed.clone();
                let output = handle.output.lock().unwrap().clone();
                let task = tokio::spawn(async move {
                    run_turn(api, turn, context, journal, output).await;
                    let _ = completed.send(id);
                });
                active = Some((id, cancel, task));
            }
        }
        tokio::select! {
            Some(done) = completions.recv(), if active.is_some() => {
                if active.as_ref().is_some_and(|(id, _, _)| *id == done) {
                    if let Some((_, _, task)) = active.take() { let _ = task.await; }
                    journal.lock().unwrap().active_turn = None;
                }
            }
            _ = interaction_changes.changed() => {
                journal.lock().unwrap().publish(active.as_ref().map(|(id, _, _)| *id), Payload::InteractionsChanged);
                let output = handle.output.lock().unwrap().clone();
                for request in handle.broker.snapshot() {
                    if remote_turns.contains(&request.turn_id) && shown.insert(request.request_id) {
                        super::display::interaction(&output, &request);
                    }
                }
            }
            command = commands.recv(), if !shutdown => {
                match command {
                    Some(Control::Prompt { command, request, observer, controller, reply }) => {
                        if let Some(controller) = controller
                            && let Err(error) = controller_lease.require(controller)
                        {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                        let encoded = match fingerprint(&request) {
                            Ok(encoded) => encoded,
                            Err(error) => { let _ = reply.send(Err(error.into())); continue; }
                        };
                        if let Some((original, turn)) = accepted.get(&command) {
                            let result = if *original == encoded { Ok(*turn) } else { Err(anyhow::anyhow!("command_id_conflict")) };
                            let _ = reply.send(result);
                        } else if queue.len() >= 64 || accepted.len() >= 4096 {
                            let _ = reply.send(Err(anyhow::anyhow!("session_queue_or_idempotency_capacity")));
                        } else {
                            let id = Uuid::new_v4();
                            accepted.insert(command, (encoded, id));
                            queue.push_back(Turn { id, request, observer });
                            journal.lock().unwrap().queued_turns.push(id);
                            let _ = reply.send(Ok(id));
                        }
                    }
                    Some(Control::Cancel { turn, controller, reply }) => {
                        if let Some(controller) = controller
                            && let Err(error) = controller_lease.require(controller)
                        {
                            if let Some(reply) = reply { let _ = reply.send(Err(error)); }
                            continue;
                        }
                        let result = if let Some((_, cancel, _)) = active.as_ref().filter(|(id, _, _)| *id == turn) {
                            handle.broker.cancel_turn(turn);
                            let _ = cancel.send(true);
                            Ok(())
                        } else if let Some(index) = queue.iter().position(|queued| queued.id == turn) {
                            let cancelled = queue.remove(index).unwrap();
                            if let Some(observer) = cancelled.observer { let _ = observer.send(Err(anyhow::anyhow!("turn cancelled"))); }
                            let mut journal = journal.lock().unwrap();
                            journal.queued_turns.retain(|id| *id != turn);
                            journal.publish(Some(turn), Payload::TurnFinished { status: "cancelled".into(), error: None });
                            Ok(())
                        } else { Err(anyhow::anyhow!("turn_not_active_or_queued")) };
                        if let Some(reply) = reply { let _ = reply.send(result); }
                    }
                    Some(Control::Snapshot { after, controller, reply }) => {
                        let pending = handle.broker.snapshot();
                        let mut snapshot = journal.lock().unwrap().snapshot(after, pending);
                        snapshot.controlled = controller.is_some_and(|id| controller_lease.renew(id));
                        let result = if after.is_none() || snapshot.resync_required {
                            api.conversation(&handle.session_id).await.and_then(|conversation| {
                                snapshot.conversation = conversation.map(serde_json::to_value).transpose()?;
                                Ok(snapshot)
                            })
                        } else { Ok(snapshot) };
                        let _ = reply.send(result);
                    }
                    Some(Control::Lease { controller, release, reply }) => {
                        let result = if release { controller_lease.release(controller) } else { controller_lease.claim(controller) };
                        let _ = reply.send(result);
                    }
                    Some(Control::Respond { controller, response, reply }) => {
                        let result = controller_lease.require(controller).and_then(|()| handle.broker.respond(response));
                        let _ = reply.send(result);
                    }
                    Some(Control::Shutdown) | None => {
                        shutdown = true;
                        queue.clear();
                        if let Some((turn, cancel, _)) = &active {
                            handle.broker.cancel_turn(*turn);
                            let _ = cancel.send(true);
                        }
                    }
                }
            }
        }
    }
}

async fn run_turn<A: API>(
    api: Arc<A>,
    turn: Turn,
    context: InteractionContext,
    journal: Arc<Mutex<Journal>>,
    output: Option<std::sync::mpsc::SyncSender<String>>,
) {
    let cancellation = context.cancel.clone();
    let broker = context.broker.clone();
    let mut buffer = String::new();
    let result = INTERACTION_CONTEXT
        .scope(context, async {
            let mut stream = api.chat(turn.request).await?;
            while let Some(response) = stream.next().await {
                let response = response?;
                if turn.observer.is_none() {
                    super::display::response(&output, &response, &mut buffer);
                }
                journal.lock().unwrap().publish(
                    Some(turn.id),
                    Payload::Chat { event: ChatEvent::from(&response) },
                );
                if let Some(observer) = &turn.observer {
                    // The receiver's drop guard requests cooperative cancellation;
                    // keep draining until orchestration has saved its state.
                    let _ = observer.send(Ok(response));
                } else if let ChatResponse::ToolCallStart { notifier, .. } = response {
                    // Remote transcript publication is the render acknowledgement only.
                    // UserInfra still enforces the original policy before tool execution.
                    notifier.notify_one();
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
    if let Some(output) = &output {
        super::display::flush(output, &mut buffer);
    }
    broker.cancel_turn(turn.id);
    let cancelled = *cancellation.borrow();
    let error = result.as_ref().err().map(|error| format!("{error:#}"));
    let status = if cancelled {
        "cancelled"
    } else if result.is_ok() {
        "completed"
    } else {
        "failed"
    };
    journal.lock().unwrap().publish(
        Some(turn.id),
        Payload::TurnFinished { status: status.into(), error },
    );
    if let (Some(observer), Err(error)) = (turn.observer, result) {
        let _ = observer.send(Err(error));
    }
}

fn fingerprint(request: &ChatRequest) -> serde_json::Result<String> {
    let mut semantic = request.clone();
    semantic.event.id.clear();
    semantic.event.timestamp.clear();
    serde_json::to_string(&semantic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{ConversationId, Event};
    use pretty_assertions::assert_eq;

    #[test]
    fn command_identity_ignores_generated_event_metadata_but_not_content() {
        let session = ConversationId::generate();
        let first = ChatRequest::new(Event::new("same prompt"), session);
        let retry = ChatRequest::new(Event::new("same prompt"), session);
        assert_eq!(fingerprint(&first).unwrap(), fingerprint(&retry).unwrap());
        let changed = ChatRequest::new(Event::new("different prompt"), session);
        assert_ne!(fingerprint(&first).unwrap(), fingerprint(&changed).unwrap());
        let other = ChatRequest::new(Event::new("same prompt"), ConversationId::generate());
        assert_ne!(fingerprint(&first).unwrap(), fingerprint(&other).unwrap());
    }
}
