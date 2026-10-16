# External-effect adapter contract — draft

Date: 2026-09-30. Program: HARNESS-MATURE-20260929. This is a product-local interface contract, not a new durable-effort engine.

## Authority boundary

The runtime owns execution and runtime-session state. A durable-effort system (for example an AgilePlus-class adapter) owns durable work/effect receipts. Product acceptance remains external to both. Runtime-local session history is evidence/context, not sole authority for whether a side effect may be retried.

## Minimal receipt

Each side-effecting operation must be able to bind:
- effect_id;
- durable_effort_ref;
- worker_attempt_id;
- runtime product/candidate/configuration;
- tool/call identity;
- operation class and target fingerprint;
- authorization reference;
- state;
- timestamps;
- downstream idempotency/request key when available;
- downstream receipt/postcondition evidence when available.

States:
1. INTENT_RECORDED — durable intent exists before dispatch.
2. DISPATCHED — call crossed the runtime/downstream boundary.
3. CONFIRMED_SUCCESS — receipt or postcondition establishes success.
4. CONFIRMED_FAILURE — downstream establishes non-success.
5. UNCERTAIN — process/transport failure means committed outcome is not known.
6. RECONCILED_SUCCESS / RECONCILED_FAILURE — later observation resolves uncertainty.
7. ABANDONED — authorized decision not to retry/reconcile further.

A tool result in transcript must not silently convert UNCERTAIN to failure or success.

## Retry rule

- INTENT_RECORDED without DISPATCHED may be retried.
- CONFIRMED_SUCCESS must not be blindly repeated.
- CONFIRMED_FAILURE follows tool-specific retry policy.
- UNCERTAIN must reconcile first unless the downstream idempotency contract makes the retry itself the reconciliation mechanism.
- Exactly-once is never claimed merely because the runtime generated a unique tool-call ID.

## Tool classification

Every side-effecting tool/adaptor is assigned:
- READ_ONLY;
- LOCAL_REVERSIBLE;
- LOCAL_NONREVERSIBLE;
- REMOTE_IDEMPOTENT;
- REMOTE_RECONCILABLE;
- REMOTE_NONRECONCILABLE / HUMAN_CONFIRMATION_REQUIRED.

Unknown classification fails closed for automatic retry after uncertainty.

## Runtime hooks

The runtime needs only versioned hooks:
- before_effect(intent) -> durable acknowledgment;
- mark_dispatched(effect_id, downstream_request_identity?);
- mark_outcome(effect_id, outcome, receipt?);
- query_effect(effect_id) -> durable state;
- reconcile(effect_id) -> tool-specific observation or explicit unsupported.

An unavailable durable-effort adapter must be explicit. For workflows requiring durable side-effect safety it is blocking, not a telemetry warning.

## Crash-boundary oracle

Use a deterministic fixture with an append-only downstream counter and, in one mode, an idempotency key.

A. Kill after INTENT_RECORDED but before dispatch: replacement retries once; downstream count = 1.
B. Kill after downstream commits but before mark_outcome: replacement observes UNCERTAIN, reconciles; downstream count remains 1.
C. Kill after mark_outcome but before terminal worker/product state: replacement reads CONFIRMED_SUCCESS and does not repeat; downstream count = 1.
D. Non-idempotent/non-queryable downstream at boundary B: replacement must surface unresolved UNCERTAIN rather than retry automatically.

Evidence binds candidate, adapter/version, fixture/downstream configuration, effect_id, attempt A/B, raw receipt log and final postcondition.

## Non-goals

This contract does not require either CLI to implement a general workflow engine, distributed transaction manager, or universal exactly-once transport. It does not permit an implementation PR to weaken reconciliation policy and self-approve the result.

## HeliosLite mapping

H-F008 is the current gap. ToolExecutor performs service-side Write/Patch/Remove/Shell/Fetch operations before any first-class durable effect receipt is visible in the inspected path. ToolCallContext can carry an adapter/effect reference in future, but sender/metrics/conversation identity alone are insufficient. The first implementation experiment should instrument one deterministic side-effect tool through a test-only adapter without changing every tool schema.
