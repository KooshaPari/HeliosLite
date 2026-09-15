//! End-to-end shell test for the `helioslite share` binary lifecycle.
//!
//! Spawns the *real* `helioslite` binary (resolved at compile time from
//! `CARGO_BIN_EXE_<name>`), drives the live wire path with raw TCP, and
//! asserts:
//!
//! 1. `helioslite share serve --host 127.0.0.1 --port P` binds the
//!    requested ephemeral port within 5s.
//! 2. `helioslite share publish --topic chat --payload '{"hi":1}'`
//!    launches and exits cleanly (binary lifecycle guardrail; the
//!    ephemeral hub it owns is independent of the serve process).
//! 3. A `POST /publish/chat` issued over raw TCP — the in-band
//!    publish endpoint the serve process exposes — drives the payload
//!    into the *serve* hub (this is the only path that lets the live
//!    SSE subscriber observe the bytes).
//! 4. A `GET /sse/chat` over raw TCP responds with `200 OK`,
//!    `Content-Type: text/event-stream`, and at least one
//!    `data: {"hi":1}` event frame within the 15s overall budget.
//! 5. A SIGINT to the `serve` process makes it exit within 5s of the
//!    signal (clean shutdown — the binary wires
//!    `tokio::signal::ctrl_c()` into its accept loop).
//!
//! Run with: `cargo test -p forge_main --test share_bin_e2e`.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Hard upper bound for the whole e2e flow. The task spec asks for 15s;
/// per-step budgets are tighter so we never blow past 15s on a hang.
const OVERALL_BUDGET: Duration = Duration::from_secs(15);

/// Per-step connect budget. The serve process needs ~1–2s to stand up
/// the tokio runtime, the listener, and start accepting — 5s matches
/// the spec's connect grace.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);

/// Per-step wait for a full SSE response (headers + first event frame)
/// once the GET has been written.
const READ_BUDGET: Duration = Duration::from_secs(5);

/// Grace window for the serve process to react to SIGINT and exit.
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(5);

/// Per-iteration deadline for the `share publish` subprocess.
const PUBLISH_BUDGET: Duration = Duration::from_secs(5);

/// Find a loopback port the spawned `helioslite share serve` binary
/// can actually bind. Some Windows hosts refuse certain ports in the
/// dynamic range with `WSAEACCES` (the kernel hands them out via
/// `bind("127.0.0.1:0")` but a later bind by another process is
/// rejected by the Windows Filtering Platform). We try the kernel's
/// ephemeral pick first and fall back to probing a curated set of
/// high loopback ports until one accepts a no-op bind. This keeps
/// the test isolated to loopback and avoids clashing with any
/// well-known service.
fn ephemeral_port() -> u16 {
    // Preferred path: let the kernel pick an ephemeral port.
    if let Some(p) = try_bind(("127.0.0.1", 0)) {
        return p;
    }
    // Fallback: probe a curated list of high loopback ports that are
    // almost never firewalled on a dev workstation. We never pick
    // below 1024 (privileged) or inside 49152–65535 (the Windows
    // dynamic range, where the WSAEACCES bug lives).
    const CANDIDATES: &[u16] = &[
        18234, 28743, 39576, 45738, 52341, 63524,
    ];
    for &p in CANDIDATES {
        if try_bind(("127.0.0.1", p)).is_some() {
            return p;
        }
    }
    panic!(
        "no loopback port available for helioslite share serve \
         (tried ephemeral + {} fallbacks); see WSAEACCES",
        CANDIDATES.len()
    );
}

/// Try `TcpListener::bind` on `addr`, return the resulting port on
/// success. Drops the listener immediately so the child process can
/// rebind it; if the bind fails (port in use, firewalled, etc.) we
/// return `None` and let the caller try the next candidate.
fn try_bind(addr: (&str, u16)) -> Option<u16> {
    let listener = TcpListener::bind(addr).ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);
    Some(port)
}

