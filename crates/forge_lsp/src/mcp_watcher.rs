//! `McpWatcher` — debounced filesystem watcher for the MCP server config.
//!
//! PR #282 added `last_seen` metadata to the MCP health surface. This
//! module extends that to *active* reload: when the on-disk config
//! changes, the host's existing MCP reload function is invoked after a
//! short debounce window so editor noise (the typical save-then-flush
//! burst from an editor) doesn't trigger N reloads.
//!
//! The watcher is decoupled from the actual reload implementation: a
//! caller passes in a `McpReloadFn` closure (or any `Fn() + Send +
//! 'static`) so the LSP layer doesn't depend on whichever MCP registry
//! the host binary wires up.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;
use tokio::sync::Notify;

/// Default location of the MCP server config file.
pub fn default_mcp_config_path() -> PathBuf {
    // `dirs` is in workspace deps but we want to avoid pulling it into
    // the LSP crate just for one lookup; honour $XDG_CONFIG_HOME and
    // fall back to $HOME/.config/forge/mcp.toml.
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = PathBuf::from(h);
                p.push(".config");
                p
            })
        })
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("forge").join("mcp.toml")
}

/// Default debounce window. Long enough to absorb editor save bursts,
/// short enough that interactive users don't notice it.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Configuration for [`McpWatcher`].
#[derive(Debug, Clone)]
pub struct McpAutoReloadConfig {
    /// Whether the watcher is enabled. When `false`, `spawn` returns
    /// a no-op handle.
    pub enabled: bool,
    /// Path to the MCP config file to watch.
    pub path: PathBuf,
    /// Debounce window — events that arrive within this duration of
    /// each other are coalesced into a single reload.
    pub debounce: Duration,
}

impl Default for McpAutoReloadConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: default_mcp_config_path(),
            debounce: DEFAULT_DEBOUNCE,
        }
    }
}

impl McpAutoReloadConfig {
    /// Override the watched path (mainly for tests).
    #[must_use]
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = path.into();
        self
    }

    /// Override the debounce window.
    #[must_use]
    pub fn with_debounce(mut self, debounce: Duration) -> Self {
        self.debounce = debounce;
        self
    }

    /// Disable the watcher (returns a no-op handle from `spawn`).
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// Errors that can occur when constructing / spawning the watcher.
#[derive(Debug, Error)]
pub enum McpWatcherError {
    /// The supplied path doesn't exist or isn't a regular file.
    #[error("mcp config path does not exist: {0}")]
    MissingConfig(PathBuf),
    /// `notify` failed to attach to the path (permission, OS-level).
    #[error("notify watcher error: {0}")]
    Notify(String),
    /// The watcher couldn't find a parent directory to attach to.
    #[error("mcp config path has no parent directory: {0}")]
    NoParent(PathBuf),
}

/// Type alias for the reload callback. Typically wraps the host crate's
/// existing MCP reload function.
pub type McpReloadFn = Arc<dyn Fn() + Send + Sync + 'static>;

/// Handle returned by [`McpWatcher::spawn`]. Drop to stop the watcher.
pub struct McpWatcherHandle {
    /// Set to `true` to request shutdown. The background task observes
    /// this flag and exits promptly.
    stop: Arc<AtomicBool>,
    /// Notified when the background task has fully exited (so callers
    /// can join deterministically in tests).
    done: Arc<Notify>,
    /// Wakes the background task out of its `rx.recv()` await. Without
    /// this, a handle parked waiting for the next filesystem event never
    /// observes the stop flag, so shutdown would block until an unrelated
    /// event arrived (or the caller's timeout expired).
    shutdown: Arc<Notify>,
}

impl std::fmt::Debug for McpWatcherHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpWatcherHandle").finish_non_exhaustive()
    }
}

impl McpWatcherHandle {
    /// Stop the watcher and wait (up to `timeout`) for the background
    /// task to exit.
    pub async fn stop(self, timeout: Duration) {
        // A disabled/no-op handle owns no task, so there is nothing to join.
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        // `notify_one` stores a permit when no waiter is registered yet, so a
        // shutdown request cannot be lost between the flag check in the task
        // and its `select!`.
        self.shutdown.notify_one();
        let _ = tokio::time::timeout(timeout, self.done.notified()).await;
    }

    /// Signal shutdown without waiting.
    pub fn signal_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.shutdown.notify_one();
    }
}

/// The MCP config watcher. Holds a `notify::RecommendedWatcher` plus
/// the reload callback and debounce state.
pub struct McpWatcher {
    config: McpAutoReloadConfig,
    reload: McpReloadFn,
}

impl std::fmt::Debug for McpWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpWatcher")
            .field("config", &self.config)
            .field("reload", &"<Fn>")
            .finish()
    }
}

impl McpWatcher {
    /// Build a new watcher with the supplied config and reload callback.
    pub fn new(config: McpAutoReloadConfig, reload: McpReloadFn) -> Self {
        Self { config, reload }
    }

    /// Build a watcher with default config and the supplied callback.
    pub fn with_default(reload: McpReloadFn) -> Self {
        Self::new(McpAutoReloadConfig::default(), reload)
    }

