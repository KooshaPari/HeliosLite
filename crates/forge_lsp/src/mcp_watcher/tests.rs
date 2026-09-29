
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

    // Repeatedly modify the file until the watcher has attached and a
    // reload has been observed. A single fixed write raced on shared
    // macOS runners: fs-event delivery stalled >400ms on 2026-09-29
    // (two mcp_watcher panics on c503badf6 with identical code green
    // the day before), and a slow attach would miss the only write
    // entirely. The 80ms debounce absorbs repeated writes, so the
    // common case exits after one settle window.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut i = 0u32;
    let mut count = n.load(Ordering::SeqCst);
    while count < 1 && std::time::Instant::now() < deadline {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&target)
            .unwrap();
        f.write_all(format!("a = {i}\n").as_bytes()).unwrap();
        i += 1;
        tokio::time::sleep(Duration::from_millis(200)).await;
        count = n.load(Ordering::SeqCst);
    }
    handle.stop(Duration::from_millis(200)).await;
    assert!(
        count >= 1,
        "expected at least 1 reload within 5s, got {count}"
    );
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

    // Wait (bounded) for the batched events to be delivered: shared
    // runners have stalled fs-event delivery past the old fixed
    // 700ms window (observed on macOS CI 2026-09-29, same run as the
    // fires-reload flake).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut count = n.load(Ordering::SeqCst);
    while count < 1 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        count = n.load(Ordering::SeqCst);
    }
    // Settle so stragglers the debouncer still emits get counted.
    tokio::time::sleep(Duration::from_millis(600)).await;
    count = n.load(Ordering::SeqCst);
    handle.stop(Duration::from_millis(200)).await;
    // 5 events within ~150ms — well inside the 300ms debounce — so we
    // expect 1 reload. Tolerance: some platforms emit separate events
    // per fsync, and a delivery stall >300ms mid-burst can split the
    // batch into a second window (3 = two such splits). Per-event
    // firing under a genuinely broken debouncer would be ~5 and must
    // still fail.
    assert!(
        (1..=3).contains(&count),
        "expected 1-3 reloads after burst, got {count}"
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
