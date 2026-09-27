//! Owner-only live actor control. IPC is internal, independently versioned from ACP.
mod actor;
pub(crate) mod lease;
pub(crate) mod protocol;
#[cfg(unix)]
pub(crate) mod server;

pub use actor::{Handle, TurnReceiver};
use forge_api::API;
use forge_domain::ConversationId;
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
        // The owned task receives shutdown and persists the cancelled turn before
        // releasing its lease. It is not aborted in the middle of a store write.
        let _ = &self.task;
    }
}
