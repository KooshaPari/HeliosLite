use forge_stream::MpscStream;
use std::future::Future;

/// Spawns the production chat producer with the live turn context preserved.
/// Tokio does not inherit task-local state when spawning a task.
pub fn spawn_interaction_stream<T, F, S>(producer: F) -> MpscStream<T>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::mpsc::Sender<T>) -> S + Send + 'static,
    S: Future<Output = ()> + Send + 'static,
{
    let context = forge_domain::InteractionContext::current();
    MpscStream::spawn(move |sender| async move {
        if let Some(context) = context {
            forge_domain::INTERACTION_CONTEXT
                .scope(context, producer(sender))
                .await;
        } else {
            producer(sender).await;
        }
    })
}
