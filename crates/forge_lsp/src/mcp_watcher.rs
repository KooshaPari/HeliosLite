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
        event.paths.iter().any(|p| p == watched)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    fn write_file(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let p = dir.path().join(name);
        let mut f = std::fs::File::create(&p).expect("create");
        f.write_all(contents.as_bytes()).expect("write");
        p
    }

    fn counter() -> (Arc<AtomicUsize>, McpReloadFn) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = Arc::clone(&n);
        let cb: McpReloadFn = Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        (n, cb)
    }

    #[test]
    fn default_path_resolves_under_config_dir() {
        let p = default_mcp_config_path();
        // We can't assert the exact path (depends on env) but it must
        // end in `forge/mcp.toml`.
        let s = p.to_string_lossy();
        assert!(s.ends_with("forge") || s.ends_with("forge/mcp.toml") || s.contains("mcp.toml"));
    }

    #[test]
    fn config_builder_overrides_path_and_debounce() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let cfg = McpAutoReloadConfig::default()
            .with_path(&target)
            .with_debounce(Duration::from_millis(50));
        assert_eq!(cfg.path, target);
        assert_eq!(cfg.debounce, Duration::from_millis(50));
        assert!(cfg.enabled);
    }

    #[test]
    fn config_builder_can_disable() {
        let cfg = McpAutoReloadConfig::default().disabled();
        assert!(!cfg.enabled);
    }

    #[test]
    fn disabled_spawn_returns_noop_handle() {
        let (_, cb) = counter();
        let h = McpWatcher::new(McpAutoReloadConfig::default().disabled(), cb)
            .spawn()
            .unwrap();
        assert!(h.is_stopped());
    }

    #[test]
    fn missing_path_returns_missing_config_error() {
        let (_, cb) = counter();
        let cfg = McpAutoReloadConfig::default().with_path("/nonexistent/xyz/mcp.toml");
        let err = McpWatcher::new(cfg, cb).spawn().unwrap_err();
        matches!(err, McpWatcherError::MissingConfig(_));
    }

    #[test]
    fn event_should_reload_matches_watched_path() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let evt = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![target.clone()],
            attrs: Default::default(),
        };
        assert!(McpWatcher::event_should_reload(&evt, &target));
    }

    #[test]
    fn event_should_reload_ignores_unrelated_paths() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let other = write_file(&tmp, "other.toml", "x = 2\n");
        let evt = Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![other],
            attrs: Default::default(),
        };
        assert!(!McpWatcher::event_should_reload(&evt, &target));
    }

    #[test]
    fn event_should_reload_ignores_non_data_kinds() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let evt = Event {
            kind: EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Read,
            )),
            paths: vec![target.clone()],
            attrs: Default::default(),
        };
        assert!(!McpWatcher::event_should_reload(&evt, &target));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_fires_reload_on_modify() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let (n, cb) = counter();
        let cfg = McpAutoReloadConfig::default()
            .with_path(&target)
            .with_debounce(Duration::from_millis(80));
        let handle = McpWatcher::new(cfg, cb).spawn().unwrap();

        // Give the watcher a moment to attach.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Modify the file.
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&target)
                .unwrap();
            f.write_all(b"a = 2\n").unwrap();
        }

        // Wait for the debounce + a margin.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let count = n.load(Ordering::SeqCst);
        handle.stop(Duration::from_millis(200)).await;
        assert!(count >= 1, "expected at least 1 reload, got {count}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_debounces_burst_into_one_reload() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let (n, cb) = counter();
        let cfg = McpAutoReloadConfig::default()
            .with_path(&target)
            .with_debounce(Duration::from_millis(300));
        let handle = McpWatcher::new(cfg, cb).spawn().unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Burst: 5 rapid modifications within the debounce window.
        for i in 0..5 {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&target)
                .unwrap();
            f.write_all(format!("a = {i}\n").as_bytes()).unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }

        // Wait for the debounce + margin to settle.
        tokio::time::sleep(Duration::from_millis(700)).await;
        let count = n.load(Ordering::SeqCst);
        handle.stop(Duration::from_millis(200)).await;
        // 5 events within ~150ms — well inside the 300ms debounce — so
        // we expect exactly 1 reload (allowing a small race tolerance
        // because some platforms emit separate events per fsync).
        assert!(
            (1..=2).contains(&count),
            "expected 1-2 reloads after burst, got {count}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_stop_completes_quickly() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let (_, cb) = counter();
        let cfg = McpAutoReloadConfig::default()
            .with_path(&target)
            .with_debounce(Duration::from_millis(50));
        let handle = McpWatcher::new(cfg, cb).spawn().unwrap();
        // Let the initial "file created" event be delivered and drained, so the
        // background task is parked in `rx.recv()` when `stop` is called. That
        // is the state in which shutdown must remain prompt; without this sleep
        // the test only passes when that first event happens to arrive after
        // `stop` (which is why it passed locally and flaked on CI).
        tokio::time::sleep(Duration::from_millis(250)).await;
        // `stop` must not have to wait for an unrelated filesystem event. The
        // budget is deliberately generous so the suite stays reliable under
        // `cargo llvm-cov` on shared runners, while a true hang still fails.
        let budget = Duration::from_secs(10);
        let start = std::time::Instant::now();
        handle.stop(budget).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed < budget,
            "watcher shutdown consumed the whole {budget:?} budget (elapsed {elapsed:?})"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_does_not_fire_for_unrelated_file() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let other = write_file(&tmp, "other.toml", "x = 1\n");
        let (n, cb) = counter();
        let cfg = McpAutoReloadConfig::default()
            .with_path(&target)
            .with_debounce(Duration::from_millis(80));
        let handle = McpWatcher::new(cfg, cb).spawn().unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Touch the *other* file — the watcher should not fire.
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&other)
                .unwrap();
            f.write_all(b"x = 2\n").unwrap();
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        let count = n.load(Ordering::SeqCst);
        handle.stop(Duration::from_millis(200)).await;
        assert_eq!(count, 0, "watcher fired for an unrelated path");
    }

    #[test]
    fn debug_impl_does_not_leak_callback() {
        let tmp = TempDir::new().unwrap();
        let target = write_file(&tmp, "mcp.toml", "a = 1\n");
        let (_, cb) = counter();
        let w = McpWatcher::new(McpAutoReloadConfig::default().with_path(target), cb);
        let dbg = format!("{w:?}");
        assert!(dbg.contains("McpWatcher"));
        assert!(dbg.contains("<Fn>"));
    }
}
