"""Original held questions and permissions settle through standard ACP responses."""

import json
import os
import selectors
import subprocess
import time


class Client:
    def __init__(self, binary, root, env):
        self.process = subprocess.Popen(
            [binary, "--directory", str(root), "acp"],
            cwd=root,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        self.buffer = bytearray()

    def send(self, message):
        self.process.stdin.write(json.dumps(message).encode() + b"\n")
        self.process.stdin.flush()

    def wait(self, predicate, timeout=10):
        deadline = time.monotonic() + timeout
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdout, selectors.EVENT_READ)
            while time.monotonic() < deadline:
                while b"\n" in self.buffer:
                    line, _, remainder = self.buffer.partition(b"\n")
                    self.buffer[:] = remainder
                    message = json.loads(line)
                    if predicate(message):
                        return message
                if selector.select(0.1):
                    chunk = os.read(self.process.stdout.fileno(), 65536)
                    assert chunk, "ACP closed before settling the original request"
                    self.buffer.extend(chunk)
        raise AssertionError("ACP expected message timed out")

    def request(self, number, method, params):
        self.send({"jsonrpc": "2.0", "id": number, "method": method, "params": params})
        response = self.wait(lambda message: message.get("id") == number)
        assert "result" in response, response
        return response["result"]

    def close(self):
        try:
            self.process.stdin.close()
        except BrokenPipeError:
            pass
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        return self.process.returncode


def qualify_acp(ipc, runtime, session, root, binary, env):
    assert "result" in ipc("release_control", runtime_id=runtime)
    client = Client(binary, root, env)
    try:
        client.request(
            1,
            "initialize",
            {"protocolVersion": 1, "clientCapabilities": {"elicitation": {"form": {}}}},
        )
        granted = client.request(
            2,
            "session/load",
            {
                "sessionId": session,
                "cwd": str(root),
                "mcpServers": [],
                "_meta": {"io.phenotype/interactionController": True},
            },
        )
        assert granted["_meta"]["io.phenotype/interactionController"] is True
        assert granted["_meta"]["io.phenotype/runtimeId"] == runtime
        for number, text, method in [
            (3, "SYNTHETIC_FOLLOWUP_acp", "elicitation/create"),
            (4, "SYNTHETIC_PERMISSION_acp", "session/request_permission"),
        ]:
            client.send(
                {
                    "jsonrpc": "2.0",
                    "id": number,
                    "method": "session/prompt",
                    "params": {
                        "sessionId": session,
                        "prompt": [{"type": "text", "text": text}],
                    },
                }
            )
            question = client.wait(
                lambda message, method=method: message.get("method") == method
            )
            metadata = question["params"]["_meta"]
            assert metadata["io.phenotype/sessionId"] == session
            assert metadata["io.phenotype/runtimeId"] == runtime
            original = next(
                item
                for item in ipc(after=None)["result"]["pending"]
                if item["request_id"] == metadata["io.phenotype/requestId"]
            )
            assert original["turn_id"] == metadata["io.phenotype/turnId"]
            if method == "elicitation/create":
                result = {
                    "action": "accept",
                    "content": {"answer": "ACP_ORIGINAL_ANSWER"},
                }
            else:
                assert not (root / "permission-acp").exists()
                assert (
                    question["params"]["toolCall"]["toolCallId"]
                    == "synthetic-permission-acp"
                )
                assert [
                    option["optionId"] for option in question["params"]["options"]
                ] == ["0", "1", "2"]
                result = {"outcome": {"outcome": "selected", "optionId": "0"}}
            response = {"jsonrpc": "2.0", "id": question["id"], "result": result}
            client.send(response)
            ended = client.wait(
                lambda message, number=number: message.get("id") == number
            )
            assert ended.get("result", {}).get("stopReason") == "end_turn", ended
            snapshot = ipc(after=0)["result"]
            assert not any(
                item["request_id"] == original["request_id"]
                for item in snapshot["pending"]
            )
            if method == "elicitation/create":
                events = [
                    item
                    for item in snapshot["events"]
                    if item["turn_id"] == original["turn_id"]
                ]
                assert "ACP_ORIGINAL_ANSWER" in json.dumps(events)
            else:
                assert (root / "permission-acp").read_text() == "approved"
                client.send(response)
                client.request(5, "session/list", {"cwd": str(root)})
                assert (root / "permission-acp").read_text() == "approved"
        print(
            json.dumps(
                {
                    "acp_checks": [
                        "same_tui_owner",
                        "explicit_controller",
                        "form_negotiation",
                        "original_question_response",
                        "original_permission_response",
                        "exact_tool_id",
                        "duplicate_response_ignored",
                    ]
                }
            )
        )
    finally:
        status = client.close()
        restored = ipc("claim_control", runtime_id=runtime)
    assert status == 0
    assert "result" in restored
