"""Run an isolated real actor and localhost provider for a bridge integration witness."""

import argparse
import http.server
import json
import os
import pty
import signal
import socket
import subprocess
import tempfile
import threading
import uuid
from pathlib import Path

from live_control import wait_for
from live_provider import Provider


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    root = Path(tempfile.mkdtemp(prefix="hl-actor-", dir="/tmp")).resolve()
    state = root / "state"
    config = state / "config" / ".helioslite.toml"
    config.parent.mkdir(parents=True)
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    server.witness_root = root
    threading.Thread(target=server.serve_forever, daemon=True).start()
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
    (state / "permissions.yaml").write_text(
        "policies:\n  - permission: confirm\n    rule:\n      command: '*'\n"
    )
    env = dict(
        os.environ,
        HELIOSLITE_HOME=str(state),
        SYNTHETIC_KEY="synthetic",
        CI="true",
        TERM="xterm-256color",
        TMPDIR="/tmp",
        FORGE_INTERACTION_TTL_SECONDS="300",
    )
    session = str(uuid.uuid4())
    stop = threading.Event()
    signal.signal(signal.SIGTERM, lambda *_: stop.set())
    signal.signal(signal.SIGINT, lambda *_: stop.set())
    master = None
    with (root / "owner.log").open("wb") as log:
        owner = subprocess.Popen(
            [str(binary), "--directory", str(root), "live-host", session, "--create"],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
        )
        try:

            def snapshot():
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(5)
                    client.connect(str(state / "live" / f"{session}.sock"))
                    client.sendall(
                        (
                            json.dumps(
                                {
                                    "version": 1,
                                    "session_id": session,
                                    "runtime_id": None,
                                    "method": "snapshot",
                                    "after": None,
                                }
                            )
                            + "\n"
                        ).encode()
                    )
                    return json.loads(client.makefile("rb").readline())["result"]

            wait_for(snapshot, timeout=45)
            owner.send_signal(signal.SIGINT)
            owner.wait(timeout=10)
            master, slave = pty.openpty()
            owner = subprocess.Popen(
                [str(binary), "--directory", str(root), "--conversation-id", session],
                env=env,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                close_fds=True,
            )
            os.close(slave)

            def drain():
                while True:
                    try:
                        chunk = os.read(master, 65536)
                        if not chunk:
                            break
                        log.write(chunk)
                        log.flush()
                    except (OSError, ValueError):
                        break

            threading.Thread(target=drain, daemon=True).start()
            current = wait_for(snapshot, timeout=45)
            receipt = {
                "root": str(root),
                "state": str(state),
                "sessionId": session,
                "runtimeId": current["runtime_id"],
                "ownerPid": owner.pid,
                "binary": str(binary),
                "acpArguments": ["--directory", str(root), "acp"],
                "environment": {
                    key: env[key]
                    for key in ["HELIOSLITE_HOME", "SYNTHETIC_KEY", "TMPDIR"]
                },
                "questionPrompt": "SYNTHETIC_FOLLOWUP_acp",
                "permissionPrompt": "SYNTHETIC_PERMISSION_acp",
                "permissionWitness": str(root / "permission-acp"),
            }
            (root / "fixture.json").write_text(json.dumps(receipt, indent=2) + "\n")
            print(json.dumps(receipt), flush=True)
            while not stop.wait(0.25):
                if owner.poll() is not None:
                    raise RuntimeError(
                        f"isolated owner exited: {owner.returncode}; {root / 'owner.log'}"
                    )
        finally:
            if owner.poll() is None:
                if master is None:
                    owner.send_signal(signal.SIGINT)
                else:
                    os.write(master, b":exit\n")
                try:
                    owner.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    owner.kill()
                    owner.wait()
            if master is not None:
                os.close(master)
            server.shutdown()
            server.server_close()


if __name__ == "__main__":
    main()
