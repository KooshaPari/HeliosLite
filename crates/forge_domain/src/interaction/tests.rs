use super::*;
use crate::ConversationId;
use pretty_assertions::assert_eq;
use std::time::Duration;
use uuid::Uuid;

fn fixture() -> (std::sync::Arc<InteractionBroker>, PendingInteraction) {
    let broker = InteractionBroker::new(
        ConversationId::generate(),
        Uuid::new_v4(),
        Duration::from_secs(300),
    );
    let pending = broker.open(
        Uuid::new_v4(),
        InteractionKind::SingleChoice,
        "choose".into(),
        vec!["one".into(), "two".into()],
    );
    (broker, pending)
}
fn response(request: &InteractionRequest) -> InteractionResponse {
    InteractionResponse {
        request_id: request.request_id,
        session_id: request.session_id,
        runtime_id: request.runtime_id,
        turn_id: request.turn_id,
        answer: InteractionAnswer::Choices(vec![1]),
    }
}

#[tokio::test]
async fn rejects_wrong_identity_without_consuming_request() {
    let (broker, pending) = fixture();
    let correct = response(&pending.request);
    let mut wrong = correct.clone();
    wrong.session_id = ConversationId::generate();
    assert!(broker.respond(wrong).is_err());
    let mut stale = correct.clone();
    stale.runtime_id = Uuid::new_v4();
    assert!(broker.respond(stale).is_err());
    let mut wrong_turn = correct.clone();
    wrong_turn.turn_id = Uuid::new_v4();
    assert!(broker.respond(wrong_turn).is_err());
    assert_eq!(broker.snapshot().len(), 1);
    broker.respond(correct.clone()).unwrap();
    assert!(broker.respond(correct).is_err());
    assert!(
        matches!(pending.wait().await, InteractionAnswer::Choices(indices) if indices == vec![1])
    );
}

#[tokio::test]
async fn rejects_invalid_answers_then_allows_original_request() {
    let (broker, pending) = fixture();
    for answer in [
        InteractionAnswer::Text("Accept".into()),
        InteractionAnswer::Choices(vec![2]),
        InteractionAnswer::Choices(vec![0, 0]),
        InteractionAnswer::Choices(vec![]),
    ] {
        let mut invalid = response(&pending.request);
        invalid.answer = answer;
        assert!(broker.respond(invalid).is_err());
    }
    assert_eq!(broker.snapshot().len(), 1);
    broker.respond(response(&pending.request)).unwrap();
    assert!(matches!(
        pending.wait().await,
        InteractionAnswer::Choices(_)
    ));
}

#[tokio::test]
async fn cancel_only_target_turn_and_reconnect_retains_other_pending() {
    let (broker, pending) = fixture();
    let second = broker.open(Uuid::new_v4(), InteractionKind::Text, "text".into(), vec![]);
    let late = response(&pending.request);
    broker.cancel_turn(pending.request.turn_id);
    assert!(matches!(pending.wait().await, InteractionAnswer::Cancel));
    assert!(broker.respond(late).is_err());
    let snapshot = broker.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].request_id, second.request.request_id);
    drop(second);
    assert!(broker.snapshot().is_empty());
}

#[tokio::test]
async fn timeout_and_expired_responses_fail_closed() {
    let broker = InteractionBroker::new(ConversationId::generate(), Uuid::new_v4(), Duration::ZERO);
    let pending = broker.open(Uuid::new_v4(), InteractionKind::Text, "text".into(), vec![]);
    let mut late = response(&pending.request);
    late.answer = InteractionAnswer::Text("late".into());
    assert!(broker.respond(late).is_err());
    assert!(matches!(pending.wait().await, InteractionAnswer::Cancel));
    let pending = broker.open(Uuid::new_v4(), InteractionKind::Text, "text".into(), vec![]);
    assert!(matches!(pending.wait().await, InteractionAnswer::Cancel));
    assert!(broker.snapshot().is_empty());
}
