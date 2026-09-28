use crate::inquire::ForgeInquire;
use forge_app::UserInfra;
use forge_domain::{
    ConversationId, INTERACTION_CONTEXT, InteractionAnswer, InteractionBroker, InteractionContext,
    InteractionResponse,
};
use futures::StreamExt;
use std::time::Duration;

#[tokio::test]
async fn production_spawn_reaches_real_user_infra_and_exact_response() {
    let session = ConversationId::generate();
    let runtime = uuid::Uuid::new_v4();
    let turn = uuid::Uuid::new_v4();
    let broker = InteractionBroker::new(session, runtime, Duration::from_secs(5));
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let context = InteractionContext {
        broker: broker.clone(),
        turn_id: turn,
        cancel: cancellation,
        terminal: false,
        tool_call: None,
    };
    let mut changed = broker.subscribe();
    let mut stream = INTERACTION_CONTEXT
        .scope(context, async {
            forge_app::spawn_interaction_stream(|sender| async move {
                let result = ForgeInquire::new().prompt_question("What branch?").await;
                sender.send(result).await.unwrap();
            })
        })
        .await;
    tokio::time::timeout(Duration::from_secs(2), changed.changed())
        .await
        .unwrap()
        .unwrap();
    let request = broker
        .snapshot()
        .pop()
        .expect("actual UserInfra call must remain pending");
    assert_eq!(request.session_id, session);
    assert_eq!(request.runtime_id, runtime);
    assert_eq!(request.turn_id, turn);
    broker
        .respond(InteractionResponse {
            request_id: request.request_id,
            session_id: session,
            runtime_id: runtime,
            turn_id: turn,
            answer: InteractionAnswer::Text("feature/test".into()),
        })
        .unwrap();
    assert_eq!(
        stream.next().await.unwrap().unwrap().as_deref(),
        Some("feature/test")
    );
    assert!(broker.snapshot().is_empty());
}
