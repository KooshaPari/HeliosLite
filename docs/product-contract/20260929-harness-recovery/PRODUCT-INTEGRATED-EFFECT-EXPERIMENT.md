# Product-integrated effect-recovery experiment — HeliosLite

Date: 2026-09-30. Status: DESIGN_READY / IMPLEMENTATION_NOT_STARTED.

## Injection seam

Instrument `ToolExecutor::execute`, not every Fs/Shell/Fetch service. This is the common boundary immediately above `call_internal`, where tool class/input are known and before the side effect begins.

`ToolCallContext` currently has no tool-call/effect identity. Add an optional, versioned effect context rather than overloading conversation_id or source:
- worker_attempt_id;
- durable_effort_ref;
- effect_adapter: trait/object handle.

The effect ID is allocated by the adapter from durable effort + attempt + operation fingerprint; it must not depend solely on model-provided optional ToolCallId.

## Experiment-only adapter

Implement an in-memory/filesystem test adapter behind a trait:
- begin(intent) -> EffectHandle after durable INTENT_RECORDED;
- mark_dispatched(handle);
- confirm(handle, outcome/receipt);
- mark_uncertain(handle, reason);
- state(handle).

Production default with no adapter preserves current behavior for ordinary non-durable sessions. A future workflow that declares durable-effect safety must fail before dispatch if its required adapter is unavailable.

## First tool

Use a deterministic write-like fixture through ToolExecutor, not a remote provider. Kill the worker at:
A. after begin before call_internal;
B. after the underlying file write but before confirm;
C. after confirm before terminal conversation state.

Replacement attempt must use the same durable_effort_ref and reconcile file hash/existence. For B, the effect is UNCERTAIN until reconciliation proves the write. No second write is issued merely because the transcript lacks ToolOutput.

## Why not instrument ForgeFsWrite first

FsWrite has snapshot/hash semantics that are useful reconciliation evidence, but instrumenting only it would leave Shell/Fetch/Remove/Patch with different safety models. The common executor seam is the product contract; tool-specific reconcilers can live below it.

## Acceptance

Native test must prove exact effect count/postcondition, durable receipt sequence, attempt A/B lineage and fail-closed behavior when reconciliation is unsupported. A contract-probe pass alone is insufficient.

## Attempt-B reconciliation API — pinned
Replacement recovery is a durable-effort decision before calling `ToolExecutor::execute` again. The executor must not infer retry permission from missing ToolOutput.

Adapter extension:
- `load(effect_id) -> EffectRecord?`;
- `reconcile(effect_id, observation) -> CONFIRMED_SUCCESS | CONFIRMED_FAILURE | RETRY_ALLOWED | STILL_UNCERTAIN`.

For Write, persist target path and expected content/hash in the effect intent. Attempt A writes and dies/loses durable confirmation. Attempt B reads/hashes the target before dispatch. Match => RECONCILED_SUCCESS/no write; provable absence/precondition => RETRY_ALLOWED; conflicting or unknowable state => STILL_UNCERTAIN and fail closed.

The current candidate's effect ID uses durable effort + conversation + path because ToolCallContext lacks stable call identity. Before production integration, add a runtime-generated effect/call identity to the context so repeated legitimate writes to the same path cannot alias. Path-only identity is test scaffolding, not accepted mature identity.
## Identity correction — reuse ToolCallFull.call_id
Further source tracing found the correct identity already exists immediately above execution: `ToolRegistry::call` receives `ToolCallFull`, whose `call_id: Option<ToolCallId>` is preserved into ToolResult; `ToolCallId::generate()` already exists for missing provider IDs.

Therefore do **not** create a second effect-call namespace. At `ToolRegistry::call`, clone the context and set an execution call ID equal to the model/provider call ID when present, otherwise a runtime-generated ToolCallId. Pass that context through `call_inner` -> ToolExecutor. The effect adapter uses durable_effort_ref + execution call ID (+ tool name/version as needed), not file path.

This also makes identity stable across attempt replacement only if the durable effort preserves the assigned call ID. The generated fallback must be persisted with the attempt/intent before dispatch; regenerating on attempt B would create a new effect and defeat reconciliation.