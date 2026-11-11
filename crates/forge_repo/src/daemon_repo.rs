use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use forge_dbd::client::{DbClient, DbClientSendError};
use forge_dbd::protocol::{ConversationMutation, Request, Response};
use forge_domain::{
    Conversation, ConversationId, ConversationRepository, ConversationSummary, ForgeExportOptions,
    ForgeExportReport, ForgeForgetOptions, ForgeForgetReport, ForgeImportOptions,
    ForgeImportReport, ForgeMigrateReport, HeliosdoctorDbStats, MigrateOptions,
};
use tracing::{debug, info, warn};

use crate::conversation::ConversationRepositoryImpl;

/// Set once per process: the first routed write that fails to connect
/// attempts to spawn the daemon. A later connect failure in the same process
/// never respawns — either the daemon came up (and subsequent connects
/// succeed) or it did not, and re-spawning would just repeat the wait.
static SPAWN_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// Decorator that routes the hot-path conversation WRITES through the
/// `forge-dbd` single-writer daemon while keeping reads on the direct diesel
/// path (P3 design: writes serialise in the daemon, reads stay latency
/// sensitive and local).
///
/// Only four methods are routed to the daemon — `upsert_conversation`,
/// `upsert_conversation_ref`, `update_parent_id`, `delete_conversation` — the
/// exact set the daemon protocol implements. Every other
/// [`ConversationRepository`] method is a plain pass-through to `inner`.
///
/// ## Sync-over-async mechanics
///
/// The original plan called for a lazily-built current-thread tokio Runtime
/// with `block_on`, on the assumption that the trait methods were SYNC. That
/// assumption does not hold: `ConversationRepository` is an `async_trait`, so
/// these methods are async and are invoked from tokio worker contexts
/// (`ForgeConversationService` and friends await the trait directly). The
/// daemon round-trip is therefore a plain `.await` on `DbClient::send` — no
/// dedicated Runtime, no `Mutex`, no `block_on`-reentrancy hazard, and no
/// `Handle::current()` risk. We deliberately do NOT spawn a separate runtime
/// or wrap the trait in sync form, which would only add complexity.
///
/// ## Client lifecycle
///
/// `DbClient` is created lazily on the first routed write and cached in a
/// clearable `tokio::sync::Mutex<Option<DbClient>>`. A failed fresh request
/// connection evicts the stale probe client, so a later write can recover.
/// `DbClient::send_classified` opens a fresh connection per request anyway
/// (client.rs), so caching only skips the connect probe.
///
/// ## Daemon spawn
///
/// When the first routed write cannot connect, the client spawns the daemon
/// (binary from `FORGE_DBD_BIN` or `forge_dbd` on PATH) exactly once per
/// process, then polls the socket for ~2 s before falling back to the direct
/// path. A later connect failure in the same process skips the spawn and
/// falls back immediately — the daemon either came up or it did not.
///
/// ## Fallback semantics
///
/// A direct fallback is safe only before a daemon request can have been sent:
/// failed initial connect or failed daemon spawn. Once
/// [`DbClient::send_classified`] starts its fresh request stream, a transport
/// error, daemon error response, or unexpected response is indeterminate: the
/// daemon may have committed the write. Those outcomes are surfaced to the
/// caller and are never replayed through `inner`.
///
/// `FORGE_DBD_STRICT_PROTOCOL=1` requires a same-stream strict daemon
/// advertisement for these four methods and refuses their direct fallback,
/// including new/unowned rows and unsupported database paths. It does not
/// constrain other repository methods, older clients, or independent writers.
/// The strict daemon must already be running; strict clients never spawn it.
pub struct DaemonConversationRepository {
    inner: Arc<ConversationRepositoryImpl>,
    strict_daemon_protocol: bool,
    strict_daemon_config_invalid: bool,
    socket_path: PathBuf,
    client: tokio::sync::Mutex<Option<Arc<DbClient>>>,
    /// Serializes recovery from an unavailable daemon. A second write must
    /// wait for the first caller's spawn/poll before deciding that direct
    /// fallback is safe.
    lifecycle_lock: tokio::sync::Mutex<()>,
    /// Daemon binary to spawn on the first routed write, resolved from
    /// `FORGE_DBD_BIN` at construction (see
    /// [`forge_domain::Environment::dbd_bin_path`]). `None` means "look up
    /// `forge_dbd` on PATH" at spawn time.
    dbd_bin: Option<PathBuf>,
    /// Test-only seam for exercising the post-spawn polling path without an
    /// operating-system-specific executable.
    #[cfg(test)]
    test_spawn_succeeds: bool,
    /// The client's workspace id (hash of its cwd), stamped onto daemon-side
    /// upserts. The daemon derives its own id from ITS current directory,
    /// which diverges from the client's in `--directory` mode (Windows path
    /// canonicalization). Reads filter by the client's hash, so daemon-written
    /// rows must carry it or they would be invisible to the caller.
    #[allow(dead_code)]
    workspace_id: i64,
}

/// Result of attempting a daemon-routed conversation write.
///
/// `Unavailable` means no daemon request was sent, so the direct repository
/// may safely execute the write. `Indeterminate` means the daemon had the
/// opportunity to observe the request; direct fallback would risk a duplicate
/// write and must be skipped.
enum DaemonWriteOutcome {
    Ack,
    Unavailable,
    Indeterminate(anyhow::Error),
}

