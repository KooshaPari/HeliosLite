# Candidate mature contract and oracle design — HeliosLite

Source: SNAPSHOT.json. Date: 2026-09-29. **DRAFT interpretation, not a frozen accepted product specification.** No requirement target or completion percentage is assigned.

## Recovered horizon and existence question

Candidate horizon: an installable, efficient coding-agent CLI/headless worker with faithful provider/tool behavior, durable session continuity, controlled side effects, human interaction where appropriate, and machine-observable outcomes. It participates in durable development efforts without becoming the sole owner of product truth. The prior user topology distinguishes app, CLI lineages and a general agent SDK; performance/headless superiority is a hypothesis, not an established differentiation.

The best-alternative challenge starts with pinned upstream Forgecode plus configuration/integration and independently held task criteria, and compares a Codex App Server based worker stack. Choose neither by README or sunk cost. Registry research must establish capability losses, provider restrictions, native-platform cost, operational burden and maintenance cost before deciding keep/fork/adapt/compose. Generic tool execution, sessions, MCP and JSON-RPC alone do not justify a fork.

## Ontology: overlapping projections, not one flattened tree

Product identity/release configuration; actor/outcome journeys; workspace and tool authority; session/thread/turn/provider/tool-call state; worker lease/attempt; durable change intent/work package; accepted product artifact/evidence; storage/migration; CLI/TUI/headless projections; adapters; operations/supply chain; verification/quality. A CLI command may expose several capabilities; a capability may span several crates. File or crate existence is not a product feature.

## Accepted assignment constraints, applied once

These constraints derive from the current user mission, not inferred code behavior:

- `H-INV-IDENTITY`: product, accepted contract/criterion, candidate, configuration, environment, verifier/version, run, time and raw provenance identify each accepted observation. Missing or mismatched identity is non-green.
- `H-INV-LIFETIMES`: worker attempt can end without deleting durable development intent or accepted product state. Work completion does not equal product acceptance.
- `H-INV-AUTHORITY`: imported assertion, agent inference, authorized decision and verified observation remain different provenance classes; confidence cannot authorize an edge.
- `H-INV-GRADER`: required failures, skips, missing collection and empty checks cannot produce acceptance. Worker-modified grader/policy cannot silently ratify itself.
- `H-INV-GROWTH`: earlier stages project the mature contract; additions must preserve the identity/state spine or explicitly account for transition debt.

These are shared invariants, not generic rows cloned onto every feature. Product-specific obligations await source reconciliation.

## Proposed journeys and stage projections

| Journey | Actor to outcome | Candidate earliest stage | Evidence required |
|---|---|---|---|
| H-J01 | Operator installs an identified fork, configures an authorized provider, performs one bounded repository change, reviews outcome | CVP | Artifact and process identity; actual tool effect; independently checked result; failure exits distinguishable from success |
| H-J02 | Worker is interrupted and replaced; operator resumes the same durable effort with valid approvals and no duplicated external effect | MVP | Persisted state, old/new attempt linkage, crash/restart record, effect-idempotency/uncertain-effect handling |
| H-J03 | Machine caller submits bounded work and receives typed progress, cancellation and terminal result without needing a TTY | MVP | Actual mounted machine surface, negative fixtures, configuration-bound replay and collector failure |
| H-J04 | Operator upgrades or rolls back supported artifacts without silently changing provider semantics or corrupting sessions | Beta/GA by platform | Versioned migrations, retention/recovery, channel identity, signing/support policy and downgrade controls |
| H-J05 | Multiple supported consumers use adapters while a shared independent grader reports quality and uncertainty | Mature | Per-consumer conformance, isolation, resource limits, authority and evidence-age controls |

Stage labels and platform inclusion are proposals until intent review. CVP must close H-J01, not merely contain disconnected primitives. Stubs may expose explicit unsupported capability responses; they must never claim an accepted success. No numeric latency, memory, reliability or accessibility threshold is invented here.

## Quality overlays and growth debt

Bind latency/RSS/PSS/startup/concurrency measurements to workload, provider/model, embeddings, build/profile, OS, cache state and session count. Define reliability under cancellation, retries, provider failure and restart; accessibility on actual terminal interactions; security on tool/MCP credentials and workspace capabilities. Keep supported-platform and release-signing profiles explicit. Targets and sampling plans remain OPEN. Transition debts: legacy forge naming/config compatibility, dual SQLite store reconciliation, provider-history transformations, internal workspace coupling, and any future external durable-effort adapter.

## First vertical-slice contract (proposal)

Use real persistence, actual CLI and machine interface, one isolated temporary repository, a local deterministic provider fixture, one side-effecting tool and an independently located grader. Bind attempt A to effort E and candidate C, perform an authorized edit, interrupt around commit/receipt boundaries, replace worker with attempt B, resume E, and prove the intended final effect without duplicate execution. Deliberately feed empty output, timeout, stale evidence, wrong candidate/configuration, denied tool, provider failure, conflicting results and modified grader policy. Provider fixture results do not qualify live provider behavior. Native Windows/Pine compatibility remains a separate consumer experiment, not an implicit dependency expansion.

## Oracle policy and evidence schema design

Design evidence records with product/subject/contract/criterion/candidate/config/environment/verifier-version/run/timestamp/artifact-digest/provenance plus status and reason. Candidate includes source and selected dependency locks, built binary digest, installed path and live process/daemon identity where applicable. Terminal states include pass, fail, not-run, skipped, collector-error, invalid-scope, stale and conflict. Only a policy-qualified pass over all required criteria closes a journey; N/A requires an authorized applicability decision.

The grader runs outside worker-writeable acceptance policy and records policy/code versions. Required controls: true success; wrong behavior; wrong product/workspace; old contract/candidate; empty/missing reports; invalid input; denied operation; failed dependency; timeout; interrupted persistence; replacement worker; replayed/double evidence; regression; conflict; verifier crash/signal; grader weakening. Mutation question: removing each guard must yield a witness rejected by the protected gate. Adversarial design is required now even where test generation waits.

Progress is a vector: functional qualification, traceability, evidence validity, journey closure, regression, performance, reliability, security, accessibility, usability, uncertainty and transition debt. Do not average critical failures away. Scope delta and engineering delta are separate; no velocity/asymptote claim without repeated comparable evidence.

## Open design risks

Full useful history and source denominator; authoritative mature capability set; actual provider/session semantics; best alternative; SDK custody; lockfile/build graph; native recovery and process-tree cleanup; grader trust/immutability implementation; release identity; full trace graph and fresh independent challenge. Architecture and product-existence gates remain OPEN.
