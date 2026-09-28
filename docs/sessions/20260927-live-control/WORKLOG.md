# Forge live control implementation

Owner: /root/forge_bridge. Worktree forge-live-control-acp-20260927 only.
Base: 49769b34d84c2d37d8ca7c7e119a93defc7a8d84; remote fork=KooshaPari/HeliosLite.
Approved plan: HarnessDesk CLIENT-CONTRACT.md and evaluation IMPLEMENTATION-STATUS.md.

## Current slice

Added typed versioned ChatEvent projection and wired existing --stream-json and
--stream-json-log producer to retain text, tool calls/results, reasoning, retry,
and interruption data. Serialization does not acknowledge tool execution.
Tests authored for text preservation and notifier non-interference.
Compilation/tests pending remote desktop availability; no heavy local builds.
No runtime, ACP or actionable remote interaction capability claimed yet.

## Required remaining work

1. Live session actor: stable persisted conversation UUID, distinct runtime UUID,
   serialized prompt queue, exact turn cancellation and exclusive ownership lease.
2. Owner-only versioned UDS IPC to this same actor; bounded event replay sequence,
   explicit resync_required and snapshot for expired cursors.
3. UserInfra broker with request/session/runtime/turn UUIDs, configurable five-minute
   TTL, exactly-once TUI/remote responses, permission deny and followup None on
   expiry/cancel; reconnect recovers pending requests before expiration.
4. Standard ACP v1 stdio bridge command using verified schema/SDK; initialize,
   session/new/load/list/prompt/cancel/update/request_permission and negotiated
   elicitation/create forms. Internal IPC is not an ACP extension.
5. Independent negative controls for duplicate/stale/wrong session/invalid/late
   responses, cancellation/timeouts, replay gaps, and concurrent ownership.
6. Remote compile/tests and synthetic runtime qualification; commit/push/draft PR.

Preserve main checkout and other harness edits. No screenshots or process kills.
Heavy compute placement: remote desktop only; alias presently offline/unresolved.

## Interaction slice in progress

Broker implementation and negative controls now authored; policy method explicitly
carries PermissionOperation through UserInfra/ForgeRepo/ForgeInfra. Outside live
actor context existing terminal widgets are unchanged. Inside live context TUI
and remote respond to the same held call, and cancellable terminal input exits
when another responder wins. App stream task propagates actor context explicitly.
Remote compile/test still pending. PR draft #319; first slice 76703197c pushed.
New transcript logs use Unix mode 0600; rich output remains CLI opt-in.

## Actor and IPC source wiring

UI initialization now acquires a per-conversation OS lease and creates one actor;
local on_chat submits into that actor. The owner-only Unix socket reaches that same
actor for prompt, exact-turn cancel, snapshot/replay and pending response. Runtime
UUID differs from conversation UUID. Prompt command IDs retain accepted results and
reject mismatched reuse; active turns serialize queued prompts. Queue and replay
are bounded. Expired/future replay cursors explicitly require a persisted snapshot.
Orchestration cancellation saves conversation before actor starts the next turn.

Known required follow-up: remote-origin turns currently avoid reading the terminal
while idle readline owns input; external-printer/event-loop integration is still
needed to show remote turns in the TUI and allow local answers to those remote
requests. Existing local turns can be answered by the same broker remotely.
ACP bridge, compile, protocol/actor negative controls and runtime qualification
remain pending. Source is not installation/runtime proof.

## Hosted evidence for 76703197cf8a07ee285d12777d16b090dde42b3f

CVP run 36342887728 logged cargo check --workspace, clippy --workspace -- -D warnings,
and cargo test --workspace; completed jobs successful and both new ChatEvent tests
explicitly passed. macOS job 108686293790 / run 36342887699 reported success but its
log contains forge_lsp failure: 117 passed, two mcp_watcher timing tests failed
(watcher_fires_reload_on_modify, watcher_debounces_burst_into_one_reload).
Both new ChatEvent tests passed there. Therefore macOS full-suite status is failed,
not green despite the job summary. These results do not validate later source.

## Hosted gate repair

platform-tests.yml explicitly used continue-on-error: true for cargo test; removed
that masking. The two failures are in forge_lsp, not forge_infra (corrected above).
Both observed zero reloads. Source path comparison was lexical; macOS /var aliases
can differ from canonical /private/var paths in filesystem notifications. Parent
identity now uses canonical paths while retaining filename equality, including
remove events where the file no longer exists. A symlink-parent negative control
covers correct target and unrelated file. Existing runtime tests remain unchanged
pending actual hosted evidence. Split pre-existing 554-line watcher test module.

## ACP source slice and compile feedback

Official agent-client-protocol-schema =1.9.1 v1 types pin the wire boundary.
Actual ACP stdio now launches/attaches owner-leased host runtimes through IPC,
negotiates initialize, supports session/new/load/list/prompt/cancel, projects
session/update, and correlates request_permission + negotiated form elicitation
back to the original broker. Disconnect leaves runtime/pending requests alive.
Runtime, turn, request and sequence metadata use io.phenotype/ namespaced _meta.
Detached host creation acquires lease before creating/loading persisted session.
No arbitrary executable is accepted from clients.

828d52f32 hosted build reached forge_main and failed -D dead_code for unused
IPC call() (ACP now consumes it); Clippy also found cancellable.rs collapsible_if,
now corrected. These are real new-source failures, not the prior watcher failure.
Current ACP source awaits its own compile/test evidence.

## Corrective source and TUI integration

11bc69581 exact hosted failures: strum feature-unification caused duplicate
EnumProperty macro import; ConfigReader was wrongly treated as trait; ToolCallId
has serde/as_str but no Display; permission choices triggered useless_vec.
Corrected all four. No local heavy compilation used.

Readline external printer now displays remote transcript/tool lifecycle and held
questions while preserving the input buffer. /respond REQUEST_ID answer (or
--cancel) resolves the same broker request; choice numbers are explicit 1-based
terminal inputs mapped to offered zero-based options. No second stdin reader is
spawned for remote turns. Runtime sockets now honor ConfigReader::base_path(),
including isolated HELIOSLITE_HOME, instead of touching legacy ~/.forge state.

Factored the exact production MpscStream context propagation helper and added a
real ForgeInquire-through-spawn test asserting pending session/runtime/turn IDs
and response delivery. Orchestrator sequential and parallel tool scopes now carry
the original ToolCallFull into permission requests. Full fake-provider/IPC and
interactive PTY qualification still required; not inferred from these unit tests.
