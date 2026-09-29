"""A completed command retry after reconnect must not outlive its replay receipt."""

import json
import uuid

from live_acp import Client
from live_control import wait_for


def qualify_replay(ipc, runtime, session, root, binary, env, command, original_turn):
    event = {
        "id": str(uuid.uuid4()),
        "timestamp": "2026-09-29T00:00:00Z",
        "value": {"Text": "SYNTHETIC_REPLAY_FLOOD"},
        "attachments": [],
        "additional_context": None,
    }
    turn = ipc("prompt", runtime_id=runtime, command_id=str(uuid.uuid4()), event=event)[
        "result"
    ]["turn_id"]

    def finished():
        snapshot = ipc(after=0)["result"]
        assert any(
            item["turn_id"] == turn and item["payload"]["kind"] == "turn_finished"
            for item in snapshot["events"]
        )
        return snapshot

    snapshot = wait_for(finished, timeout=45)
    assert snapshot["resync_required"]
    assert not any(item["turn_id"] == original_turn for item in snapshot["events"])
    sequence = snapshot["sequence"]
    assert "result" in ipc("release_control", runtime_id=runtime)
    client = Client(binary, root, env)
    try:
        client.request(1, "initialize", {"protocolVersion": 1})
        client.request(
            2,
            "session/load",
            {
                "sessionId": session,
                "cwd": str(root),
                "mcpServers": [],
                "_meta": {"io.phenotype/interactionController": True},
            },
        )
        client.send(
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": session,
                    "prompt": [{"type": "text", "text": "Reply with the witness."}],
                    "_meta": {"io.phenotype/commandId": command},
                },
            }
        )
        response = client.wait(lambda message: message.get("id") == 3)
        assert "command_result_expired" in response["error"]["message"], response
        assert ipc(after=sequence)["result"]["sequence"] == sequence, (
            "retry executed again"
        )
        print(
            json.dumps(
                {
                    "replay_checks": [
                        "terminal_evicted",
                        "fresh_acp_attach",
                        "expired_retry_rejected",
                        "no_reexecution",
                    ]
                }
            )
        )
    finally:
        assert client.close() == 0
        assert ipc("claim_control", runtime_id=runtime)["result"]["controlled"]
