"""Graceful TUI exit preserves its session before another owner resumes it."""

import json
import os
import signal
import subprocess
import uuid

from live_control import wait_for


def qualify_shutdown(ipc, runtime, session, root, binary, process, master, env):
    marker = "SYNTHETIC_FOLLOWUP_shutdown"
    event = {
        "id": str(uuid.uuid4()),
        "timestamp": "2026-09-29T00:00:00Z",
        "value": {"Text": marker},
        "attachments": [],
        "additional_context": None,
    }
    turn = ipc("prompt", runtime_id=runtime, command_id=str(uuid.uuid4()), event=event)[
        "result"
    ]["turn_id"]

    def held():
        assert any(
            item["turn_id"] == turn for item in ipc(after=None)["result"]["pending"]
        )
        return True

    wait_for(held, timeout=20)
    os.write(master, b":exit\n")
    assert process.wait(timeout=15) == 0
    resumed = subprocess.Popen(
        [binary, "--directory", str(root), "live-host", session],
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        snapshot = wait_for(lambda: ipc(after=None), timeout=15)["result"]
        assert snapshot["runtime_id"] != runtime
        assert snapshot["session_id"] == session
        assert not snapshot["pending"]
        assert marker in json.dumps(snapshot["conversation"])
        print(
            json.dumps(
                {
                    "shutdown_checks": [
                        "held_turn_cancelled_on_exit",
                        "owner_joined",
                        "session_resumed",
                        "prompt_preserved",
                    ]
                }
            )
        )
    finally:
        resumed.send_signal(signal.SIGINT)
        try:
            resumed.wait(timeout=10)
        except subprocess.TimeoutExpired:
            resumed.kill()
            resumed.wait()
