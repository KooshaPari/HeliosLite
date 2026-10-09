use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

pub use agent_client_protocol_schema::v1 as schema;

/// Decode and re-encode with official v1 types at every protocol boundary.
pub fn checked<T: Serialize + DeserializeOwned>(value: Value) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(serde_json::from_value::<T>(value)?)?)
}
pub async fn send(value: Value) -> anyhow::Result<()> {
    let mut line = serde_json::to_vec(&value)?;
    line.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout.write_all(&line).await?;
    stdout.flush().await?;
    Ok(())
}
pub async fn result(id: Value, value: Value) -> anyhow::Result<()> {
    send(json!({"jsonrpc":"2.0", "id":id, "result":value})).await
}
pub async fn error(id: Value, code: i64, message: impl ToString) -> anyhow::Result<()> {
    send(json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message.to_string()}}))
        .await
}
pub async fn update(session: forge_domain::ConversationId, update: Value) -> anyhow::Result<()> {
    let params =
        checked::<schema::SessionNotification>(json!({"sessionId":session,"update":update}))?;
    send(json!({"jsonrpc":"2.0", "method":"session/update", "params":params})).await
}
