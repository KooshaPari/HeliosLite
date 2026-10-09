use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use forge_domain::{InteractionAnswer, InteractionContext, InteractionKind, InteractionResponse};
use forge_select::{PromptAnswer, prompt_cancellable};

struct CancelInput(Arc<AtomicBool>);
impl Drop for CancelInput {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Both terminal and remote respondents settle the same broker operation.
pub(crate) async fn ask(
    context: InteractionContext,
    kind: InteractionKind,
    message: &str,
    choices: Vec<String>,
) -> InteractionAnswer {
    let mut cancellation = context.cancel.clone();
    if *cancellation.borrow() {
        return InteractionAnswer::Cancel;
    }
    let multiple = matches!(kind, InteractionKind::MultipleChoice);
    let pending = context
        .broker
        .open(context.turn_id, kind, message.to_owned(), choices);
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel_input = CancelInput(cancelled.clone());
    let terminal = if context.terminal
        && std::io::stdin().is_terminal()
        && std::io::stderr().is_terminal()
    {
        let request = pending.request.clone();
        let broker = context.broker.clone();
        Some(tokio::task::spawn_blocking(move || {
            let answer =
                match prompt_cancellable(&request.message, &request.choices, multiple, cancelled) {
                    Ok(Some(PromptAnswer::Text(text))) => InteractionAnswer::Text(text),
                    Ok(Some(PromptAnswer::Indices(indices))) => InteractionAnswer::Choices(indices),
                    Ok(None) | Err(_) => InteractionAnswer::Cancel,
                };
            let _ = broker.respond(InteractionResponse {
                request_id: request.request_id,
                session_id: request.session_id,
                runtime_id: request.runtime_id,
                turn_id: request.turn_id,
                answer,
            });
        }))
    } else {
        None
    };
    let answer = tokio::select! {
        answer = pending.wait() => answer,
        _ = async {
            loop {
                if *cancellation.borrow_and_update() {
                    break;
                }
                if cancellation.changed().await.is_err() {
                    break;
                }
            }
        } => InteractionAnswer::Cancel,
    };
    drop(cancel_input);
    if let Some(terminal) = terminal {
        let _ = terminal.await;
    }
    answer
}
