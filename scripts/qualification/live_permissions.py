"""Real policy decisions must gate the exact synthetic shell operation."""

import json
import uuid

from live_control import wait_for


def qualify_permissions(ipc, runtime, root):
    for mode in ["reject", "accept", "cancel", "expire"]:
        witness = root / f"permission-{mode}"
        event = {
            "id": str(uuid.uuid4()),
            "timestamp": "2026-09-29T00:00:00Z",
            "value": {"Text": f"SYNTHETIC_PERMISSION_{mode}"},
            "attachments": [],
            "additional_context": None,
        }
        turn = ipc(
            "prompt", runtime_id=runtime, command_id=str(uuid.uuid4()), event=event
        )["result"]["turn_id"]

        def held(turn=turn):
            snapshot = ipc(after=0)["result"]
            pending = [item for item in snapshot["pending"] if item["turn_id"] == turn]
            ended = [
                item
                for item in snapshot["events"]
                if item["turn_id"] == turn
                and item["payload"]["kind"] == "turn_finished"
            ]
            if ended and not pending:
                raise RuntimeError(
                    f"permission operation completed without a held decision: {ended}"
                )
            assert len(pending) == 1
            return pending[0]

        request = wait_for(held, timeout=20)
        assert request["kind"]["kind"] == "permission"
        assert request["kind"]["operation"]["kind"] == "execute"
        assert str(witness) in request["kind"]["operation"]["command"]
        assert request["tool_call"]["call_id"] == f"synthetic-permission-{mode}"
        assert not witness.exists(), "operation executed before permission decision"
        response = {
            key: request[key]
            for key in ["request_id", "session_id", "runtime_id", "turn_id"]
        }
        response["answer"] = {
            "kind": "choices",
            "value": [0 if mode == "accept" else 1],
        }
        if mode in {"accept", "reject"}:
            assert "result" in ipc("respond", runtime_id=runtime, response=response)
        elif mode == "cancel":
            assert "result" in ipc("cancel", runtime_id=runtime, turn_id=turn)

        def finished(turn=turn, mode=mode):
            events = ipc(after=0)["result"]["events"]
            terminal = [
                item
                for item in events
                if item["turn_id"] == turn
                and item["payload"]["kind"] == "turn_finished"
            ]
            assert terminal
            expected = "cancelled" if mode == "cancel" else "completed"
            assert terminal[-1]["payload"]["status"] == expected, terminal[-1]
            return True

        wait_for(finished, timeout=15)
        assert "error" in ipc("respond", runtime_id=runtime, response=response)
        if mode == "accept":
            assert witness.read_text() == "approved", (
                "operation did not execute exactly once"
            )
        else:
            assert not witness.exists(), f"{mode} unexpectedly executed the operation"
    print(
        json.dumps(
            {
                "permission_checks": [
                    "pending_before_execute",
                    "exact_operation_identity",
                    "allow_once",
                    "reject",
                    "cancel",
                    "expiry",
                    "duplicate_rejected",
                ]
            }
        )
    )