impl DaemonConversationRepository {
    fn validate_strict_configuration(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.strict_daemon_config_invalid,
            "FORGE_DBD_STRICT_PROTOCOL must be unset or 1"
        );
        Ok(())
    }

    fn require_direct_allowed(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.strict_daemon_protocol,
            "strict daemon protocol requires daemon routing; direct write refused"
        );
        Ok(())
    }

    fn daemon_routing_available(&self) -> anyhow::Result<bool> {
        let path = self.inner.daemon_database_path();
        if path == std::path::Path::new(":memory:") || !path.is_absolute() {
            return Ok(false);
        }
        anyhow::ensure!(
            path.is_absolute() && path.is_file(),
            "daemon routing requires an existing absolute database path"
        );
        Ok(true)
    }

    pub fn new(
        inner: Arc<ConversationRepositoryImpl>,
        socket_path: PathBuf,
        workspace_id: i64,
    ) -> Self {
        let strict_setting = std::env::var_os("FORGE_DBD_STRICT_PROTOCOL");
        Self {
            inner,
            strict_daemon_protocol: strict_setting.is_some(),
            strict_daemon_config_invalid: strict_setting.is_some_and(|value| value != "1"),
            socket_path,
            client: tokio::sync::Mutex::new(None),
            lifecycle_lock: tokio::sync::Mutex::new(()),
            dbd_bin: default_dbd_bin(),
            #[cfg(test)]
            test_spawn_succeeds: false,
            workspace_id,
        }
    }

    /// Test-only constructor: injects the daemon binary path so tests can
    /// point at a guaranteed-missing binary without mutating process env.
    #[cfg(test)]
    fn new_with_bin(
        inner: Arc<ConversationRepositoryImpl>,
        socket_path: PathBuf,
        dbd_bin: Option<PathBuf>,
        workspace_id: i64,
    ) -> Self {
        Self {
            inner,
            strict_daemon_protocol: false,
            strict_daemon_config_invalid: false,
            socket_path,
            client: tokio::sync::Mutex::new(None),
            lifecycle_lock: tokio::sync::Mutex::new(()),
            dbd_bin,
            test_spawn_succeeds: false,
            workspace_id,
        }
    }

    /// Test-only constructor that simulates a successful daemon spawn.
    #[cfg(test)]
    fn new_with_spawn_success(
        inner: Arc<ConversationRepositoryImpl>,
        socket_path: PathBuf,
        workspace_id: i64,
    ) -> Self {
        let mut repo = Self::new_with_bin(inner, socket_path, None, workspace_id);
        repo.test_spawn_succeeds = true;
        repo
    }

    /// Attempt one daemon round-trip for a write request.
    ///
    /// Only an initial connection/spawn failure is classified as
    /// [`DaemonWriteOutcome::Unavailable`]. All failures after
    /// [`DbClient::send_classified`] begins are indeterminate because the
    /// daemon may have recorded the request before the client lost its
    /// response.
    async fn try_daemon(&self, request: Request) -> DaemonWriteOutcome {
        // Only daemon startup and probe-cache access share the lifecycle
        // lock. `send_classified` opens a fresh stream and can wait on daemon
        // I/O, so it must run after both mutex guards have been released.
        // Holding either guard across that await serializes healthy writes and
        // prevents a later caller from recovering an obsolete cached probe.
        let client = {
            let _lifecycle = self.lifecycle_lock.lock().await;
            if !self.ensure_client().await {
                debug!(
                    socket = %self.socket_path.display(),
                    "forge_dbd still unavailable; falling back to direct write"
                );
                return DaemonWriteOutcome::Unavailable;
            }

            let cached = self.client.lock().await;
            let Some(client) = cached.as_ref() else {
                return DaemonWriteOutcome::Unavailable;
            };
            client.clone()
        };
        match client
            .send_classified_with_policy(request, self.strict_daemon_protocol)
            .await
        {
            Ok(Response::Ack) => DaemonWriteOutcome::Ack,
            Ok(Response::Error { message }) => {
                warn!(
                    message = %message,
                    "forge_dbd rejected write; not replaying an indeterminate write"
                );
                DaemonWriteOutcome::Indeterminate(anyhow::anyhow!(
                    "forge_dbd rejected write after receiving the request: {message}"
                ))
            }
            Ok(other) => {
                warn!(
                    response = ?other,
                    "forge_dbd returned an unexpected response; not replaying an indeterminate write"
                );
                DaemonWriteOutcome::Indeterminate(anyhow::anyhow!(
                    "forge_dbd returned an unexpected response after receiving the request: {other:?}"
                ))
            }
            Err(DbClientSendError::Unavailable(err)) => {
                // A fresh stream could not be opened, so no request bytes
                // were sent. Drop this exact stale successful probe and
                // permit the safe direct fallback. Keep the comparison under
                // the lifecycle lock so a concurrent recovery cannot lose a
                // newer cache entry.
                let _lifecycle = self.lifecycle_lock.lock().await;
                let mut cached = self.client.lock().await;
                if cached
                    .as_ref()
                    .is_some_and(|cached| Arc::ptr_eq(cached, &client))
                {
                    *cached = None;
                }
                debug!(
                    socket = %self.socket_path.display(),
                    error = %err,
                    "forge_dbd cached probe went stale before request send; falling back to direct write"
                );
                DaemonWriteOutcome::Unavailable
            }
            Err(DbClientSendError::Indeterminate(err)) => {
                warn!(
                    error = %err,
                    "forge_dbd request transport failed; not replaying an indeterminate write"
                );
                DaemonWriteOutcome::Indeterminate(err.context(
                    "forge_dbd request outcome is indeterminate; direct fallback skipped to avoid duplicate write",
                ))
            }
        }
    }

    /// Apply the safety policy shared by every daemon-routed write.
    ///
    /// The direct fallback runs only when no daemon request was sent.
    async fn write_or_fallback<F, Fut>(&self, request: Request, fallback: F) -> anyhow::Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = anyhow::Result<()>>,
    {
        match self.try_daemon(request).await {
            DaemonWriteOutcome::Ack => Ok(()),
            DaemonWriteOutcome::Unavailable if self.strict_daemon_protocol => {
                anyhow::bail!("strict daemon protocol unavailable; direct fallback refused")
            }
            DaemonWriteOutcome::Unavailable => fallback().await,
            DaemonWriteOutcome::Indeterminate(error) => Err(error),
        }
    }

    /// Attempts to bring the daemon up and connect to it.
    ///
    /// Spawns the daemon exactly once per process (guarded by
    /// [`SPAWN_ATTEMPTED`]); a second connect failure in the same process
    /// skips the spawn and falls straight back to the direct write. After a
    /// successful spawn the socket is polled for ~2 s — the daemon needs a
    /// moment to bind (the named-pipe `ERROR_PIPE_BUSY` retry inside
    /// `DbClient::connect` also helps) — and the connected client is cached
    /// in the clearable client cache for subsequent writes. The caller holds
    /// `lifecycle_lock`, so concurrent writers cannot bypass this recovery.
    async fn ensure_client(&self) -> bool {
        if self.client.lock().await.is_some() {
            return true;
        }
        if let Ok(client) = DbClient::connect(&self.socket_path).await {
            *self.client.lock().await = Some(Arc::new(client));
            return true;
        }
        // A reachable older daemon can fail its protocol probe. Starting a
        // second daemon here could displace its socket before we can verify
        // strict policy, so strict clients require a separately started daemon.
        if self.strict_daemon_protocol {
            return false;
        }
        debug!(
            socket = %self.socket_path.display(),
            "forge_dbd unavailable; attempting to spawn daemon"
        );
        if SPAWN_ATTEMPTED.swap(true, Ordering::SeqCst) {
            return false;
        }
        if !self.spawn_daemon() {
            // Spawn failed (e.g. binary not found): no point polling a socket
            // nothing will bind. The guard is already set, so we do not retry.
            return false;
        }
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            if let Ok(client) = DbClient::connect(&self.socket_path).await {
                *self.client.lock().await = Some(Arc::new(client));
                return true;
            }
        }
        false
    }

    /// Spawns the `forge-dbd` daemon as a detached child process.
    ///
    /// Returns `false` (after logging) when the binary cannot be spawned,
    /// e.g. `FORGE_DBD_BIN` points at a missing file.
    ///
    /// The child receives the repository's resolved write path and socket path.
    fn spawn_daemon(&self) -> bool {
        #[cfg(test)]
        if self.test_spawn_succeeds {
            return true;
        }

        let bin = self
            .dbd_bin
            .clone()
            .unwrap_or_else(|| PathBuf::from("forge_dbd"));
        let mut cmd = match self.daemon_command(&bin) {
            Ok(cmd) => cmd,
            Err(error) => {
                warn!(error = %error, "cannot resolve daemon database path; using direct storage");
                return false;
            }
        };
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match cmd.spawn() {
            Ok(_) => {
                info!(
                    bin = %bin.display(),
                    "spawned forge-dbd daemon; waiting for it to accept connections"
                );
                true
            }
            Err(e) => {
                warn!(
                    bin = %bin.display(),
                    error = %e,
                    "failed to spawn forge-dbd daemon; falling back to direct write"
                );
                false
            }
        }
    }

    fn daemon_command(&self, bin: &std::path::Path) -> anyhow::Result<std::process::Command> {
        let database = resolve_daemon_database_path(self.inner.daemon_database_path())?;
        let mut cmd = std::process::Command::new(bin);
        cmd.env("FORGE_WRITE_DB_PATH", database);
        cmd.env("FORGE_DBD_SOCKET", &self.socket_path);
        Ok(cmd)
    }
}

