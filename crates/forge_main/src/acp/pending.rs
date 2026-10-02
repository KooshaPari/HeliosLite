use forge_domain::ConversationId;
use serde_json::Value;
use std::collections::HashMap;
use uuid::Uuid;

/// Settle missing receipts even if eviction happened after retry acceptance.
pub(super) fn take_missing(
    prompts: &mut HashMap<Uuid, (ConversationId, Value)>,
    session: ConversationId,
    active: Option<Uuid>,
    queued: &[Uuid],
    finished: &HashMap<Uuid, String>,
) -> Vec<Value> {
    let missing: Vec<_> = prompts
        .iter()
        .filter(|(turn, (owner, _))| {
            *owner == session
                && active.as_ref() != Some(*turn)
                && !queued.contains(*turn)
                && !finished.contains_key(*turn)
        })
        .map(|(turn, _)| *turn)
        .collect();
    missing
        .into_iter()
        .filter_map(|turn| prompts.remove(&turn).map(|(_, id)| id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_receipts_expire_only_matching_untracked_prompts() {
        let session = ConversationId::generate();
        let other = ConversationId::generate();
        let active = Uuid::new_v4();
        let queued = Uuid::new_v4();
        let done = Uuid::new_v4();
        let expired = Uuid::new_v4();
        let unrelated = Uuid::new_v4();
        let mut prompts = HashMap::from([
            (active, (session, json!(1))),
            (queued, (session, json!(2))),
            (done, (session, json!(3))),
            (expired, (session, json!(4))),
            (unrelated, (other, json!(5))),
        ]);
        assert_eq!(
            take_missing(
                &mut prompts,
                session,
                Some(active),
                &[queued],
                &HashMap::from([(done, "completed".into())])
            ),
            vec![json!(4)]
        );
        assert_eq!(prompts.len(), 4);
        assert!(!prompts.contains_key(&expired));
        assert!(prompts.contains_key(&unrelated));
        let lost = take_missing(&mut prompts, session, None, &[], &HashMap::new());
        assert_eq!(lost.len(), 3);
        assert_eq!(prompts.len(), 1);
    }
}
