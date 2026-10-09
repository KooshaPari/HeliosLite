//! Owner-only live actor control. IPC is internal, independently versioned from ACP.
mod actor;
mod controller;
mod display;
mod handle;
pub(crate) mod lease;
pub(crate) mod protocol;
#[cfg(unix)]
pub(crate) mod server;
pub(crate) mod terminal;

use forge_api::API;
use forge_domain::ConversationId;
pub use handle::{Handle, TurnReceiver};
use std::sync::Arc;

pub struct LiveControl {
    pub handle: Handle,
    task: tokio::task::JoinHandle<()>,
    #[cfg(unix)]
    listener: tokio::task::JoinHandle<()>,
}

impl LiveControl {
    pub fn start<A: API + 'static>(api: Arc<A>, session: ConversationId) -> anyhow::Result<Self> {
        let directory = lease::runtime_dir()?;
        let lease = lease::acquire(&directory, session)?;
        Self::with_lease(api, session, directory, lease)
    }

    /// Cancels active work and waits for its final persistence before releasing ownership.
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        #[cfg(unix)]
        self.listener.abort();
        self.handle.shutdown();
        (&mut self.task).await?;
        Ok(())
    }

    pub(crate) fn with_lease<A: API + 'static>(
        api: Arc<A>,
        session: ConversationId,
        directory: std::path::PathBuf,
        lease: std::fs::File,
    ) -> anyhow::Result<Self> {
        #[cfg(not(unix))]
        let _ = directory;
        #[cfg(unix)]
        let listener = {
            use std::os::unix::fs::PermissionsExt;
            let path = lease::socket_path(&directory, session);
            // Holding the lease proves any old socket has no owning actor.
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            let listener = tokio::net::UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            listener
        };
        let (handle, task) = Handle::spawn(api, session, lease);
        #[cfg(unix)]
        let listener = server::serve(listener, handle.clone());
        Ok(Self {
            handle,
            task,
            #[cfg(unix)]
            listener,
        })
    }
}

impl Drop for LiveControl {
    fn drop(&mut self) {
        #[cfg(unix)]
        self.listener.abort();
        self.handle.shutdown();
        // Explicit shutdown is awaited at the host and UI entrypoints. Drop is
        // best effort for exceptional paths while the executor is still alive.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use uuid::Uuid;

    #[tokio::test]
    async fn shutdown_waits_for_owner_completion() {
        let session = ConversationId::generate();
        let runtime = Uuid::new_v4();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let persisted = Arc::new(AtomicBool::new(false));
        let completion = persisted.clone();
        let task = tokio::spawn(async move {
            assert!(matches!(rx.recv().await, Some(actor::Control::Shutdown)));
            tokio::task::yield_now().await;
            completion.store(true, Ordering::Release);
        });
        let owner = LiveControl {
            handle: Handle {
                session_id: session,
                runtime_id: runtime,
                broker: forge_domain::InteractionBroker::new(
                    session,
                    runtime,
                    Duration::from_secs(5),
                ),
                tx,
                output: Arc::new(Mutex::new(None)),
            },
            task,
            #[cfg(unix)]
            listener: tokio::spawn(std::future::pending()),
        };
        owner.shutdown().await.unwrap();
        assert!(persisted.load(Ordering::Acquire));
    }
}
