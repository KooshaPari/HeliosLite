use crate::live_control::{
    LiveControl, lease,
    protocol::{Command, Request, Snapshot, VERSION},
    server,
};
use forge_api::{API, ForgeAPI};
use forge_domain::{Conversation, ConversationId};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

pub async fn snapshot(
    session: ConversationId,
    runtime: Option<Uuid>,
    after: Option<u64>,
) -> anyhow::Result<Snapshot> {
    let request = Request {
        version: VERSION,
        session_id: session,
        runtime_id: runtime,
        command: Command::Snapshot { after },
    };
    Ok(serde_json::from_value(call(&request).await?)?)
}

pub async fn call(request: &Request) -> anyhow::Result<serde_json::Value> {
    let path = lease::socket_path(&lease::runtime_dir()?, request.session_id);
    tokio::time::timeout(Duration::from_secs(5), server::call(path, request)).await?
}

pub async fn attach(
    session: ConversationId,
    cwd: &Path,
    create: bool,
) -> anyhow::Result<(Snapshot, &'static str)> {
    if let Ok(snapshot) = snapshot(session, None, None).await {
        anyhow::ensure!(!create, "new_session_identity_already_exists");
        return Ok((snapshot, "attached"));
    }
    // Never remove a socket or take ownership here. The child must win the OS
    // lease before loading or creating a persisted conversation.
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("--directory")
        .arg(cwd)
        .arg("live-host")
        .arg(session.to_string());
    if create {
        command.arg("--create");
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    use std::os::unix::process::CommandExt;
    // SAFETY: only async-signal-safe setsid is invoked after fork.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    for _ in 0..50 {
        if let Ok(snapshot) = snapshot(session, None, None).await {
            // Reap the launcher asynchronously without tying runtime lifetime to ACP.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok((snapshot, if create { "created" } else { "resumed" }));
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("runtime startup failed: {status}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "runtime startup pending; retry session/load without creating another conversation"
    )
}

pub async fn host(session: ConversationId, cwd: PathBuf, create: bool) -> anyhow::Result<()> {
    let directory = lease::runtime_dir()?;
    let ownership = lease::acquire(&directory, session)?;
    let config = forge_config::ForgeConfig::read()?;
    let api = Arc::new(ForgeAPI::init(cwd.clone(), config));
    let persisted = api.conversation(&session).await?;
    if create {
        anyhow::ensure!(persisted.is_none(), "conversation already exists");
        let mut conversation = Conversation::new(session);
        conversation.cwd = Some(cwd.to_string_lossy().into_owned());
        conversation.source = Some("acp".into());
        api.upsert_conversation(conversation).await?;
    } else {
        let persisted = persisted.ok_or_else(|| anyhow::anyhow!("conversation not found"))?;
        if let Some(original) = persisted.cwd {
            anyhow::ensure!(
                Path::new(&original).canonicalize()? == cwd.canonicalize()?,
                "workspace_mismatch"
            );
        }
    }
    let _runtime = LiveControl::with_lease(api, session, directory, ownership)?;
    tokio::signal::ctrl_c().await?;
    Ok(())
}
