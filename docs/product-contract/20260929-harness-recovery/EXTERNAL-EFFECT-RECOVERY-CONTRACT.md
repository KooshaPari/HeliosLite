# External-effect recovery contract — HeliosLite

Date: 2026-09-30. Status: DRAFT mature-contract obligation; implementation mapping evidence-backed.

## Current implementation facts
- Forge domain models `ToolCallId`; some provider tool calls may omit IDs and internal/XML recovery can generate IDs.
- `ToolResult` carries the matching optional call ID.
- `forge_app::ToolExecutor` dispatches filesystem, shell, network and other side effects through service traits, with optional sandboxing.
- the inspected frozen source exposes no first-class durable pre-dispatch/outcome/uncertainty receipt around those calls.

Therefore a tool-call/result pair is conversation evidence, not a durable transaction/effect receipt. Generated or absent IDs particularly cannot be treated as downstream idempotency keys without an explicit mapping.

## Required adapter contract
Before a side-effecting operation crosses `ToolExecutor`/service boundaries, provide an adapter hook that can durably bind: external durable effort, worker attempt, runtime conversation/tool-call identity, stable effect ID, operation/target digest and authorization reference. After dispatch, emit provider/downstream request identity where available and a qualified outcome.

Use the same states as the cross-product contract: `intent_recorded`, `dispatched`, `confirmed_success`, `confirmed_failure`, `uncertain`, `reconciled_success`, `reconciled_not_applied`, `manual_resolution_required`. Crash after dispatch without trustworthy acknowledgement is `uncertain`.

## Ownership
HeliosLite should not grow a second canonical durable-work engine. The external development-effort authority owns the durable receipt; Helios exposes versioned hooks/adapter events. Tracera best-effort telemetry is explicitly insufficient as the acceptance/effect ledger because its existing bridge can drop delivery failures.

## Falsifying experiment
Run a real ToolExecutor side effect against a deterministic local service/file fixture, terminate the worker at pre-dispatch, post-effect/pre-ack and post-ack/pre-terminal boundaries, then resume through a replacement attempt bound to the same external effort. Prove no blind retry of uncertainty and reject stale/wrong candidate receipts.

Current state: contract defined; implementation adapter and native crash experiment NOT_RUN.