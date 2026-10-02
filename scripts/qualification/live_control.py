"""Isolated process qualification for the live owner and ACP attachment."""

import json
import os
import selectors
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path


def wait_for(action, timeout=20):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            return action()
        except (OSError, AssertionError) as error:
            last = error
            time.sleep(0.05)
    raise AssertionError(f"deadline exceeded: {last}")


def qualify(binary, root):
    env = dict(os.environ, HELIOSLITE_HOME=str(root / "state"), CI="true")
    session = str(uuid.uuid4())
    path = root / "state" / "live" / f"{session}.sock"
    command = [binary, "--directory", str(root), "live-host", session, "--create"]
    owner = subprocess.Popen(
        command,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    bridge = None

    def ipc(method="snapshot", **fields):
        message = {
            "version": 1,
            "session_id": session,
            "runtime_id": None,
            "method": method,
            **fields,
        }
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5)
            client.connect(str(path))
            client.sendall(json.dumps(message).encode() + b"\n")
            result = json.loads(client.makefile("rb").readline())
        return result

    try:
        snapshot = wait_for(lambda: ipc(after=None))["result"]
        runtime = snapshot["runtime_id"]
        assert runtime != session
        controller = str(uuid.uuid4())
        assert snapshot["controlled"] is False
        assert (
            ipc("cancel", runtime_id=runtime, turn_id=str(uuid.uuid4()))["error"]
            == "controller_required"
        )
        assert (
            ipc("claim_control", runtime_id=runtime, controller_id=controller)[
                "result"
            ]["controlled"]
            is True
        )
        assert (
            ipc("claim_control", runtime_id=runtime, controller_id=str(uuid.uuid4()))[
                "error"
            ]
            == "controller_conflict"
        )
        assert (
            ipc("release_control", runtime_id=runtime, controller_id=str(uuid.uuid4()))[
                "error"
            ]
            == "controller_lease_lost"
        )
        assert ipc(after=None, controller_id=controller)["result"]["controlled"] is True
        assert ipc(after=None)["result"]["controlled"] is False
        assert (
            ipc("release_control", runtime_id=runtime, controller_id=controller)[
                "result"
            ]["controlled"]
            is False
        )
        assert path.stat().st_mode & 0o777 == 0o600
        assert path.parent.stat().st_mode & 0o777 == 0o700
        duplicate = subprocess.run(
            command, env=env, capture_output=True, timeout=15, check=False
        )
        assert duplicate.returncode != 0, "second owner acquired the same session"
        assert ipc(after=None)["result"]["runtime_id"] == runtime
        assert ipc("cancel", turn_id=str(uuid.uuid4()))["error"] == "runtime_mismatch"
        assert ipc(after=2**64 - 1)["result"]["resync_required"] is True
        bridge = subprocess.Popen(
            [binary, "--directory", str(root), "acp"],
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        pending = bytearray()

        def rpc(number, method, params):
            request = {
                "jsonrpc": "2.0",
                "id": number,
                "method": method,
                "params": params,
            }
            bridge.stdin.write(json.dumps(request).encode() + b"\n")
            bridge.stdin.flush()
            deadline = time.monotonic() + 15
            with selectors.DefaultSelector() as selector:
                selector.register(bridge.stdout, selectors.EVENT_READ)
                while time.monotonic() < deadline:
                    if not selector.select(0.1):
                        continue
                    chunk = os.read(bridge.stdout.fileno(), 65536)
                    assert chunk, "ACP unexpectedly disconnected"
                    pending.extend(chunk)
                    while b"\n" in pending:
                        line, _, remaining = pending.partition(b"\n")
                        pending[:] = remaining
                        response = json.loads(line)
                        if response.get("id") == number:
                            return response
            raise AssertionError(f"ACP request timed out: {method}")

        initialized = rpc(
            1, "initialize", {"protocolVersion": 1, "clientCapabilities": {}}
        )
        assert initialized["result"]["protocolVersion"] == 1
        assert (
            initialized["result"]["agentCapabilities"]["_meta"][
                "io.phenotype/interactionController"
            ]
            is True
        )
        loaded = rpc(
            2,
            "session/load",
            {"sessionId": session, "cwd": str(root), "mcpServers": []},
        )
        assert loaded["result"]["_meta"]["io.phenotype/runtimeId"] == runtime
        assert loaded["result"]["_meta"]["io.phenotype/state"] == "attached"
        assert loaded["result"]["_meta"]["io.phenotype/interactionController"] is False
        passive = rpc(
            3,
            "session/prompt",
            {
                "sessionId": session,
                "prompt": [{"type": "text", "text": "must not run"}],
            },
        )
        assert passive["error"]["message"] == "controller_required"
        granted = rpc(
            4,
            "session/load",
            {
                "sessionId": session,
                "cwd": str(root),
                "mcpServers": [],
                "_meta": {"io.phenotype/interactionController": True},
            },
        )
        assert granted["result"]["_meta"]["io.phenotype/interactionController"] is True
        assert (
            ipc("claim_control", runtime_id=runtime, controller_id=controller)["error"]
            == "controller_conflict"
        )
        bridge.stdin.close()
        assert bridge.wait(timeout=10) == 0
        bridge = None
        assert ipc(after=None)["result"]["runtime_id"] == runtime
        assert (
            ipc("claim_control", runtime_id=runtime, controller_id=controller)[
                "result"
            ]["controlled"]
            is True
        )
        print(
            json.dumps(
                {
                    "session": session,
                    "runtime": runtime,
                    "checks": [
                        "private_socket",
                        "exclusive_owner",
                        "runtime_identity",
                        "future_cursor_resync",
                        "acp_initialize",
                        "same_owner_attach",
                        "disconnect_preserves_owner",
                        "passive_cannot_control",
                        "exclusive_controller",
                        "explicit_acp_grant",
                        "disconnect_releases_control",
                    ],
                }
            )
        )
        return session
    finally:
        if bridge is not None:
            bridge.terminate()
            bridge.wait(timeout=10)
        owner.send_signal(signal.SIGINT)
        try:
            owner.wait(timeout=10)
        except subprocess.TimeoutExpired:
            owner.kill()
            owner.wait()
        if owner.returncode not in (0, -signal.SIGINT):
            print(owner.stderr.read().decode(errors="replace"), file=sys.stderr)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory(prefix="hl-control-") as directory:
        binary = str(Path(sys.argv[1]).resolve())
        root = Path(directory).resolve()
        session = qualify(binary, root)
        from live_tui import qualify_tui

        qualify_tui(binary, root, session)