fn resolve_daemon_database_path(configured: &std::path::Path) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        configured.is_absolute() && configured.is_file(),
        "daemon spawn requires an existing absolute database path"
    );
    Ok(configured.canonicalize()?)
}

/// Resolves the daemon binary path from the environment, mirroring
/// [`forge_domain::Environment::dbd_bin_path`]: `FORGE_DBD_BIN` if set,
/// otherwise `None` (spawn-time `forge_dbd` PATH lookup). The method ignores
/// `self` — it only reads the environment — so a throwaway
/// [`forge_domain::Environment`] suffices.
fn default_dbd_bin() -> Option<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let env = forge_domain::Environment {
        os: std::env::consts::OS.to_string(),
        cwd: std::env::current_dir().unwrap_or_else(|_| home.clone()),
        home: Some(home),
        shell: std::env::var("SHELL").unwrap_or_default(),
        base_path: PathBuf::from("."),
    };
    env.dbd_bin_path()
}

#[async_trait::async_trait]
impl ConversationRepository for DaemonConversationRepository {
    // -----------------------------------------------------------------------
    // Daemon-routed writes
    // -----------------------------------------------------------------------

    async fn upsert_conversation(&self, conversation: Conversation) -> anyhow::Result<()> {
        self.validate_strict_configuration()?;
        if !self.daemon_routing_available()? {
            self.require_direct_allowed()?;
            return self.inner.upsert_conversation(conversation).await;
        }
        if !self.inner.daemon_has_local_owned(&conversation.id).await? {
            self.require_direct_allowed()?;
            return self.inner.upsert_conversation(conversation).await;
        }
        let database_token = self.inner.daemon_database_token().await?;
        self.write_or_fallback(
            Request::MutationV3 {
                workspace_id: self.inner.workspace_id(),
                database_token,
                mutation: ConversationMutation::UpsertConversation {
                    conversation: conversation.clone(),
                    workspace_id: None,
                },
            },
            || self.inner.upsert_conversation(conversation),
        )
        .await
    }

    async fn upsert_conversation_ref(&self, conversation: &Conversation) -> anyhow::Result<()> {
        self.validate_strict_configuration()?;
        if !self.daemon_routing_available()? {
            self.require_direct_allowed()?;
            return self.inner.upsert_conversation_ref(conversation).await;
        }
        if !self.inner.daemon_has_local_owned(&conversation.id).await? {
            self.require_direct_allowed()?;
            return self.inner.upsert_conversation_ref(conversation).await;
        }
        let database_token = self.inner.daemon_database_token().await?;
        self.write_or_fallback(
            Request::MutationV3 {
                workspace_id: self.inner.workspace_id(),
                database_token,
                mutation: ConversationMutation::UpsertConversationRef {
                    conversation: conversation.clone(),
                    workspace_id: None,
                },
            },
            || self.inner.upsert_conversation_ref(conversation),
        )
        .await
    }

    async fn update_parent_id(
        &self,
        conversation_id: &ConversationId,
        new_parent_id: Option<&ConversationId>,
    ) -> anyhow::Result<()> {
        self.validate_strict_configuration()?;
        if !self.daemon_routing_available()? {
            self.require_direct_allowed()?;
            return self
                .inner
                .update_parent_id(conversation_id, new_parent_id)
                .await;
        }
        if !self.inner.daemon_has_local_owned(conversation_id).await? {
            self.require_direct_allowed()?;
            return self
                .inner
                .update_parent_id(conversation_id, new_parent_id)
                .await;
        }
        let database_token = self.inner.daemon_database_token().await?;
        self.write_or_fallback(
            Request::MutationV3 {
                workspace_id: self.inner.workspace_id(),
                database_token,
                mutation: ConversationMutation::UpdateParentId {
                    conversation_id: *conversation_id,
                    new_parent_id: new_parent_id.cloned(),
                },
            },
            || self.inner.update_parent_id(conversation_id, new_parent_id),
        )
        .await
    }

