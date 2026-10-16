#!/usr/bin/env python3
"""Crash-boundary oracle for the external-effect adapter contract.

This is a contract diagnostic, not production runtime integration.
It uses real subprocess termination, filesystem durability, and a real side-effect log.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

INTENT = "INTENT_RECORDED"
DISPATCHED = "DISPATCHED"
CONFIRMED = "CONFIRMED_SUCCESS"
UNCERTAIN = "UNCERTAIN"
RECONCILED = "RECONCILED_SUCCESS"


def atomic_json(path: Path, value: dict) -> None:
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(value, sort_keys=True))
    os.replace(tmp, path)


def load(path: Path) -> dict:
    return json.loads(path.read_text())


def append_effect(log: Path, effect_id: str, idempotent: bool) -> None:
    existing = log.read_text().splitlines() if log.exists() else []
    if idempotent and effect_id in existing:
        return
    with log.open("a") as fh:
        fh.write(effect_id + "\n")
        fh.flush()
        os.fsync(fh.fileno())


def effect_count(log: Path, effect_id: str) -> int:
    if not log.exists():
        return 0
    return sum(line == effect_id for line in log.read_text().splitlines())


def child(root: Path, boundary: str, queryable: bool, idempotent: bool) -> None:
    receipt = root / "receipt.json"
    log = root / "downstream.log"
    effect_id = "effect-001"
    base = {
        "effect_id": effect_id,
        "durable_effort_ref": "effort-test",
        "worker_attempt_id": "attempt-a",
        "operation": "append-counter",
        "queryable": queryable,
        "idempotent": idempotent,
    }
    atomic_json(receipt, {**base, "state": INTENT})
    if boundary == "before_dispatch":
        os._exit(71)

    atomic_json(receipt, {**base, "state": DISPATCHED})
    append_effect(log, effect_id, idempotent=idempotent)
    if boundary == "after_effect":
        os._exit(72)

    atomic_json(receipt, {**base, "state": CONFIRMED})
    if boundary == "after_outcome":
        os._exit(73)
    raise AssertionError(boundary)


def recover(root: Path) -> str:
    receipt = root / "receipt.json"
    log = root / "downstream.log"
    state = load(receipt)
    effect_id = state["effect_id"]

    if state["state"] == INTENT:
        append_effect(log, effect_id, idempotent=state["idempotent"])
        atomic_json(receipt, {**state, "worker_attempt_id": "attempt-b", "state": CONFIRMED})
        return CONFIRMED

    if state["state"] == DISPATCHED:
        atomic_json(receipt, {**state, "worker_attempt_id": "attempt-b", "state": UNCERTAIN})
        if state["queryable"]:
            if effect_count(log, effect_id) == 1:
                resolved = load(receipt)
                atomic_json(receipt, {**resolved, "state": RECONCILED})
                return RECONCILED
            raise AssertionError("queryable downstream did not expose exactly one committed effect")
        return UNCERTAIN

    if state["state"] == CONFIRMED:
        return CONFIRMED
    raise AssertionError(state)


def run_case(boundary: str, queryable: bool, idempotent: bool, expected_state: str) -> dict:
    with tempfile.TemporaryDirectory(prefix="effect-recovery-") as tmp:
        root = Path(tmp)
        args = [
            sys.executable,
            __file__,
            "--child",
            str(root),
            boundary,
            "1" if queryable else "0",
            "1" if idempotent else "0",
        ]
        proc = subprocess.run(args, check=False)
        assert proc.returncode in (71, 72, 73), proc.returncode
        recovered = recover(root)
        count = effect_count(root / "downstream.log", "effect-001")
        assert recovered == expected_state, (boundary, recovered, expected_state)
        if boundary == "before_dispatch":
            assert count == 1, count
        elif boundary in ("after_effect", "after_outcome"):
            assert count == 1, count
        return {
            "boundary": boundary,
            "queryable": queryable,
            "idempotent": idempotent,
            "recovered_state": recovered,
            "downstream_effect_count": count,
        }


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--child":
        _, _, root, boundary, queryable, idempotent = sys.argv
        child(Path(root), boundary, queryable == "1", idempotent == "1")
        return

    results = [
        run_case("before_dispatch", True, True, CONFIRMED),
        run_case("after_effect", True, True, RECONCILED),
        run_case("after_outcome", True, True, CONFIRMED),
        run_case("after_effect", False, False, UNCERTAIN),
    ]
    assert results[-1]["downstream_effect_count"] == 1
    print(json.dumps({
        "schema": "external-effect-recovery-probe/v1",
        "scope": "contract diagnostic with real subprocess death/filesystem effect; not product runtime integration",
        "cases": results,
        "assertions": "passed",
    }, indent=2))


if __name__ == "__main__":
    main()
