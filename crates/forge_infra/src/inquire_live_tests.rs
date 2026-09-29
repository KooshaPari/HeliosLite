use crate::inquire::ForgeInquire;
use forge_app::UserInfra;
use forge_domain::{
    ConversationId, INTERACTION_CONTEXT, InteractionAnswer, InteractionBroker, InteractionContext,
    InteractionResponse,
};
use futures::StreamExt;
use pretty_assertions::assert_eq;
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

#[tokio::test]
async fn production_permission_rejects_stale_invalid_and_duplicate_responses() {
    use forge_domain::{InteractionKind, PermissionOperation, PolicyPermission};
    let session = ConversationId::generate();
    let runtime = uuid::Uuid::new_v4();
    let turn = uuid::Uuid::new_v4();
    let broker = InteractionBroker::new(session, runtime, Duration::from_secs(5));
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let tool = forge_domain::ToolCallFull::new("shell")
        .call_id(forge_domain::ToolCallId::new("synthetic-tool"));
    let context = InteractionContext {
        broker: broker.clone(),
        turn_id: turn,
        cancel: cancellation,
        terminal: false,
        tool_call: Some(tool.clone()),
    };
    let operation = PermissionOperation::Execute {
        command: "synthetic command".into(),
        cwd: std::env::temp_dir(),
    };
    let expected = operation.clone();
    let mut changed = broker.subscribe();
    let mut stream = INTERACTION_CONTEXT
        .scope(context, async {
            forge_app::spawn_interaction_stream(|sender| async move {
                let result = ForgeInquire::new()
                    .confirm_permission("Allow?", &operation)
                    .await;
                sender.send(result).await.unwrap();
            })
        })
        .await;
    tokio::time::timeout(Duration::from_secs(2), changed.changed())
        .await
        .unwrap()
        .unwrap();
    let request = broker.snapshot().pop().unwrap();
    assert!(
        matches!(request.kind, InteractionKind::Permission { operation } if operation == expected)
    );
    assert_eq!(request.turn_id, turn);
    assert_eq!(request.tool_call, Some(tool));
    let response = InteractionResponse {
        request_id: request.request_id,
        session_id: session,
        runtime_id: runtime,
        turn_id: turn,
        answer: InteractionAnswer::Choices(vec![1]),
    };
    let mut wrong = response.clone();
    wrong.session_id = ConversationId::generate();
    assert!(broker.respond(wrong).is_err());
    let mut wrong = response.clone();
    wrong.runtime_id = uuid::Uuid::new_v4();
    assert!(broker.respond(wrong).is_err());
    let mut wrong = response.clone();
    wrong.turn_id = uuid::Uuid::new_v4();
    assert!(broker.respond(wrong).is_err());
    let mut invalid = response.clone();
    invalid.answer = InteractionAnswer::Choices(vec![99]);
    assert!(broker.respond(invalid).is_err());
    assert_eq!(broker.snapshot().len(), 1);
    broker.respond(response.clone()).unwrap();
    assert!(broker.respond(response).is_err());
    assert_eq!(
        stream.next().await.unwrap().unwrap(),
        Some(PolicyPermission::Reject)
    );
    assert!(broker.snapshot().is_empty());
}

#[tokio::test]
async fn production_followup_cancel_and_expiry_reject_late_answers() {
    for expire in [false, true] {
        let session = ConversationId::generate();
        let runtime = uuid::Uuid::new_v4();
        let turn = uuid::Uuid::new_v4();
        let ttl = if expire {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(5)
        };
        let broker = InteractionBroker::new(session, runtime, ttl);
        let (cancel, cancellation) = tokio::sync::watch::channel(false);
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
                    sender
                        .send(ForgeInquire::new().prompt_question("Followup?").await)
                        .await
                        .unwrap();
                })
            })
            .await;
        tokio::time::timeout(Duration::from_secs(2), changed.changed())
            .await
            .unwrap()
            .unwrap();
        let request = broker.snapshot().pop().unwrap();
        if !expire {
            cancel.send(true).unwrap();
        }
        let actual = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(actual, None);
        assert!(
            broker
                .respond(InteractionResponse {
                    request_id: request.request_id,
                    session_id: session,
                    runtime_id: runtime,
                    turn_id: turn,
                    answer: InteractionAnswer::Text("late".into()),
                })
                .is_err()
        );
        assert!(broker.snapshot().is_empty());
    }
}

#[tokio::test]
async fn production_already_cancelled_turn_never_opens_a_followup() {
    let broker = InteractionBroker::new(
        ConversationId::generate(),
        uuid::Uuid::new_v4(),
        Duration::from_secs(5),
    );
    let (_cancel, cancellation) = tokio::sync::watch::channel(true);
    let context = InteractionContext {
        broker: broker.clone(),
        turn_id: uuid::Uuid::new_v4(),
        cancel: cancellation,
        terminal: false,
        tool_call: None,
    };
    let mut stream = INTERACTION_CONTEXT
        .scope(context, async {
            forge_app::spawn_interaction_stream(|sender| async move {
                sender
                    .send(ForgeInquire::new().prompt_question("After cancel?").await)
                    .await
                    .unwrap();
            })
        })
        .await;
    let actual = tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(actual, None);
    assert!(broker.snapshot().is_empty());
}
