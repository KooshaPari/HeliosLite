"""Synthetic provider and process-scoped PTY proof for the live TUI owner."""

import http.server
import json
import os
import pty
import socket
import subprocess
import threading
import uuid

from live_control import wait_for
from live_provider import Provider


def qualify_tui(binary, root, session):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    server.witness_root = root
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config = root / "state" / "config" / ".helioslite.toml"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(f"""tool_supported = true
restricted = true
[session]
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
""")
    (root / "state" / "permissions.yaml").write_text(
        "policies:\n  - permission: confirm\n    rule:\n      command: '*'\n"
    )
    env = dict(
        os.environ,
        HELIOSLITE_HOME=str(root / "state"),
        SYNTHETIC_KEY="synthetic",
        TERM="xterm-256color",
        CI="true",
        FORGE_INTERACTION_TTL_SECONDS="5",
    )
    master, slave = pty.openpty()
    process = subprocess.Popen(
        [binary, "--directory", str(root), "--conversation-id", session],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        close_fds=True,
    )
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
    controller = str(uuid.uuid4())

    def ipc(method="snapshot", **fields):
        message = {
            "version": 1,
            "session_id": session,
            "runtime_id": None,
            "controller_id": controller,
            "method": method,
            **fields,
        }
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5)
            client.connect(str(path))
            client.sendall(json.dumps(message).encode() + b"\n")
            return json.loads(client.makefile("rb").readline())

    try:
        snapshot = wait_for(lambda: ipc(after=None), timeout=45)["result"]
        runtime = snapshot["runtime_id"]
        assert snapshot["controlled"] is False
        assert ipc("claim_control", runtime_id=runtime)["result"]["controlled"] is True
        event = {
            "id": str(uuid.uuid4()),
            "timestamp": "2026-09-29T00:00:00Z",
            "value": {"Text": "Reply with the witness."},
            "attachments": [],
            "additional_context": None,
        }
        command = str(uuid.uuid4())
        accepted = ipc("prompt", runtime_id=runtime, command_id=command, event=event)
        turn = accepted["result"]["turn_id"]
        event["id"] = str(uuid.uuid4())
        assert (
            ipc("prompt", runtime_id=runtime, command_id=command, event=event)[
                "result"
            ]["turn_id"]
            == turn
        )
        event["value"] = {"Text": "different prompt"}
        assert (
            ipc("prompt", runtime_id=runtime, command_id=command, event=event)["error"]
            == "command_id_conflict"
        )

        def finished():
            current = ipc(after=0)["result"]
            events = [event for event in current["events"] if event["turn_id"] == turn]
            terminal = [
                event for event in events if event["payload"]["kind"] == "turn_finished"
            ]
            assert terminal, "turn has not finished"
            assert terminal[-1]["payload"]["status"] == "completed", terminal[-1]
            assert all(
                event["session_id"] == session and event["runtime_id"] == runtime
                for event in events
            )
            return events

        events = wait_for(finished, timeout=45)
        assert any(event["payload"]["kind"] == "turn_started" for event in events)
        wait_for(lambda: witness(transcript), timeout=10)
        assert process.poll() is None, "TUI exited after remote turn"
        qualify_followups(ipc, runtime, session, master, transcript)
        from live_permissions import qualify_permissions

        qualify_permissions(ipc, runtime, root)
        from live_acp import qualify_acp

        qualify_acp(ipc, runtime, session, root, binary, env)
        from live_replay import qualify_replay

        qualify_replay(ipc, runtime, session, root, binary, env, command, turn)
        from live_shutdown import qualify_shutdown

        qualify_shutdown(ipc, runtime, session, root, binary, process, master, env)
        print(
            json.dumps(
                {
                    "tui_session": session,
                    "runtime": runtime,
                    "turn": turn,
                    "checks": [
                        "same_live_tui_ipc",
                        "remote_prompt_completed",
                        "terminal_witness",
                        "prompt_deduplication",
                        "prompt_conflict",
                        "event_correlation",
                        "held_followup",
                        "local_response",
                        "remote_response",
                        "same_tool_result",
                        "pending_reconnect",
                        "wrong_identity",
                        "invalid_answer",
                        "duplicate_response",
                        "turn_cancel",
                        "request_expiry",
                    ],
                }
            )
        )
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