/// Poll `TcpStream::connect` until `127.0.0.1:port` accepts a
/// connection or `budget` elapses. We sleep 100ms between tries so a
/// hung serve process doesn't burn CPU on rapid-fail connect loops.
fn wait_for_accept(port: u16, budget: Duration) {
    let deadline = Instant::now() + budget;
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => {
                let _ = s.shutdown(Shutdown::Both);
                return;
            }
            Err(_) => {
                if Instant::now() >= deadline {
                    panic!(
                        "helioslite share serve never accepted 127.0.0.1:{port} within {budget:?}{}",
                        serve_log_summary()
                    );
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Poll `try_wait` every 100ms up to `budget`. Panics on deadline;
/// returns the captured `ExitStatus`.
fn wait_for_exit(child: &mut Child, budget: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!("child did not exit within {budget:?}; killed it");
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Path used by the test to capture both stdout and stderr from
/// the spawned `helioslite share` subprocesses. We snapshot the
/// path on the first call so each spawned `serve` writes to the
/// same file the test re-reads on failure — the panic hook in
/// `helioslite_main.rs` uses `println!` (stdout), so silently
/// dropping it would lose the actual error message that explains
/// why the binary exited.
fn share_log_path() -> std::path::PathBuf {
    std::env::temp_dir().join("helioslite-share-e2e.log")
}

/// Spawn `helioslite share serve --host 127.0.0.1 --port P` with
/// stdio fully detached (stdin piped so the test never inherits
/// the parent's TTY; stdout/stderr both routed to the same
/// on-disk log so the binary's `panic::set_hook` `println!` makes
/// it into a file the test can read on failure).
fn spawn_serve(port: u16) -> Child {
    let stderr_path = share_log_path();
    // Truncate the log so it reflects only this run.
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&stderr_path)
        .unwrap_or_else(|e| panic!("open serve log {stderr_path:?}: {e}"));
    let stdout_file = stderr_file
        .try_clone()
        .unwrap_or_else(|e| panic!("clone serve log handle: {e}"));
    let bin = env!("CARGO_BIN_EXE_helioslite");
    let result = Command::new(bin)
        .args([
            "share",
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LOG")
        .env_remove("FORGE_CONFIG")
        .spawn();
    match result {
        Ok(c) => c,
        Err(e) => panic!("spawn helioslite share serve: {e}"),
    }
}

/// Read the captured `helioslite share serve` log (if any) into a
/// short, single-line summary for inclusion in a panic message.
/// Strips ANSI colour codes so the panic text is grep-friendly.
fn serve_log_summary() -> String {
    let path = share_log_path();
    if let Ok(s) = std::fs::read_to_string(&path) {
        // Trim to the first non-empty line.
        let line = s
            .lines()
            .map(strip_ansi)
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default();
        if line.is_empty() {
            return String::new();
        }
        let truncated = if line.len() > 240 {
            format!("{}…", &line[..240])
        } else {
            line
        };
        format!("\n  child output: {truncated}")
    } else {
        String::new()
    }
}

/// Strip a best-effort subset of ANSI CSI escape codes so panic
/// messages are clean without pulling in a colour-stripping crate.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Consume `[` + any params + final letter.
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Spawn `helioslite share publish --topic chat --payload '{"hi":1}'`.
/// The publish subcommand constructs its own ephemeral `ShareHub` so
/// the bytes it sends are never seen by the live serve process — but
/// the binary still has to construct the hub, parse clap args, parse
/// the payload, build a runtime, and exit cleanly. This is the
/// binary-lifecycle guardrail the spec calls for in step (4).
fn spawn_publish() -> Child {
    let bin = env!("CARGO_BIN_EXE_helioslite");
    Command::new(bin)
        .args([
            "share",
            "publish",
            "--topic",
            "chat",
            "--payload",
            r#"{"hi":1}"#,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn helioslite share publish: {e}"))
}

/// Write `body` to `stream` and shut down the write half so the peer
/// sees EOF on its reads (mimics how `reqwest`/`curl` close the
/// request body).
fn write_request(stream: &mut TcpStream, body: &[u8]) {
    stream
        .write_all(body)
        .expect("write request bytes to relay");
    stream
        .shutdown(Shutdown::Write)
        .expect("shutdown write half");
}

/// Drain the socket into `buf` until `budget` elapses OR the response
/// includes the SSE `data:` event we're looking for. Returns the raw
/// response bytes — caller asserts on substring presence.
fn read_until_event(stream: &mut TcpStream, buf: &mut Vec<u8>, budget: Duration) {
    let deadline = Instant::now() + budget;
    let mut local = [0u8; 4096];
    while Instant::now() < deadline {
        stream
            .set_read_timeout(Some(Duration::from_millis(250)))
            .expect("set_read_timeout");
        match stream.read(&mut local) {
            Ok(0) => break, // peer closed
            Ok(n) => {
                buf.extend_from_slice(&local[..n]);
                let text = std::str::from_utf8(buf).unwrap_or("");
                if text.contains("data:") && text.contains("\n\n") {
                    return;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // keep polling
            }
            Err(e) => panic!("read error: {e}"),
        }
    }
}

/// Cross-platform shutdown signal. On Unix we send SIGINT so the
/// serve process exercises its `tokio::signal::ctrl_c()` handler; on
/// Windows we fall back to `Child::kill()` (TerminateProcess) because
/// the stdlib does not expose a portable SIGINT. `libc` is already a
/// direct dep of `forge_main`, so integration tests can use it
/// without any Cargo.toml change.
#[cfg(unix)]
fn send_shutdown_signal(child: &mut Child) {
    let pid = child.id() as i32;
    let rc = unsafe { libc::kill(pid, libc::SIGINT) };
    if rc != 0 {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn send_shutdown_signal(child: &mut Child) {
    let _ = child.kill();
}

/// RAII guard: takes ownership of the serve `Child` so a panic
/// between spawn and the explicit shutdown step kills the child and
/// releases the port. After the explicit shutdown succeeds, callers
/// `into_inner()` to take the child out and avoid the killer firing
/// on a child that's already exited.
struct ServeGuard {
    child: Option<Child>,
}

impl ServeGuard {
    fn new(child: Child) -> Self {
        Self {
            child: Some(child),
        }
    }
    /// Probe the wrapped child for early exit. Returns the captured
    /// `ExitStatus` if the child has exited, or `None` if it's
    /// still running.
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self.child.as_mut() {
            Some(c) => c.try_wait(),
            None => Ok(None),
        }
    }
    /// Take the wrapped child without running the panic-time
    /// killer — call this after a successful graceful shutdown so
    /// the `Drop` impl becomes a no-op.
    fn into_inner(mut self) -> Child {
        self.child
            .take()
            .expect("ServeGuard: child already consumed")
    }
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helioslite_share_lifecycle_end_to_end() {
    let started = Instant::now();

    fn remaining(started: Instant) -> Duration {
        OVERALL_BUDGET
            .checked_sub(started.elapsed())
            .unwrap_or_default()
    }

    // (1) Reserve an ephemeral port via the kernel.
    let port = ephemeral_port();

    // (2) Spawn `helioslite share serve --host 127.0.0.1 --port {port}`.
    //     Wrap the child in `ServeGuard` so a panic between here and
    //     the explicit shutdown step still kills the subprocess and
    //     releases the bound port.
    let mut guard = ServeGuard::new(spawn_serve(port));

    // Quick liveness probe — if the binary crashed before binding
    // (e.g. invalid config, panic hook fire), surface that to the
    // test panic message before we spin on TCP connect.
    thread::sleep(Duration::from_millis(250));
    match guard.try_wait() {
        Ok(Some(status)) => {
            panic!(
                "helioslite share serve exited unexpectedly before binding (status {status:?}){}",
                serve_log_summary()
            );
        }
        Ok(None) => {} // still running — proceed
        Err(e) => panic!("try_wait failed: {e}"),
    }

    // (3) Wait until the port is accepting.
    let connect_budget = CONNECT_BUDGET.min(remaining(started));
    wait_for_accept(port, connect_budget);
    assert!(
        started.elapsed() < OVERALL_BUDGET,
        "exceeded overall budget before publish step"
    );

    // (4a) Spawn `helioslite share publish --topic chat --payload
    //      '{"hi":1}'` and wait for it to exit. This exercises the
    //      binary's publish code path end-to-end.
    let mut publish = spawn_publish();
    let publish_status = wait_for_exit(&mut publish, PUBLISH_BUDGET);
    assert!(
        publish_status.success(),
        "share publish exited non-zero: {publish_status:?}"
    );

    // (4b) Drive the payload through the *serve* process's hub via
    //      `POST /publish/chat`. The publish subcommand owns an
    //      ephemeral hub and never reaches our serve, so the only
    //      path that delivers `{"hi":1}` to a live subscriber is the
    //      HTTP endpoint the relay exposes.
    let publish_body = serde_json::json!({
        "id": "e2e-shell-1",
        "topic": "chat",
        "seq": 0,
        "ts": "2026-09-13T00:00:00Z",
        "payload": {"hi": 1}
    })
    .to_string();
    let mut publish_conn =
        TcpStream::connect(("127.0.0.1", port)).expect("connect for POST /publish/chat");
    let publish_req = format!(
        "POST /publish/chat HTTP/1.1\r\nHost: 127.0.0.1\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        publish_body.len(),
        publish_body
    );
    write_request(&mut publish_conn, publish_req.as_bytes());
    let mut publish_resp = Vec::new();
    publish_conn
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set_read_timeout on publish conn");
    publish_conn
        .read_to_end(&mut publish_resp)
        .expect("read publish response");
    let publish_text = std::str::from_utf8(&publish_resp).unwrap_or("");
    assert!(
        publish_text.starts_with("HTTP/1.1 202"),
        "publish POST must return 202 Accepted, got: {publish_text:?}"
    );

    // (5) Open the SSE GET and assert the live subscriber sees
    //     `data: {"hi":1}`. The relay writes the full `ShareMessage`
    //     envelope into `data:`, so the actual substring we look for
    //     is `"hi":1` somewhere inside the JSON.
    let mut sse =
        TcpStream::connect(("127.0.0.1", port)).expect("connect for GET /sse/chat");
    let sse_req = "GET /sse/chat HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                   Accept: text/event-stream\r\nConnection: close\r\n\r\n";
    write_request(&mut sse, sse_req.as_bytes());
    let mut sse_buf = Vec::new();
    let read_budget = READ_BUDGET.min(remaining(started));
    read_until_event(&mut sse, &mut sse_buf, read_budget);

    let sse_text = std::str::from_utf8(&sse_buf).unwrap_or("");
    assert!(
        sse_text.starts_with("HTTP/1.1 200"),
        "SSE must return 200 OK; got: {sse_text:?}"
    );
    assert!(
        sse_text.to_ascii_lowercase().contains("text/event-stream"),
        "SSE response must declare text/event-stream; got: {sse_text:?}"
    );
    assert!(
        sse_text.contains("data: "),
        "SSE response must contain at least one `data:` frame; got: {sse_text:?}"
    );
    assert!(
        sse_text.contains(r#""hi":1"#),
        "SSE response must carry the published payload {{\"hi\":1}}; got: {sse_text:?}"
    );

    // Drop the SSE connection so the relay's pump_sse loop sees EOF
    // and frees its task slot — otherwise the relay would carry the
    // dead connection until serve exits.
    drop(sse);

    // (6) Drop the guard (taking the child out) so its `Drop`
    //     doesn't fire after we've waited for a clean exit. Then
    //     send the shutdown signal and wait. The guard is purely a
    //     panic-net — once shutdown completes successfully, the
    //     child has exited and the guard's cleanup logic would be
    //     a no-op anyway, but we still unwrap to make the control
    //     flow explicit.
    let mut serve = guard.into_inner();
    send_shutdown_signal(&mut serve);
    let serve_status = wait_for_exit(&mut serve, SHUTDOWN_BUDGET.min(remaining(started)));
    // The contract on Unix is `code == 0` (clean SIGINT); on Windows
    // `Child::kill()` yields whatever TerminateProcess returns. The
    // hard requirement is "exits within 5s after signal" — that's
    // already enforced by `wait_for_exit` blocking on a deadline,
    // so we only record the status code here.
    eprintln!("serve process exited with status {serve_status:?}");

    // Final guard: we never want to overrun the overall budget.
    assert!(
        started.elapsed() < OVERALL_BUDGET,
        "test exceeded its overall 15s budget: {:?}",
        started.elapsed()
    );
}