    async fn delete_conversation(&self, conversation_id: &ConversationId) -> anyhow::Result<()> {
        self.validate_strict_configuration()?;
        if !self.daemon_routing_available()? {
            self.require_direct_allowed()?;
            return self.inner.delete_conversation(conversation_id).await;
        }
        self.write_or_fallback(
            Request::DeleteWithLedger {
                workspace_id: self.inner.workspace_id(),
                conversation_id: *conversation_id,
            },
            || self.inner.delete_conversation(conversation_id),
        )
        .await
    }

    // -----------------------------------------------------------------------
    // Pass-throughs (reads + maintenance stay direct)
    // -----------------------------------------------------------------------

    async fn get_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> anyhow::Result<Option<Conversation>> {
        self.inner.get_conversation(conversation_id).await
    }

    async fn get_all_conversations(
        &self,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.inner.get_all_conversations(limit).await
    }

    async fn get_last_conversation(&self) -> anyhow::Result<Option<Conversation>> {
        self.inner.get_last_conversation().await
    }

    async fn get_conversations_by_parent(
        &self,
        parent_id: &ConversationId,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.inner.get_conversations_by_parent(parent_id).await
    }

    async fn get_parent_conversations(
        &self,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.inner.get_parent_conversations(limit).await
    }

    async fn get_parent_conversations_lite(
        &self,
        limit: Option<usize>,
        all_workspaces: bool,
    ) -> anyhow::Result<Option<Vec<ConversationSummary>>> {
        self.inner
            .get_parent_conversations_lite(limit, all_workspaces)
            .await
    }

    async fn get_conversations_by_source(
        &self,
        source: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.inner.get_conversations_by_source(source, limit).await
    }

    async fn search_conversations(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<Conversation>> {
        self.inner.search_conversations(query, limit).await
    }

    async fn get_conversation_snippet(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        token_count: usize,
    ) -> anyhow::Result<Option<String>> {
        self.inner
            .get_conversation_snippet(conversation_id, query, token_count)
            .await
    }

    async fn get_conversation_highlight(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        open_mark: &str,
        close_mark: &str,
    ) -> anyhow::Result<Option<String>> {
        self.inner
            .get_conversation_highlight(conversation_id, query, open_mark, close_mark)
            .await
    }

    async fn optimize_fts_index(&self) -> anyhow::Result<()> {
        self.inner.optimize_fts_index().await
    }

    async fn refresh_fts_index(&self) -> anyhow::Result<()> {
        self.inner.refresh_fts_index().await
    }

    async fn get_conversations_by_cwd(
        &self,
        cwd: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.inner.get_conversations_by_cwd(cwd, limit).await
    }

    async fn mark_intent_state(
        &self,
        conversation_id: &ConversationId,
        new_state: &str,
    ) -> anyhow::Result<()> {
        self.inner
            .mark_intent_state(conversation_id, new_state)
            .await
    }

    async fn list_prune_eligible(
        &self,
        workspace_id: Option<i64>,
        limit: usize,
    ) -> anyhow::Result<Vec<Conversation>> {
        self.inner.list_prune_eligible(workspace_id, limit).await
    }

    async fn prune_conversation(&self, conversation_id: &ConversationId) -> anyhow::Result<()> {
        self.inner.prune_conversation(conversation_id).await
    }

    async fn rewind_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> anyhow::Result<Option<Conversation>> {
        self.inner.rewind_conversation(conversation_id).await
    }

    async fn compress_uncompressed_contexts(&self) -> anyhow::Result<(usize, usize, usize)> {
        self.inner.compress_uncompressed_contexts().await
    }

    async fn import_forge_db(&self, source: PathBuf) -> anyhow::Result<ForgeImportReport> {
        self.inner.import_forge_db(source).await
    }

    async fn import_forge_db_with_options(
        &self,
        source: PathBuf,
        options: &ForgeImportOptions,
    ) -> anyhow::Result<ForgeImportReport> {
        self.inner
            .import_forge_db_with_options(source, options)
            .await
    }

    async fn import_forge_db_selected(
        &self,
        source: PathBuf,
        ids: &[ConversationId],
        restore_deleted: bool,
        accept_unknown: bool,
        expected_digest: Option<&str>,
        options: &ForgeImportOptions,
    ) -> anyhow::Result<ForgeImportReport> {
        self.inner
            .import_forge_db_selected(
                source,
                ids,
                restore_deleted,
                accept_unknown,
                expected_digest,
                options,
            )
            .await
    }

    async fn export_forge_db(
        &self,
        dest: PathBuf,
        options: &ForgeExportOptions,
    ) -> anyhow::Result<ForgeExportReport> {
        self.inner.export_forge_db(dest, options).await
    }

    async fn database_stats(&self) -> anyhow::Result<HeliosdoctorDbStats> {
        self.inner.database_stats().await
    }

    async fn forget_conversations(
        &self,
        options: &ForgeForgetOptions,
    ) -> anyhow::Result<ForgeForgetReport> {
        self.inner.forget_conversations(options).await
    }

    async fn migrate_data_dir(
        &self,
        options: &MigrateOptions,
    ) -> anyhow::Result<ForgeMigrateReport> {
        self.inner.migrate_data_dir(options).await
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    #[cfg(unix)]
    use forge_dbd::protocol::{HealthStatus, read_frame, write_frame};
    use forge_domain::{Conversation, ConversationId, WorkspaceHash};
    use pretty_assertions::assert_eq;
    #[cfg(unix)]
    use tokio::net::UnixListener;
    #[cfg(unix)]
    use tokio::sync::oneshot;

    use super::*;
    use crate::conversation::ConversationRepositoryImpl;
    use crate::database::DatabasePool;

    /// Serializes the decorator tests that touch the process-wide spawn guard
    /// ([`SPAWN_ATTEMPTED`]) so they cannot race each other.
    static SPAWN_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[cfg(unix)]
    const TEST_WORKSPACE_ID: i64 = 0;

    fn in_memory_inner() -> Arc<ConversationRepositoryImpl> {
        let pool = Arc::new(DatabasePool::in_memory().expect("in-memory pool"));
        Arc::new(ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0)))
    }

