use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;

use super::Handle;
use super::protocol::{Command, Request, VERSION};

const MAX_FRAME: usize = 1024 * 1024;

pub fn serve(listener: UnixListener, handle: Handle) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut clients = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept(), if clients.len() < 32 => {
                    let Ok((stream, _)) = accepted else { break };
                    let Ok(peer) = stream.peer_cred() else { continue };
                    // SAFETY: geteuid has no preconditions.
                    if peer.uid() != unsafe { libc::geteuid() } { continue; }
                    let handle = handle.clone();
                    clients.spawn(async move { let _ = connection(stream, handle).await; });
                }
                _ = clients.join_next(), if !clients.is_empty() => {}
            }
        }
    })
}

pub(crate) async fn read_frame<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            anyhow::ensure!(frame.is_empty(), "truncated_frame");
            return Ok(None);
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let count = end.map_or(available.len(), |index| index + 1);
        anyhow::ensure!(frame.len() + count <= MAX_FRAME, "frame_too_large");
        frame.extend(available.iter().take(count));
        reader.consume(count);
        if end.is_some() {
            return Ok(Some(frame));
        }
    }
}

async fn connection(stream: UnixStream, handle: Handle) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    while let Some(frame) = read_frame(&mut reader).await? {
        let response = match serde_json::from_slice::<Request>(&frame) {
            Ok(request) => dispatch(&handle, request).await,
            Err(error) => Err(error.into()),
        };
        let response = match response {
            Ok(value) => serde_json::json!({ "version": VERSION, "result": value }),
            Err(error) => serde_json::json!({ "version": VERSION, "error": error.to_string() }),
        };
        let mut encoded = serde_json::to_vec(&response)?;
        encoded.push(b'\n');
        writer.write_all(&encoded).await?;
    }
    Ok(())
}

async fn dispatch(handle: &Handle, request: Request) -> anyhow::Result<serde_json::Value> {
    anyhow::ensure!(request.version == VERSION, "unsupported_version");
    anyhow::ensure!(request.session_id == handle.session_id, "session_mismatch");
    if !matches!(&request.command, Command::Snapshot { .. }) {
        anyhow::ensure!(
            request.runtime_id == Some(handle.runtime_id),
            "runtime_mismatch"
        );
    } else if let Some(runtime) = request.runtime_id {
        anyhow::ensure!(runtime == handle.runtime_id, "runtime_mismatch");
    }
    let controller = || {
        request
            .controller_id
            .ok_or_else(|| anyhow::anyhow!("controller_required"))
    };
    match request.command {
        Command::ClaimControl => {
            handle.control(controller()?, false).await?;
            Ok(serde_json::json!({ "controlled": true }))
        }
        Command::ReleaseControl => {
            handle.control(controller()?, true).await?;
            Ok(serde_json::json!({ "controlled": false }))
        }
        Command::Snapshot { after } => Ok(serde_json::to_value(
            handle.snapshot(after, request.controller_id).await?,
        )?),
        Command::Prompt { command_id, event } => {
            let turn = handle
                .prompt(
                    controller()?,
                    command_id,
                    forge_domain::ChatRequest::new(event, handle.session_id),
                )
                .await?;
            Ok(serde_json::json!({ "turn_id": turn, "runtime_id": handle.runtime_id }))
        }
        Command::Cancel { turn_id } => {
            handle.cancel(controller()?, turn_id).await?;
            Ok(serde_json::json!({}))
        }
        Command::Respond { response } => {
            handle.respond(controller()?, response).await?;
            Ok(serde_json::json!({}))
        }
    }
}

/// One request per connection makes cancellation independent of a pending prompt.
pub async fn call(path: PathBuf, request: &Request) -> anyhow::Result<serde_json::Value> {
    let stream = UnixStream::connect(path).await?;
    // SAFETY: geteuid has no preconditions.
    anyhow::ensure!(
        stream.peer_cred()?.uid() == unsafe { libc::geteuid() },
        "runtime_owner_mismatch"
    );
    let (reader, mut writer) = stream.into_split();
    let mut encoded = serde_json::to_vec(request)?;
    encoded.push(b'\n');
    anyhow::ensure!(encoded.len() <= MAX_FRAME, "frame_too_large");
    writer.write_all(&encoded).await?;
    let response = read_frame(&mut BufReader::new(reader))
        .await?
        .ok_or_else(|| anyhow::anyhow!("runtime_disconnected"))?;
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    anyhow::ensure!(response["version"] == VERSION, "unsupported_version");
    if let Some(error) = response.get("error") {
        anyhow::bail!("{error}");
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing_result"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn framing_rejects_truncation_and_oversize_without_losing_next_frame() {
        let mut reader = BufReader::new(&b"one\ntwo\n"[..]);
        assert_eq!(
            read_frame(&mut reader).await.unwrap(),
            Some(b"one\n".to_vec())
        );
        assert_eq!(
            read_frame(&mut reader).await.unwrap(),
            Some(b"two\n".to_vec())
        );
        assert_eq!(read_frame(&mut reader).await.unwrap(), None);
        assert!(
            read_frame(&mut BufReader::new(&b"truncated"[..]))
                .await
                .is_err()
        );
        let oversized = vec![b'x'; MAX_FRAME + 1];
        assert!(
            read_frame(&mut BufReader::new(oversized.as_slice()))
                .await
                .is_err()
        );
    }
}
