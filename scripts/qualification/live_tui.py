"""Synthetic provider and process-scoped PTY proof for the live TUI owner."""
import http.server
import json
import os
from pathlib import Path
import pty
import socket
import subprocess
import threading
import time
import uuid

from live_control import wait_for


class Provider(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for delta, finish in [({"role": "assistant", "content": "SYNTHETIC_TUI_WITNESS\n"}, None), ({}, "stop")]:
            chunk = {"id": "synthetic", "object": "chat.completion.chunk", "created": 1,
                     "model": "synthetic", "choices": [{"index": 0, "delta": delta,
                                                          "finish_reason": finish}]}
            self.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


def qualify_tui(binary, root, session):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config = root / "state" / "config" / ".helioslite.toml"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(f'''[session]
provider_id = "synthetic"
model_id = "synthetic"
[updates]
frequency = "never"
auto_update = false
[[providers]]
id = "synthetic"
api_key_var = "SYNTHETIC_KEY"
url = "http://127.0.0.1:{server.server_port}/v1/chat/completions"
response_type = "OpenAI"
[[providers.models]]
id = "synthetic"
name = "Synthetic local witness"
context_length = 32768
tools_supported = true
''')
    env = dict(os.environ, HELIOSLITE_HOME=str(root / "state"), SYNTHETIC_KEY="synthetic",
               TERM="xterm-256color", CI="true")
    master, slave = pty.openpty()
    process = subprocess.Popen([binary, "--directory", str(root), "--conversation-id", session],
                               stdin=slave, stdout=slave, stderr=slave, env=env, close_fds=True)
    os.close(slave)
    transcript = bytearray()

    def drain():
        while True:
            try:
                chunk = os.read(master, 65536)
                if not chunk:
                    break
                transcript.extend(chunk)
            except OSError:
                break

    reader = threading.Thread(target=drain, daemon=True)
    reader.start()
    path = root / "state" / "live" / f"{session}.sock"

    def ipc(method="snapshot", **fields):
        message = {"version": 1, "session_id": session, "runtime_id": None,
                   "method": method, **fields}
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5)
            client.connect(str(path))
            client.sendall(json.dumps(message).encode() + b"\n")
            return json.loads(client.makefile("rb").readline())

    try:
        snapshot = wait_for(lambda: ipc(after=None), timeout=45)["result"]
        runtime = snapshot["runtime_id"]
        event = {"id": str(uuid.uuid4()), "timestamp": "2026-09-29T00:00:00Z",
                 "value": {"Text": "Reply with the witness."}, "attachments": [],
                 "additional_context": None}
        command = str(uuid.uuid4())
        accepted = ipc("prompt", runtime_id=runtime, command_id=command, event=event)
        turn = accepted["result"]["turn_id"]
        event["id"] = str(uuid.uuid4())
        assert ipc("prompt", runtime_id=runtime, command_id=command, event=event)["result"]["turn_id"] == turn
        event["value"] = {"Text": "different prompt"}
        assert ipc("prompt", runtime_id=runtime, command_id=command, event=event)["error"] == "command_id_conflict"

        def finished():
            current = ipc(after=0)["result"]
            events = [event for event in current["events"] if event["turn_id"] == turn]
            terminal = [event for event in events if event["payload"]["kind"] == "turn_finished"]
            assert terminal, "turn has not finished"
            assert terminal[-1]["payload"]["status"] == "completed", terminal[-1]
            assert all(event["session_id"] == session and event["runtime_id"] == runtime for event in events)
            return events

        events = wait_for(finished, timeout=45)
        assert any(event["payload"]["kind"] == "turn_started" for event in events)
        wait_for(lambda: witness(transcript), timeout=10)
        assert process.poll() is None, "TUI exited after remote turn"
        print(json.dumps({"tui_session": session, "runtime": runtime, "turn": turn,
                          "checks": ["same_live_tui_ipc", "remote_prompt_completed", "terminal_witness",
                                     "prompt_deduplication", "prompt_conflict", "event_correlation"]}))
    except Exception:
        print(transcript.decode(errors="replace"))
        raise
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        os.close(master)
        server.shutdown()
        server.server_close()


def witness(transcript):
    assert b"SYNTHETIC_TUI_WITNESS" in transcript
    return True
