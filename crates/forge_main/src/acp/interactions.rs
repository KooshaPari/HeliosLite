use super::wire::{self, schema};
use forge_domain::{InteractionAnswer, InteractionKind, InteractionRequest};
use serde_json::{Value, json};

pub fn request(request: &InteractionRequest, forms: bool) -> anyhow::Result<Option<Value>> {
    let metadata = json!({"io.phenotype/sessionId":request.session_id,"io.phenotype/runtimeId":request.runtime_id,"io.phenotype/turnId":request.turn_id,
        "io.phenotype/requestId":request.request_id,"io.phenotype/expiresAt":request.expires_at});
    let id = format!("forge/{}", request.request_id);
    match &request.kind {
        InteractionKind::Permission { operation } => {
            let tool_id = request
                .tool_call
                .as_ref()
                .and_then(|tool| tool.call_id.as_ref())
                .map(|id| id.as_str().to_owned())
                .unwrap_or_else(|| request.request_id.to_string());
            let params = wire::checked::<schema::RequestPermissionRequest>(json!({
                "sessionId":request.session_id,
                "toolCall":{"toolCallId":tool_id,"title":request.message,"status":"pending","rawInput":operation},
                "options":[
                    {"optionId":"0","name":"Accept","kind":"allow_once"},
                    {"optionId":"1","name":"Reject","kind":"reject_once"},
                    {"optionId":"2","name":"Accept and Remember","kind":"allow_always"}],
                "_meta":metadata
            }))?;
            Ok(Some(
                json!({"jsonrpc":"2.0","id":id,"method":"session/request_permission","params":params}),
            ))
        }
        _ if !forms => Ok(None),
        kind => {
            let choices: Vec<_> = request
                .choices
                .iter()
                .enumerate()
                .map(|(index, label)| json!({"const":index.to_string(),"title":label}))
                .collect();
            let property = match kind {
                InteractionKind::Text => json!({"type":"string","title":"Answer"}),
                InteractionKind::SingleChoice => {
                    json!({"type":"string","title":"Choice","oneOf":choices})
                }
                InteractionKind::MultipleChoice => {
                    json!({"type":"array","title":"Choices","items":{"anyOf":choices}})
                }
                _ => unreachable!(),
            };
            let params = wire::checked::<schema::CreateElicitationRequest>(json!({
                "sessionId":request.session_id,"mode":"form","message":request.message,
                "requestedSchema":{"type":"object","properties":{"answer":property},"required":["answer"]},"_meta":metadata
            }))?;
            Ok(Some(
                json!({"jsonrpc":"2.0","id":id,"method":"elicitation/create","params":params}),
            ))
        }
    }
}

pub fn answer(request: &InteractionRequest, value: Value) -> anyhow::Result<InteractionAnswer> {
    if matches!(request.kind, InteractionKind::Permission { .. }) {
        let value = wire::checked::<schema::RequestPermissionResponse>(value)?;
        return match value
            .get("outcome")
            .unwrap_or(&serde_json::Value::Null)
            .get("outcome")
            .unwrap_or(&serde_json::Value::Null)
            .as_str()
        {
            Some("cancelled") => Ok(InteractionAnswer::Cancel),
            Some("selected") => {
                let id = value
                    .get("outcome")
                    .unwrap_or(&serde_json::Value::Null)
                    .get("optionId")
                    .unwrap_or(&serde_json::Value::Null)
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing optionId"))?;
                anyhow::ensure!(["0", "1", "2"].contains(&id), "unoffered optionId");
                Ok(InteractionAnswer::Choices(vec![id.parse()?]))
            }
            _ => anyhow::bail!("invalid permission outcome"),
        };
    }
    let value = wire::checked::<schema::CreateElicitationResponse>(value)?;
    match value
        .get("action")
        .unwrap_or(&serde_json::Value::Null)
        .as_str()
    {
        Some("cancel" | "decline") => Ok(InteractionAnswer::Cancel),
        Some("accept") => {
            let value = value
                .get("content")
                .unwrap_or(&serde_json::Value::Null)
                .get("answer")
                .unwrap_or(&serde_json::Value::Null);
            let answer = match request.kind {
                InteractionKind::Text => InteractionAnswer::Text(
                    value
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("text required"))?
                        .to_owned(),
                ),
                InteractionKind::SingleChoice => InteractionAnswer::Choices(vec![
                    value
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("option required"))?
                        .parse()?,
                ]),
                InteractionKind::MultipleChoice => InteractionAnswer::Choices(
                    value
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("options required"))?
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .ok_or_else(|| anyhow::anyhow!("option required"))?
                                .parse::<usize>()
                                .map_err(Into::into)
                        })
                        .collect::<anyhow::Result<_>>()?,
                ),
                _ => anyhow::bail!("invalid interaction"),
            };
            anyhow::ensure!(request.accepts(&answer), "invalid answer");
            Ok(answer)
        }
        _ => anyhow::bail!("invalid elicitation action"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::ConversationId;
    use uuid::Uuid;

    fn fixture(kind: InteractionKind) -> InteractionRequest {
        InteractionRequest {
            request_id: Uuid::new_v4(),
            session_id: ConversationId::generate(),
            runtime_id: Uuid::new_v4(),
            turn_id: Uuid::new_v4(),
            expires_at: chrono::Utc::now(),
            message: "Choose a strategy".into(),
            choices: vec!["one".into(), "two".into()],
            kind,
            tool_call: None,
        }
    }

    #[test]
    fn forms_require_explicit_negotiation_and_preserve_schema() {
        for kind in [
            InteractionKind::Text,
            InteractionKind::SingleChoice,
            InteractionKind::MultipleChoice,
        ] {
            let fixture = fixture(kind);
            assert!(request(&fixture, false).unwrap().is_none());
            let request = request(&fixture, true).unwrap().unwrap();
            assert_eq!(request["method"], "elicitation/create");
            assert_eq!(
                request["params"]["_meta"]["io.phenotype/sessionId"],
                fixture.session_id.to_string()
            );
            assert_eq!(
                request["params"]["_meta"]["io.phenotype/requestId"],
                fixture.request_id.to_string()
            );
            assert!(request["params"]["requestedSchema"]["properties"]["answer"].is_object());
        }
    }

    #[test]
    fn unoffered_options_and_wrong_form_types_are_rejected() {
        let fixture = fixture(InteractionKind::SingleChoice);
        assert!(
            answer(
                &fixture,
                json!({"action":"accept","content":{"answer":"99"}})
            )
            .is_err()
        );
        assert!(
            answer(
                &fixture,
                json!({"action":"accept","content":{"answer":true}})
            )
            .is_err()
        );
        assert!(matches!(
            answer(&fixture, json!({"action":"decline"})).unwrap(),
            InteractionAnswer::Cancel
        ));
    }

    #[test]
    fn permission_options_map_exactly_to_original_policy() {
        let fixture = fixture(InteractionKind::Permission {
            operation: forge_domain::PermissionOperation::Execute {
                command: "echo example".into(),
                cwd: "/tmp".into(),
            },
        });
        let request = request(&fixture, false).unwrap().unwrap();
        assert_eq!(request["params"]["options"][2]["kind"], "allow_always");
        assert!(
            matches!(answer(&fixture, json!({"outcome":{"outcome":"selected","optionId":"2"}})).unwrap(),
            InteractionAnswer::Choices(indices) if indices == vec![2])
        );
        assert!(
            answer(
                &fixture,
                json!({"outcome":{"outcome":"selected","optionId":"Accept"}})
            )
            .is_err()
        );
    }
}