def qualify_followups(ipc, runtime, session, master, transcript):
    for mode in ["local", "remote", "reconnect", "cancel", "expire"]:
        event = {
            "id": str(uuid.uuid4()),
            "timestamp": "2026-09-29T00:00:00Z",
            "value": {"Text": f"SYNTHETIC_FOLLOWUP_{mode}"},
            "attachments": [],
            "additional_context": None,
        }
        turn = ipc(
            "prompt", runtime_id=runtime, command_id=str(uuid.uuid4()), event=event
        )["result"]["turn_id"]

        def held(turn=turn):
            requests = [
                request
                for request in ipc(after=None)["result"]["pending"]
                if request["turn_id"] == turn
            ]
            assert len(requests) == 1
            return requests[0]

        request = wait_for(held, timeout=20)
        assert request["session_id"] == session and request["runtime_id"] == runtime
        assert request["tool_call"]["call_id"] == "synthetic-followup"
        response = {
            key: request[key]
            for key in ["request_id", "session_id", "runtime_id", "turn_id"]
        }
        response["answer"] = {"kind": "text", "value": "SYNTHETIC_ANSWER"}
        if mode == "local":
            for field in ["session_id", "runtime_id", "turn_id"]:
                invalid = dict(response, **{field: str(uuid.uuid4())})
                assert "error" in ipc("respond", runtime_id=runtime, response=invalid)
            invalid = dict(response, answer={"kind": "choices", "value": [99]})
            assert "error" in ipc("respond", runtime_id=runtime, response=invalid)
            os.write(
                master, f"/respond {request['request_id']} SYNTHETIC_ANSWER\n".encode()
            )
        elif mode == "remote":
            assert "result" in ipc("respond", runtime_id=runtime, response=response)
        elif mode == "reconnect":
            assert "result" in ipc("release_control", runtime_id=runtime)
            next_controller = str(uuid.uuid4())
            assert "result" in ipc(
                "claim_control", runtime_id=runtime, controller_id=next_controller
            )
            recovered = ipc(after=None, controller_id=next_controller)["result"][
                "pending"
            ]
            assert any(
                item["request_id"] == request["request_id"] for item in recovered
            )
            assert "result" in ipc(
                "respond",
                runtime_id=runtime,
                controller_id=next_controller,
                response=response,
            )
            assert "result" in ipc(
                "release_control", runtime_id=runtime, controller_id=next_controller
            )
            assert "result" in ipc("claim_control", runtime_id=runtime)
        elif mode == "cancel":
            assert "result" in ipc("cancel", runtime_id=runtime, turn_id=turn)

        def settled(request=request, turn=turn, mode=mode):
            snapshot = ipc(after=0)["result"]
            assert not any(
                item["request_id"] == request["request_id"]
                for item in snapshot["pending"]
            )
            finished = [
                item
                for item in snapshot["events"]
                if item["turn_id"] == turn
                and item["payload"]["kind"] == "turn_finished"
            ]
            assert finished
            expected = "cancelled" if mode == "cancel" else "completed"
            assert finished[-1]["payload"]["status"] == expected, finished[-1]
            if mode in {"local", "remote", "reconnect"}:
                original_turn = [
                    item for item in snapshot["events"] if item["turn_id"] == turn
                ]
                assert "SYNTHETIC_ANSWER" in json.dumps(original_turn), (
                    "answer did not reach original tool result"
                )
            return True

        wait_for(settled, timeout=15)
        assert "error" in ipc("respond", runtime_id=runtime, response=response)
        assert b"SYNTHETIC_HELD_QUESTION" in transcript
