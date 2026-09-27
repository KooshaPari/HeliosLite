//! Correlation and exactly-once settlement for live user interactions.
mod broker;
mod types;

pub use broker::*;
pub use types::*;

/// Context propagated explicitly into each owned orchestration task.
#[derive(Clone)]
pub struct InteractionContext {
    pub broker: std::sync::Arc<InteractionBroker>,
    pub turn_id: uuid::Uuid,
    pub cancel: tokio::sync::watch::Receiver<bool>,
    pub terminal: bool,
}

tokio::task_local! {
    /// Active turn context; unrelated sessions and tasks never share requests.
    pub static INTERACTION_CONTEXT: InteractionContext;
}

impl InteractionContext {
    /// Returns the active task's context when controlled by a live actor.
    pub fn current() -> Option<Self> {
        INTERACTION_CONTEXT.try_with(Clone::clone).ok()
    }
}

#[cfg(test)]
mod tests;
