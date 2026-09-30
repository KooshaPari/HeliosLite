# Semantic findings — HeliosLite

All repository paths below refer to source `536a25cac1dc21ac97bbc86c7e9af74bd5932780`, inspected 2026-09-29. These are source-level findings, not claims that native product tests were run.

## H-F001 — Empty or missing validation can become passed

Severity: blocks reliance on the inherited benchmark as acceptance authority.

`benchmarks/verification.ts::allValidationsPassed` uses `results.every(...)`. `processValidations` substitutes an empty result array when checks are absent or output is falsy, then turns that empty array into `passed`. Counterexamples: empty checks with arbitrary output; configured `^OK$` check with no output. Correct acceptance must distinguish a declared no-output success criterion from missing collection, and require a nonempty set of applicable criteria. No generic blanket requirement that all valid commands emit text is intended.

## H-F002 — Timeout information is dropped at the CLI aggregation boundary

`benchmarks/task-executor.ts::executeTask` resolves with captured output on timeout and returns `isTimeout: true` without an `error`. `benchmarks/cli.ts` updates `hasTimeout` only inside `if (executionResult.error)`. Therefore a timeout can reach normal validation, including the empty-output success path. Test timeout before output, timeout after matching partial output, child descendants surviving shell termination, cancel races and explicitly authorized early-exit tasks. A declared early-exit task is not equivalent to an unplanned timeout.

## H-F003 — Validation failure can still produce process exit zero

The final CLI summary exits 1 only when `failCount > 0`; validation failures and timeouts are explicitly excluded. A caller using exit status alone can observe a green despite failed acceptance. Resolve the benchmark's observational-report versus gating modes explicitly; a report-only run must not be consumed as product acceptance. The strict gate must propagate every required failed/incomplete dimension without averaging.

## H-F004 — A signalled shell verifier is coerced to exit zero

`validateShellCommand` handles child close using `code ?? 0`, discarding the signal. A terminated verifier with null exit code can satisfy an expected-zero check. Preserve code, signal and collector outcome separately. Design a verifier timeout and process-tree policy. The diagnostic probe also checks Node's real null-code/SIGTERM event; it does not execute the repository's complete benchmark.

## H-F005 — Manifest dependency identity differs from current Shared head

`Cargo.toml` pins three phenotype packages to `b2d06d53`, expanded to `b2d06d5376c1e94e207c679f402597103a5a10a4`. The separately observed Shared default is `d24b13edbb76741b8063f4f7f3369d1b7310ca5b`; evidence is not interchangeable. At the manifest pin, `crates/agile-plus/crates/agileplus-cache/Cargo.toml` (blob c9a4252dcb5478e763d394e435b3013812f2566c) contains lib/features/dependencies but no package table. The commit calls this a manifest repair. This is a source inconsistency requiring Cargo/lockfile/build-graph verification, NOT a claim that every HeliosLite build fails or that this crate is selected. No dependency update is made by this pass.

## H-F006 — Historical release success does not qualify every release property

The Sep 18 release handoff explicitly records unsigned Windows binaries and signing skip paths. Its claim of a completed release-assets mission does not establish signing, native behavior or the mature contract. Preserve the historical mission's limited meaning. Define each platform's accepted signing/support policy before treating a skip as N/A; no retroactive assertion that all platforms were contractually required to be signed.

## H-F007 — Existing requirements are claims, not recovered mature truth

The current catalog says implemented means shipped and tested and marks the evaluation harness implemented. H-F001–004 demonstrate why those labels cannot serve as independent acceptance. Preserve the catalog for archaeology, extract distinct obligations, reconcile authority, and bind accepted criteria to exact evidence. Do not replace it with a generated high-count catalog.

## Diagnostic evidence boundary

The local decision-flow probe uses transcribed, scoped logic plus an actual Node child-process termination. Its success means the counterexamples were reproduced in that diagnostic model. It is NOT native CLI execution, a tested patch, platform qualification or an independent fresh review. Native product tests remain NOT_RUN. Source findings remain OPEN until source-bound tests and fixes are independently reviewed.

## H-F008 — Side effect can precede any durable external-effect receipt

`crates/forge_app/src/tool_executor.rs::execute` calls `call_internal` first. Side-effecting branches such as Write/Shell/Patch/Remove perform the service operation inside `call_internal`; only after it returns does the executor send formatted output and convert the operation to `ToolOutput`. `ToolCallContext` carries an optional sender, metrics and conversation/source metadata, but no durable effect ID/state machine or idempotency/reconciliation interface. `ToolCallId` exists at the model-call layer but is optional and is not established here as a downstream idempotency key.

Therefore a crash after the service-side effect commits but before its result is durably attached to development/product state can leave an externally ambiguous outcome. This is a source-level crash-window finding, not proof that duplicate effects have occurred in production. Required resolution: bind side-effect execution to an external-effect receipt with INTENT_RECORDED/DISPATCHED/CONFIRMED/UNCERTAIN/RECONCILED semantics, owned by durable effort or a versioned adapter rather than presentation telemetry. Test kill-before-dispatch, kill-after-effect-before-ack, and kill-after-ack boundaries with idempotent and non-idempotent fixtures.

## H-F009 — Mounted `forge_tracera` conflates Tracera product identity with telemetry transport

Frozen HeliosLite main initializes `TraceraTelem` and emits session lifecycle telemetry when `TRACERA_ENDPOINT` is configured. The local `forge_tracera` crate explicitly describes itself as an outbound observability wire format (`tracera.v1`) with HTTP batching/auth/offline retry.

Accepted user intent recovered elsewhere defines Tracera as the canonical product/feature/traceability graph, not merely an observability sink. Therefore the mounted integration is real implementation but its **semantic ownership/name is contradictory**. Do not infer that this telemetry transport satisfies Tracera integration obligations. Resolve whether it is: (a) a generic telemetry/evidence transport that should be renamed and optionally export into Tracera, or (b) a legitimate Tracera ingestion adapter conforming to Tracera's accepted graph/evidence contract. Until resolved, it is CURRENT IMPLEMENTATION / CONTRADICTORY NAMING-BOUNDARY, not accepted product truth.

## H-F010 — Mounted AgilePlus implementation may duplicate external durable-effort authority

Frozen HeliosLite main mounts `helioslite agileplus <subcommand>` before full UI startup. Local `forge_agileplus` implements a 31-pillar scorecard, sprint records and rolling velocity; ADR 003 labels AgilePlus adoption Accepted. This proves mounted implementation/history, but not that HeliosLite should canonically own AgilePlus's delivery/process model.

The mature recovery architecture separates worker runtime from durable development effort. Therefore keep the mounted code classified as CURRENT IMPLEMENTATION + HISTORICAL ACCEPTED ADR pending authority reconciliation: if AgilePlus is the durable-effort/product-management authority, Helios should consume a versioned interface rather than fork its canonical model internally. Do not delete or bless the local crate until schema/authority equivalence is tested.