    /// Accessor for the config (handy in tests).
    pub fn config(&self) -> &McpAutoReloadConfig {
        &self.config
    }

    /// Return `true` if the supplied [`Event`] should trigger a reload.
    /// We react to any modify/create/remove on the watched file or a
    /// file matching the same name in the same directory.
    pub fn event_should_reload(event: &Event, watched: &Path) -> bool {
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return false;
        }
        event.paths.iter().any(|p| Self::paths_match(p, watched))
    }

    /// Compare an fs-event path against the watched path, tolerating the
    /// symlink resolution the OS backend applies.
    ///
    /// macOS FSEvents reports fully resolved paths, so a watched path under
    /// `/var/folders/...` (the default user temp dir, and the CI runner's
    /// `$TMPDIR`) comes back as `/private/var/folders/...`. A bare `==` then
    /// never matches and the watcher observes zero reloads. Normalising the
    /// *parent* directory (which is where the `/var -> /private/var` link
    /// lives) while keeping the final component resolves this and still works
    /// for `Remove` events, where the file itself no longer exists.
    fn paths_match(a: &Path, b: &Path) -> bool {
        a == b || normalize_parent(a) == normalize_parent(b)
    }

    /// Spawn the watcher onto the current Tokio runtime. Returns a
    /// handle whose `stop` method shuts the watcher down.
    ///
    /// When `config.enabled` is `false`, returns a no-op handle that
    /// never invokes the reload callback.
    pub fn spawn(self) -> Result<McpWatcherHandle, McpWatcherError> {
        if !self.config.enabled {
            return Ok(McpWatcherHandle::noop());
        }

        if !self.config.path.exists() {
            return Err(McpWatcherError::MissingConfig(self.config.path.clone()));
        }

        let parent = self
            .config
            .path
            .parent()
            .ok_or_else(|| McpWatcherError::NoParent(self.config.path.clone()))?
            .to_path_buf();
        let watched = self.config.path.clone();
        let debounce = self.config.debounce;
        let reload = Arc::clone(&self.reload);

        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(Notify::new());
        let shutdown = Arc::new(Notify::new());
        let stop_bg = Arc::clone(&stop);
        let done_bg = Arc::clone(&done);
        let shutdown_for_task = Arc::clone(&shutdown);

        // Channel between the synchronous notify thread and the async
        // tokio task that owns the debounce timer.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(32);

        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<Event>| {
                match res {
                    Ok(event) if Self::event_should_reload(&event, &watched) => {
                        // Best-effort: if the receiver is full (debounce
                        // window still active), the message is dropped and
                        // the already-pending reload still fires.
                        let _ = tx.blocking_send(());
                    }
                    Ok(_) | Err(_) => {}
                }
            })
            .map_err(|e| McpWatcherError::Notify(e.to_string()))?;

        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .map_err(|e| McpWatcherError::Notify(e.to_string()))?;

        let reload_for_task = Arc::clone(&reload);
        let stop_for_task = Arc::clone(&stop_bg);
        let done_for_task = Arc::clone(&done_bg);

        tokio::spawn(async move {
            // Keep the watcher alive for the duration of the task.
            let _watcher = watcher;

            // Use the *longest* debounce window seen since the last
            // reload: this collapses bursts of editor-save events into
            // a single reload. Every wait below is interruptible by the
            // shutdown notification so `stop` never waits on the
            // filesystem.
            'watch: while !stop_for_task.load(Ordering::SeqCst) {
                // Wait for the first event.
                tokio::select! {
                    biased;
                    _ = shutdown_for_task.notified() => break 'watch,
                    maybe = rx.recv() => {
                        if maybe.is_none() {
                            break 'watch;
                        }
                    }
                }
                // Drain any further events that arrive within the
                // debounce window.
                loop {
                    tokio::select! {
                        biased;
                        _ = shutdown_for_task.notified() => break 'watch,
                        res = tokio::time::timeout(debounce, rx.recv()) => {
                            match res {
                                Ok(Some(())) => continue,
                                Ok(None) | Err(_) => break, // closed or elapsed — time to fire
                            }
                        }
                    }
                }
                if stop_for_task.load(Ordering::SeqCst) {
                    break;
                }
                (reload_for_task)();
            }
            // `notify_one` stores a permit, so a caller that reaches
            // `done.notified()` after the task exited still observes it.
            done_for_task.notify_one();
        });

        Ok(McpWatcherHandle { stop, done, shutdown })
    }
}

impl McpWatcherHandle {
    /// Construct a no-op handle (used when the watcher is disabled).
    fn noop() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(true)),
            done: Arc::new(Notify::new()),
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Whether the underlying task has been asked to stop.
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// Resolve symlinks in the *parent* directory of `p`, keeping the final
/// component verbatim. This makes path comparison backend-agnostic (macOS
/// FSEvents resolves `/var` to `/private/var`) while still working when the
/// leaf file has just been removed and can no longer be canonicalised.
fn normalize_parent(p: &Path) -> PathBuf {
    match (p.parent(), p.file_name()) {
        (Some(dir), Some(name)) if !dir.as_os_str().is_empty() => {
            std::fs::canonicalize(dir).map_or_else(|_| p.to_path_buf(), |c| c.join(name))
        }
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests;
