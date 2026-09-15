//! End-to-end shell test for the `helioslite share` binary lifecycle.
//!
//! Spawns the *real* `helioslite` binary (resolved at compile time from
//! `CARGO_BIN_EXE_<name>`) and asserts:
//!
//! 1. `helioslite share serve --host 127.0.0.1 --port P` binds the
//!    requested port within a few seconds (this is what the
//!    `run_serve_blocking` runtime-thread fix unblocks — without it the
//!    binary panics with "Cannot start a runtime from within a runtime"
//!    and never binds.
//! 2. `helioslite share publish --topic T --payload '{"hi":1}'` launches
//!    and exits cleanly.
//! 3. The serve process accepts a plain TCP connection and answers a
//!    `GET /sse/<topic>` with `200 OK` + `text/event-stream` — proving the
//!    relay's request dispatcher runs inside the real process.
//! 4. Terminating the serve process makes it exit.
//!
//! The live SSE *data-frame* round-trip is intentionally NOT asserted here:
//! the broadcast subscriber-vs-publish ordering is inherently racy on
//! Windows CI and is already covered deterministically by the crate's own
//! `transport::sse` unit tests. This test's job is the binary lifecycle +
//! request-dispatch, which is what the runtime-thread fix is about.
//!
//! Run with: `cargo test -p forge_main --test share_bin_e2e`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const CONNECT_BUDGET: Duration = Duration::from_secs(5);
const PUBLISH_BUDGET: Duration = Duration::from_secs(10);
const EXIT_BUDGET: Duration = Duration::from_secs(5);

/// Reserve an ephemeral port via the kernel, then close it so `serve` can
/// bind it. There is an unavoidable tiny race (another process could grab
/// it between close and bind) but it is vanishingly rare on CI.
///
/// We pick a real port rather than `--port 0` because the test needs to
/// know the exact port to connect to.
fn local_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

fn spawn_serve(port: u16) -> Child {
    // Provide /dev/null (or NUL on Windows) as stdin. The binary reads
    // stdin when it is a pipe (to detect interactive vs scripted input);
    // an immediate EOF/NUL means "not interactive", so `serve` proceeds
    // to bind. Piping an *open-but-writing* stdin would block it.
    let stdin = {
        #[cfg(unix)]
        let f = std::fs::File::open("/dev/null").unwrap();
        #[cfg(windows)]
        let f = std::fs::File::open("NUL").unwrap();
        Stdio::from(f)
    };
    Command::new(env!("CARGO_BIN_EXE_helioslite"))
        .args([
            "share",
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdin(stdin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("FORGE_CONFIG")
        .spawn()
        .expect("spawn helioslite share serve")
}

fn spawn_publish() -> Child {
    Command::new(env!("CARGO_BIN_EXE_helioslite"))
        .args([
            "share",
            "publish",
            "--topic",
            "chat",
            "--payload",
            r#"{"hi":1}"#,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("FORGE_CONFIG")
        .spawn()
        .expect("spawn helioslite share publish")
}

/// Poll TCP connect until the port accepts or the budget elapses.
fn wait_for_accept(port: u16, budget: Duration) {
    let deadline = Instant::now() + budget;
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(_) => return,
            Err(_) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("port {port} never accepted within {budget:?}: {e}"),
        }
    }
}

fn wait_for_exit(child: &mut Child, budget: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + budget;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("child did not exit within {budget:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Send an HTTP request on a connected stream and read the response
/// **headers** (up to and including the blank line, with a byte cap).
///
/// We deliberately do NOT read to EOF: an SSE upstream keeps the stream
/// open (it never sends EOF/0 bytes), so a `read to EOF` would block
/// forever. The assertions only need the status line + `Content-Type`, so
/// headers are sufficient.
fn http_request_head(stream: &mut TcpStream, request: &str) -> Vec<u8> {
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush request");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 512];
    // Stop at the end-of-headers marker or a cap (no EOF read).
    while buf.len() < 16 * 1024 {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
    buf
}

struct ServeGuard {
    child: Option<Child>,
}

impl ServeGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Fully synchronous test (no awaits): plain `#[test]` avoids creating a
/// runtime and keeps the child-process lifecycle deterministic.
#[test]
fn helioslite_share_serve_binds_and_dispatches() {
    let port = local_port();

    // Spawn serve, wrapped so a panic still reaps it.
    let mut guard = ServeGuard::new(spawn_serve(port));

    // Quick liveness probe — surface an early crash before we spin.
    thread::sleep(Duration::from_millis(300));
    if let Ok(Some(status)) = guard.child.as_mut().unwrap().try_wait() {
        panic!("share serve exited before binding: {status:?}");
    }

    // (1) Serve binds and accepts a connection.
    wait_for_accept(port, CONNECT_BUDGET);

    // (2) Publish subcommand launches and exits cleanly.
    let mut publish = spawn_publish();
    let publish_status = wait_for_exit(&mut publish, PUBLISH_BUDGET);
    assert!(
        publish_status.success(),
        "share publish exited non-zero: {publish_status:?}"
    );

    // (3) The relay dispatches an HTTP request inside the real process.
    //     Sending `GET /sse/chat` must yield `200 OK` + event-stream CT,
    //     proving the accept loop + router run correctly.
    let mut conn = TcpStream::connect(("127.0.0.1", port)).expect("connect for GET");
    let req = "GET /sse/chat HTTP/1.1\r\nHost: 127.0.0.1\r\n\
               Accept: text/event-stream\r\nConnection: close\r\n\r\n";
    let resp = http_request_head(&mut conn, req);
    // `http_request_head` reads only the response headers (SSE upstreams
    // stay open, so we never read to EOF). Convert to a lowercase string.
    // Response headers are ASCII, so `from_utf8` (not the disallowed
    // `from_utf8_lossy`) is safe; fall back to empty on any non-UTF8.
    let resp_str = std::str::from_utf8(&resp).unwrap_or_default();
    let head = resp_str.lines().next().unwrap_or_default();
    assert!(
        head.starts_with("HTTP/1.1 200") || head.starts_with("HTTP/1.0 200"),
        "expected 200 status in response head, got: {head}"
    );
    let resp_lower = resp_str.to_ascii_lowercase();
    assert!(
        resp_lower.contains("text/event-stream"),
        "expected text/event-stream content type in response"
    );

    // (4) Terminate serve and confirm it exits.
    let mut child = guard.child.take().unwrap();
    let _ = child.kill();
    let status = wait_for_exit(&mut child, EXIT_BUDGET);
    // The child may or may not report a "clean" status on kill; the
    // requirement is that it actually exits, releasing the port.
    let _ = status;

    // Port should now be released — a rebind succeeds.
    let _rebind = TcpListener::bind(("127.0.0.1", port)).expect("port released after serve exit");
}