    fn file_inner(root: &std::path::Path) -> Arc<ConversationRepositoryImpl> {
        let pool = Arc::new(
            DatabasePool::try_from(crate::database::PoolConfig::new(root.join("write.db")))
                .expect("file pool"),
        );
        Arc::new(ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0)))
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn bound_in_memory_mutations_stay_direct() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        SPAWN_ATTEMPTED.store(false, Ordering::SeqCst);
        let inner = in_memory_inner();
        let conversation =
            Conversation::new(ConversationId::generate()).title(Some("before".into()));
        let id = conversation.id;
        inner.upsert_conversation(conversation.clone()).await?;
        let repo =
            DaemonConversationRepository::new(inner.clone(), PathBuf::from("unused.sock"), 0);
        repo.upsert_conversation(conversation.title(Some("after".into())))
            .await?;
        assert_eq!(
            inner
                .get_conversation(&id)
                .await?
                .and_then(|row| row.title)
                .as_deref(),
            Some("after")
        );
        assert!(!SPAWN_ATTEMPTED.load(Ordering::SeqCst));
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn strict_daemon_unavailable_refuses_direct_replay() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        SPAWN_ATTEMPTED.store(false, Ordering::SeqCst);
        let temp = tempfile::tempdir()?;
        let inner = file_inner(temp.path());
        let conversation =
            Conversation::new(ConversationId::generate()).title(Some("before".into()));
        let id = conversation.id;
        inner.upsert_conversation(conversation.clone()).await?;
        let mut repo = DaemonConversationRepository::new_with_bin(
            inner.clone(),
            temp.path().join("absent.sock"),
            None,
            0,
        );
        repo.strict_daemon_protocol = true;
        assert!(
            repo.upsert_conversation(conversation.title(Some("after".into())))
                .await
                .is_err()
        );
        assert_eq!(
            inner
                .get_conversation(&id)
                .await?
                .and_then(|row| row.title)
                .as_deref(),
            Some("before")
        );
        assert!(!SPAWN_ATTEMPTED.load(Ordering::SeqCst));
        // Independent direct writers are outside the daemon protocol policy.
        inner
            .upsert_conversation(Conversation::new(id).title(Some("direct".into())))
            .await?;
        assert_eq!(
            inner
                .get_conversation(&id)
                .await?
                .and_then(|row| row.title)
                .as_deref(),
            Some("direct")
        );
        Ok(())
    }

    #[tokio::test]
    async fn strict_refuses_direct_only_paths_and_invalid_configuration() -> anyhow::Result<()> {
        let inner = in_memory_inner();
        let existing = Conversation::new(ConversationId::generate()).title(Some("before".into()));
        let id = existing.id;
        inner.upsert_conversation(existing.clone()).await?;
        let mut repo = DaemonConversationRepository::new_with_bin(
            inner.clone(),
            PathBuf::from("unused"),
            None,
            0,
        );
        repo.strict_daemon_protocol = true;
        assert!(
            repo.upsert_conversation(existing.clone().title(Some("after".into())))
                .await
                .is_err()
        );
        assert!(repo.upsert_conversation_ref(&existing).await.is_err());
        assert!(repo.update_parent_id(&id, None).await.is_err());
        assert!(repo.delete_conversation(&id).await.is_err());
        assert_eq!(
            inner
                .get_conversation(&id)
                .await?
                .and_then(|row| row.title)
                .as_deref(),
            Some("before")
        );
        repo.strict_daemon_config_invalid = true;
        assert!(
            repo.upsert_conversation(existing)
                .await
                .unwrap_err()
                .to_string()
                .contains("must be unset or 1")
        );

        let temp = tempfile::tempdir()?;
        let file = file_inner(temp.path());
        let mut file_repo = DaemonConversationRepository::new_with_bin(
            file.clone(),
            temp.path().join("unused"),
            None,
            0,
        );
        file_repo.strict_daemon_protocol = true;
        let new_conversation = Conversation::new(ConversationId::generate());
        let new_id = new_conversation.id;
        assert!(
            file_repo
                .upsert_conversation(new_conversation)
                .await
                .is_err()
        );
        assert!(file.get_conversation(&new_id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn bound_relative_database_mutations_stay_direct() -> anyhow::Result<()> {
        const ROOT: &str = "HELIOSLITE_RELATIVE_REPO_TEST_ROOT";
        if let Some(root) = std::env::var_os(ROOT) {
            std::env::set_current_dir(root)?;
            let _serial = SPAWN_GUARD.lock().unwrap();
            SPAWN_ATTEMPTED.store(false, Ordering::SeqCst);
            let inner = file_inner(std::path::Path::new("."));
            let conversation = Conversation::new(ConversationId::generate());
            let id = conversation.id;
            inner.upsert_conversation(conversation.clone()).await?;
            let repo =
                DaemonConversationRepository::new(inner.clone(), PathBuf::from("unused.sock"), 0);
            repo.upsert_conversation(conversation.title(Some("updated".into())))
                .await?;
            assert_eq!(
                inner
                    .get_conversation(&id)
                    .await?
                    .and_then(|row| row.title)
                    .as_deref(),
                Some("updated")
            );
            assert!(!SPAWN_ATTEMPTED.load(Ordering::SeqCst));
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let output = std::process::Command::new(std::env::current_exe()?)
            .arg("--exact")
            .arg("daemon_repo::tests::bound_relative_database_mutations_stay_direct")
            .arg("--nocapture")
            .env(ROOT, temp.path())
            .output()?;
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
        Ok(())
    }

    #[test]
    fn bound_spawn_uses_resolved_repository_and_socket_paths() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        let temp = tempfile::tempdir()?;
        let database = temp.path().join("custom write.db");
        let socket = temp.path().join("custom.sock");
        let pool = Arc::new(DatabasePool::try_from(PoolConfig::new(database.clone()))?);
        let inner = Arc::new(ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0)));
        let repo = DaemonConversationRepository::new(inner, socket.clone(), 0);
        let command = repo.daemon_command(std::path::Path::new("daemon"))?;
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("FORGE_WRITE_DB_PATH"))
                .copied()
                .flatten(),
            Some(database.canonicalize()?.as_os_str())
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("FORGE_DBD_SOCKET"))
                .copied()
                .flatten(),
            Some(socket.as_os_str())
        );
        Ok(())
    }

    #[test]
    fn bound_spawn_refuses_in_memory_repository() {
        let repo =
            DaemonConversationRepository::new(in_memory_inner(), PathBuf::from("daemon.sock"), 0);
        assert!(repo.daemon_command(std::path::Path::new("daemon")).is_err());
    }

    #[test]
    fn bound_spawn_refuses_relative_paths_after_cwd_change() -> anyhow::Result<()> {
        const ROOT: &str = "HELIOSLITE_BOUND_PATH_TEST_ROOT";
        if let Some(root) = std::env::var_os(ROOT) {
            let root = PathBuf::from(root);
            let first = root.join("first");
            let second = root.join("second");
            std::fs::create_dir_all(&first)?;
            std::fs::create_dir_all(&second)?;
            std::fs::write(first.join("relative.db"), b"first")?;
            std::fs::write(second.join("relative.db"), b"second")?;
            #[cfg(unix)]
            std::fs::write(second.join(":memory:"), b"literal file")?;
            std::env::set_current_dir(&first)?;
            assert!(resolve_daemon_database_path(std::path::Path::new("relative.db")).is_err());
            std::env::set_current_dir(&second)?;
            assert!(resolve_daemon_database_path(std::path::Path::new("relative.db")).is_err());
            assert!(resolve_daemon_database_path(std::path::Path::new(":memory:")).is_err());
            assert!(resolve_daemon_database_path(&root.join("missing").join("db")).is_err());
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let output = std::process::Command::new(std::env::current_exe()?)
            .arg("--exact")
            .arg("daemon_repo::tests::bound_spawn_refuses_relative_paths_after_cwd_change")
            .arg("--nocapture")
            .env(ROOT, temp.path())
            .output()?;
        assert!(
            output.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"));
        Ok(())
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn bound_new_and_unowned_rows_use_direct_validation() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        use diesel::prelude::*;
        let _serial = SPAWN_GUARD.lock().unwrap();
        SPAWN_ATTEMPTED.store(false, Ordering::SeqCst);
        let temp = tempfile::tempdir()?;
        let legacy = temp.path().join("legacy.db");
        let mut source = diesel::sqlite::SqliteConnection::establish(legacy.to_str().unwrap())?;
        diesel::sql_query("CREATE TABLE conversations (conversation_id TEXT PRIMARY KEY NOT NULL, title TEXT, created_at TIMESTAMP NOT NULL)").execute(&mut source)?;
        let legacy_id = ConversationId::generate();
        diesel::sql_query("INSERT INTO conversations VALUES (?, 'legacy', CURRENT_TIMESTAMP)")
            .bind::<diesel::sql_types::Text, _>(legacy_id.into_string())
            .execute(&mut source)?;
        drop(source);
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(temp.path().join("write.db")).with_legacy_database_path(Some(legacy)),
        )?);
        let foreign_id = ConversationId::generate();
        {
            let mut conn = pool.get_connection()?;
            diesel::sql_query("INSERT INTO conversations (conversation_id, workspace_id, title, created_at) VALUES (?, 9, 'foreign', CURRENT_TIMESTAMP)")
                .bind::<diesel::sql_types::Text, _>(foreign_id.into_string()).execute(&mut *conn)?;
        }
        let inner = Arc::new(ConversationRepositoryImpl::new(
            pool.clone(),
            WorkspaceHash::new(0),
        ));
        let repo =
            DaemonConversationRepository::new(inner.clone(), temp.path().join("missing.sock"), 0);
        assert!(
            repo.upsert_conversation(Conversation::new(legacy_id).title(Some("shadow".into())))
                .await
                .is_err()
        );
        assert!(repo.update_parent_id(&legacy_id, None).await.is_err());
        assert!(
            repo.upsert_conversation(Conversation::new(foreign_id).title(Some("overwrite".into())))
                .await
                .is_err()
        );
        assert!(repo.update_parent_id(&foreign_id, None).await.is_err());
        let fresh = Conversation::new(ConversationId::generate()).title(Some("fresh".into()));
        let fresh_id = fresh.id;
        repo.upsert_conversation(fresh).await?;
        assert_eq!(
            inner
                .get_conversation(&fresh_id)
                .await?
                .and_then(|row| row.title)
                .as_deref(),
            Some("fresh")
        );
        assert!(
            !SPAWN_ATTEMPTED.load(Ordering::SeqCst),
            "new or unowned rows must not reach daemon spawn"
        );
        let mut conn = pool.get_connection()?;
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            n: i64,
        }
        let local =
            diesel::sql_query("SELECT COUNT(*) AS n FROM conversations WHERE conversation_id = ?")
                .bind::<diesel::sql_types::Text, _>(legacy_id.into_string())
                .get_result::<Count>(&mut *conn)?
                .n;
        assert_eq!(local, 0);
        Ok(())
    }

    /// The decorator must fall back to the direct implementation when the
    /// daemon is absent: a socket path that does not exist makes
    /// `DbClient::connect` fail, and the write still lands in the inner
    /// repository.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // The process-wide spawn state makes these tests serial by design.
    async fn falls_back_to_direct_write_when_daemon_absent() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        // Mark the spawn as already attempted so this test never launches a
        // real daemon; the assertion is about the fallback, not the spawn.
        SPAWN_ATTEMPTED.store(true, Ordering::SeqCst);

        let temp = tempfile::tempdir()?;
        let inner = file_inner(temp.path());
        let repo = DaemonConversationRepository::new(
            inner.clone(),
            PathBuf::from("/nonexistent/.forge.db.sock"),
            0,
        );

        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("daemon-fallback".to_string()));
        let id = conversation.id;

        inner.upsert_conversation(conversation.clone()).await?;
        repo.upsert_conversation(conversation).await?;

        let actual = inner.get_conversation(&id).await?;
        assert_eq!(
            actual.expect("row persisted via direct fallback").title,
            Some("daemon-fallback".to_string())
        );
        Ok(())
    }

    /// With `FORGE_DBD_BIN` pointing at a binary that does not exist, the
    /// first routed write attempts to spawn it exactly once, fails, and falls
    /// back to the direct path — the row still persists. A second write does
    /// not re-attempt the spawn (the guard is set) and falls back without
    /// hanging.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // The process-wide spawn state makes these tests serial by design.
    async fn spawn_is_attempted_once_then_falls_back() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        // Reset the process-wide guard so this test exercises the spawn path
        // deterministically.
        SPAWN_ATTEMPTED.store(false, Ordering::SeqCst);

        let temp = tempfile::tempdir()?;
        let inner = file_inner(temp.path());
        let repo = DaemonConversationRepository::new_with_bin(
            inner.clone(),
            PathBuf::from("/nonexistent/.forge.db.sock"),
            Some(PathBuf::from("/definitely/missing/forge_dbd_bin")),
            0,
        );

        let conversation =
            Conversation::new(ConversationId::generate()).title(Some("spawn-fallback".to_string()));
        let id = conversation.id;

        inner.upsert_conversation(conversation.clone()).await?;
        repo.upsert_conversation(conversation).await?;
        let actual = inner.get_conversation(&id).await?;
        assert_eq!(
            actual.expect("row persisted via direct fallback").title,
            Some("spawn-fallback".to_string())
        );

        // Second write: the spawn guard is set, so no respawn (and no 2 s
        // retry wait) — straight to the direct fallback.
        let second =
            Conversation::new(ConversationId::generate()).title(Some("second".to_string()));
        let second_id = second.id;
        repo.upsert_conversation(second).await?;
        let actual = inner.get_conversation(&second_id).await?;
        assert_eq!(
            actual
                .expect("second row persisted via direct fallback")
                .title,
            Some("second".to_string())
        );

        Ok(())
    }

    /// A second write arriving while the first one is polling a freshly
    /// spawned daemon must not bypass that in-progress recovery by falling
    /// straight back to SQLite. Otherwise both the daemon and direct path can
    /// write concurrently once the daemon binds.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // The process-wide spawn state makes these tests serial by design.
    async fn concurrent_write_waits_for_in_progress_daemon_spawn() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        SPAWN_ATTEMPTED.store(false, Ordering::SeqCst);

        let temp = tempfile::tempdir()?;
        let inner = file_inner(temp.path());
        let repo = DaemonConversationRepository::new_with_spawn_success(
            inner.clone(),
            PathBuf::from("/nonexistent/forge-dbd-delayed.sock"),
            0,
        );
        let first = Conversation::new(ConversationId::generate())
            .title(Some("first-startup-write".to_string()));
        let second = Conversation::new(ConversationId::generate())
            .title(Some("second-startup-write".to_string()));
        inner.upsert_conversation(first.clone()).await?;
        inner.upsert_conversation(second.clone()).await?;

        let first_write = repo.upsert_conversation(first);
        tokio::pin!(first_write);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            result = &mut first_write => panic!("first write completed before delayed spawn window: {result:?}"),
        }

        let second_write =
            tokio::time::timeout(Duration::from_millis(100), repo.upsert_conversation(second))
                .await;
        assert!(
            second_write.is_err(),
            "second write must wait for the first spawn/poll instead of direct-falling back"
        );

        first_write.await?;
        Ok(())
    }

    /// A healthy daemon may receive independent writes concurrently. In
    /// particular, a delayed response to the first request must not retain a
    /// lifecycle or cache mutex that prevents the second request from being
    /// dispatched (or from performing later cache recovery).
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // Serializes the process-wide spawn flag.
    async fn stalled_daemon_response_does_not_serialize_healthy_writes() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        SPAWN_ATTEMPTED.store(true, Ordering::SeqCst);

        let temp_dir = tempfile::tempdir()?;
        let socket_path = temp_dir.path().join("forge-dbd.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let inner = in_memory_inner();
        let repo = Arc::new(DaemonConversationRepository::new(inner, socket_path, 0));

        let (first_received_tx, first_received_rx) = oneshot::channel();
        let (second_received_tx, second_received_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            // `DbClient::connect` first negotiates the mutation protocol.
            // Complete that non-mutating exchange before accepting writes.
            let (mut probe, _) = listener.accept().await.unwrap();
            let request: Request = read_frame(&mut probe).await.unwrap();
            assert!(matches!(request, Request::Ping));
            write_frame(
                &mut probe,
                &Response::Health(HealthStatus {
                    strict_daemon_protocol: false,
                    protocol_version: 2,
                    uptime_secs: 0,
                    queue_depth: 0,
                    db_reachable: true,
                }),
            )
            .await
            .unwrap();
            drop(probe);

            let (mut first_stream, _) = listener.accept().await.unwrap();
            let _: Request = read_frame(&mut first_stream).await.unwrap();
            first_received_tx.send(()).unwrap();

            let (mut second_stream, _) = listener.accept().await.unwrap();
            let _: Request = read_frame(&mut second_stream).await.unwrap();
            second_received_tx.send(()).unwrap();

            release_rx.await.unwrap();
            write_frame(&mut first_stream, &Response::Ack)
                .await
                .unwrap();
            write_frame(&mut second_stream, &Response::Ack)
                .await
                .unwrap();
        });
        let cached_client = DbClient::connect(repo.socket_path.as_path()).await?;
        *repo.client.lock().await = Some(Arc::new(cached_client));

        let first_repo = repo.clone();
        let first = tokio::spawn(async move {
            first_repo
                .try_daemon(Request::MutationV2 {
                    workspace_id: TEST_WORKSPACE_ID,
                    mutation: ConversationMutation::UpdateParentId {
                        conversation_id: ConversationId::generate(),
                        new_parent_id: None,
                    },
                })
                .await
        });
        first_received_rx.await?;

        let second_repo = repo.clone();
        let second = tokio::spawn(async move {
            second_repo
                .try_daemon(Request::MutationV2 {
                    workspace_id: TEST_WORKSPACE_ID,
                    mutation: ConversationMutation::UpdateParentId {
                        conversation_id: ConversationId::generate(),
                        new_parent_id: None,
                    },
                })
                .await
        });
        tokio::time::timeout(Duration::from_millis(150), second_received_rx)
            .await
            .expect("second healthy write must not wait for the first response")?;

        release_tx.send(()).unwrap();
        assert!(matches!(first.await?, DaemonWriteOutcome::Ack));
        assert!(matches!(second.await?, DaemonWriteOutcome::Ack));
        server.await?;
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn bound_lost_ack_never_replays_through_direct_repository() -> anyhow::Result<()> {
        use forge_dbd::protocol::{HealthStatus, read_frame, write_frame};
        use tokio::net::windows::named_pipe::ServerOptions;
        let temp = tempfile::tempdir()?;
        let socket = temp.path().join("lost-ack.sock");
        let pipe = forge_dbd::protocol::named_pipe_name(&socket);
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe)?;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (recorded_tx, recorded_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            first.connect().await.unwrap();
            let mut first = first;
            assert!(matches!(
                read_frame::<_, Request>(&mut first).await.unwrap(),
                Request::Ping
            ));
            write_frame(
                &mut first,
                &Response::Health(HealthStatus {
                    strict_daemon_protocol: false,
                    protocol_version: 3,
                    uptime_secs: 0,
                    queue_depth: 0,
                    db_reachable: true,
                }),
            )
            .await
            .unwrap();
            let second = ServerOptions::new().create(&pipe).unwrap();
            drop(first);
            ready_tx.send(()).unwrap();
            second.connect().await.unwrap();
            let mut second = second;
            assert!(matches!(
                read_frame::<_, Request>(&mut second).await.unwrap(),
                Request::Ping
            ));
            write_frame(
                &mut second,
                &Response::Health(HealthStatus {
                    strict_daemon_protocol: false,
                    protocol_version: 3,
                    uptime_secs: 0,
                    queue_depth: 0,
                    db_reachable: true,
                }),
            )
            .await
            .unwrap();
            let request = read_frame::<_, Request>(&mut second).await.unwrap();
            assert!(matches!(request, Request::MutationV3 { .. }));
            recorded_tx.send(()).unwrap();
        });
        let inner = file_inner(temp.path());
        let conversation =
            Conversation::new(ConversationId::generate()).title(Some("before".into()));
        let id = conversation.id;
        inner.upsert_conversation(conversation.clone()).await?;
        let repo = DaemonConversationRepository::new(inner.clone(), socket, 0);
        let client = DbClient::connect(&repo.socket_path).await?;
        *repo.client.lock().await = Some(Arc::new(client));
        ready_rx.await?;
        let result = repo
            .upsert_conversation(conversation.title(Some("after".into())))
            .await;
        assert!(result.is_err());
        recorded_rx.await?;
        assert_eq!(
            inner
                .get_conversation(&id)
                .await?
                .and_then(|row| row.title)
                .as_deref(),
            Some("before")
        );
        task.await?;
        Ok(())
    }

    /// `DbClient` is only a successful probe cache; every send opens a new
    /// stream. If the daemon exits after that probe, a refusal while opening
    /// the fresh stream is pre-send and may safely use the direct writer.
    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // The process-wide spawn state makes these tests serial by design.
    async fn stale_cached_client_pre_send_refusal_falls_back_directly() -> anyhow::Result<()> {
        let _serial = SPAWN_GUARD.lock().unwrap();
        SPAWN_ATTEMPTED.store(true, Ordering::SeqCst);

        let temp_dir = tempfile::tempdir()?;
        let socket_path = temp_dir.path().join("forge-dbd.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let inner = file_inner(temp_dir.path());
        let repo = DaemonConversationRepository::new(inner.clone(), socket_path, 0);
        let server = tokio::spawn(async move {
            // `DbClient::connect` first negotiates the mutation protocol.
            // Complete that non-mutating exchange before removing the daemon.
            let (mut probe, _) = listener.accept().await.unwrap();
            let request: Request = read_frame(&mut probe).await.unwrap();
            assert!(matches!(request, Request::Ping));
            write_frame(
                &mut probe,
                &Response::Health(HealthStatus {
                    strict_daemon_protocol: false,
                    protocol_version: 2,
                    uptime_secs: 0,
                    queue_depth: 0,
                    db_reachable: true,
                }),
            )
            .await
            .unwrap();
            drop(probe);
            drop(listener);
        });
        let cached_client = DbClient::connect(repo.socket_path.as_path()).await?;
        *repo.client.lock().await = Some(Arc::new(cached_client));
        server.await?;

        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("stale-client-direct-fallback".to_string()));
        let id = conversation.id;
        inner
            .upsert_conversation(conversation.clone().title(Some("before".into())))
            .await?;
        repo.upsert_conversation(conversation).await?;

        assert!(
            inner
                .get_conversation(&id)
                .await?
                .and_then(|row| row.title)
                .as_deref()
                == Some("stale-client-direct-fallback"),
            "a pre-send connection refusal must fall back without replay risk"
        );
        Ok(())
    }

    /// Once a daemon accepts a write request, a lost ACK is indeterminate: it
    /// may have committed the write, so the decorator must return an explicit
    /// error instead of replaying the request through the direct repository.
    #[cfg(unix)]
    #[tokio::test]
    async fn does_not_fallback_after_daemon_records_request_then_loses_ack() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let socket_path = temp_dir.path().join("forge-dbd.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let (recorded_tx, recorded_rx) = oneshot::channel();

        tokio::spawn(async move {
            // `DbClient::connect` first negotiates the mutation protocol.
            // Complete that non-mutating exchange before accepting the write
            // transport whose ACK we intentionally lose.
            let (mut probe, _) = listener.accept().await.unwrap();
            let request: Request = read_frame(&mut probe).await.unwrap();
            assert!(matches!(request, Request::Ping));
            write_frame(
                &mut probe,
                &Response::Health(HealthStatus {
                    strict_daemon_protocol: false,
                    protocol_version: 3,
                    uptime_secs: 0,
                    queue_depth: 0,
                    db_reachable: true,
                }),
            )
            .await
            .unwrap();
            drop(probe);

            let (mut write_stream, _) = listener.accept().await.unwrap();
            let probe: Request = read_frame(&mut write_stream).await.unwrap();
            assert!(matches!(probe, Request::Ping));
            write_frame(
                &mut write_stream,
                &Response::Health(HealthStatus {
                    strict_daemon_protocol: false,
                    protocol_version: 3,
                    uptime_secs: 0,
                    queue_depth: 0,
                    db_reachable: true,
                }),
            )
            .await
            .unwrap();
            let request: Request = read_frame(&mut write_stream).await.unwrap();
            recorded_tx.send(request).unwrap();
            // Simulate a daemon that committed/recorded the request, then
            // crashes before emitting Ack.
        });

        let inner = file_inner(temp_dir.path());
        let repo = DaemonConversationRepository::new(inner.clone(), socket_path, 0);
        let conversation =
            Conversation::new(ConversationId::generate()).title(Some("ack-loss".to_string()));
        let id = conversation.id;
        inner
            .upsert_conversation(conversation.clone().title(Some("before".into())))
            .await?;
        let database_token = inner.daemon_database_token().await?;

        let actual = repo.upsert_conversation(conversation.clone()).await;
        assert!(actual.is_err());

        let actual = recorded_rx.await.expect("daemon recorded request");
        let expected = Request::MutationV3 {
            workspace_id: TEST_WORKSPACE_ID,
            database_token,
            mutation: ConversationMutation::UpsertConversation { conversation, workspace_id: None },
        };
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));

        let actual = inner.get_conversation(&id).await?;
        assert_eq!(actual.and_then(|row| row.title).as_deref(), Some("before"));
        Ok(())
    }
}
