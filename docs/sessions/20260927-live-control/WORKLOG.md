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

## 2026-09-29 continuation

Preserved 5fbdee8d4 pushed without rewrite to PR #319. Hosted CVP run
36580276682: full workspace cargo check passed at that source; clippy failed
and workspace tests were still running at observation. This is the first
compile pass for the actor/ACP/TUI integration, not runtime qualification.
Compute placement remains standard free GitHub ubuntu-latest; no heavy local
build was started. Main checkout and unrelated processes remain untouched.

Added production spawned UserInfra permission negative controls (wrong session,
runtime, turn, invalid selection, duplicate response) and actual followup
cancellation/TTL/late-response tests. Corrected synchronous terminal response
handling and map-entry registration/ACP conditional lint issues. These tests
require exact new-source hosted execution before claiming pass.

Hosted clippy job 109446301338 showed five failures: four collapsible ACP
conditionals and one input mutex held across terminal response await. All five
are corrected. Added isolated cross-process qualification script and free
ubuntu-latest workflow: real binary host lease/socket modes, conflicting owner,
runtime mismatch, future cursor resync, schema initialize, ACP attachment to
same runtime, and disconnect preserving that owner. It creates only temporary
state and terminates only subprocesses it started. This host qualification is
not yet same-live-TUI/fake-provider successful-turn proof; that remains open.

Synthetic live-TUI qualification now starts a separate owned PTY process using
only temporary HELIOSLITE_HOME and a localhost SSE provider. It requires a
completed remote prompt, correlated session/runtime/turn events, transcript text
in that same live TUI, and duplicate/conflicting command behavior. While adding
this witness, found an actual idempotency bug: ACP regenerates Event UUID/time
on retry, so full JSON comparison falsely rejected an identical command.
Fingerprint comparison now excludes only those generated metadata fields,
retaining session, prompt, attachments and context; independent negative test
requires changed content/session to differ. Hosted runtime results still pending.

## Verified 81e0e6ad1 hosted runtime witness

Run 36582436825 / job 109454081677 logs: domain interaction 4/4;
real spawned UserInfra 3/3; live_control 4/4; exact helioslite binary built.
Process checks passed for private socket, exclusive owner, runtime identity,
future-cursor resync, ACP initialize/same-owner attach/disconnect preservation.
Real TUI (owned PTY, no captures) passed remote prompt completion, terminal text
witness, duplicate/conflict command handling and event correlation. Witness:
session dc6d7ee9-a8f0-4fc4-ac99-0b8595326161, TUI runtime
50e89598-89dd-4174-9c3b-67429120e804, turn
f34e2cf2-b5b4-460b-9020-52c23ea1abb1. Prior a29a53f25 full workspace tests,
check and clippy passed; 81e0e6ad1 clippy found one redundant error conversion.

Grouped follow-up fixes normalize fingerprint error type, observe cancellation
before/during real UserInfra waits, test pre-cancel and original tool-call
identity, and exercise held followups through the actual synthetic provider/TUI:
local /respond, wrong identity, invalid answer, duplicate/late response,
turn cancellation and five-second TTL. Python ruff + syntax and Rust formatting
checked locally; hosted execution required for the changed revision.

## Explicit external controller contract

HarnessDesk integration identified a required distinction: runtime ownership is
not remote interaction ownership. Per CLIENT-CONTRACT.md section 2.2, passive
session/load must not enable controls. Added actor-serialized 30-second external
controller leases (renewed by ACP polling), explicit grant on namespaced
io.phenotype/interactionController load metadata, advertised initialize capability,
passive mutation rejection, conflict rejection (no steal), release on disconnect,
and session_info_update ownership-loss metadata. Local native TUI stays available.
Only the external controller receives ACP held requests; runtime and pending
operations survive connection loss. Interaction metadata now explicitly includes
io.phenotype/sessionId so forms never depend on a client's selected session.
Actor handle extraction keeps new modules below the 350-line target.

Qualification now requires passive controls rejected, exclusive controller,
explicit ACP grant, and control release on disconnect. Existing actual TUI
prompt/followup tests acquire control first, verify remote and local answers reach
the original tool result, and retain cancel/expiry/late-answer negative controls.

At e7833cb2e: focused Rust tests and binary build passed, host/ACP checks passed;
held-followup runtime witness failed because the fixture left global
`tool_supported` false (production agent config overrides model capabilities).
Synthetic provider therefore received no native tool definitions and emitted its
text witness. Fixture now explicitly enables native tool support. This is a
fixture correction, not weakening the held-operation acceptance assertion.

## 9e4f1b70c ownership qualification and corrected followup diagnosis

Run 36585387869 / job 109464188267 passed focused Rust/ACP/controller tests
and exact binary build. Actual process gates passed: passive control rejection,
exclusive controller, explicit ACP grant, disconnect release, owner preservation,
private socket and replay resync. The TUI's ordinary remote prompt still passed.

The held-followup witness still failed. Further source tracing supersedes the
previous global-tool-flag hypothesis: embedded defaults already enable native
tools. The actual missing wiring is the built-in Forge and Muse tool allowlists,
which omitted `followup`. Added the real tool to those interactive agents and a
parse/availability regression test; kept research-only Sage unchanged. No fixture
acceptance assertion was relaxed. Synthetic provider records only offered tool
names for bounded diagnosis.

The final synthetic policy witness appends only to fresh temporary files after
an actual permission grant. It requires no file before decision, exactly one
append after Accept, and no file after Reject, turn cancellation or TTL expiry.
Duplicate responses must fail. Followup qualification also checks a new external
controller recovers and answers the original pending request after lease handoff.
Build cache retention on failure is enabled for this dedicated free workflow,
with cargo-bin caching disabled; failed tests still fail the job.

## Graceful lifetime finalization

Source review found Drop-only shutdown could return from tokio main before the
actor completed its final save. Added awaited LiveControl shutdown at both UI
and detached-host entrypoints, plus cancellation during pre-stream setup. The
owner completion unit test requires the shutdown task to finish before return.
An isolated runtime witness exits the TUI while a question is pending, then
acquires a new owner and verifies the same persisted session/prompt is available.
No existing user process is involved.

At 0cfe4d1f0, PR head was confirmed current but no source Actions runs had been
created automatically (release-drafter/bot checks only). Explicit standard free
workflow dispatch is used for the final grouped revision; no paid runner or
required gate bypass is involved.

## Whole-command input negative control

Run 36588892471 / job 109476471043 now reached a real native followup and
printed its request UUID, but only the first `/` of the pasted `/respond ...`
command was consumed. The stronger oracle failed: the answer never reached the
original tool result. Source inspection identified the Rustyline 18.0.1 external
printer path polling stdin while bytes were already in its BufReader.

Official upstream fix: https://github.com/kkawakam/rustyline/commit/f2bbcc5cdf7b99dfe7d97af03976b111bc6fd256
(`Check input buffer before polling`). Pin that exact upstream revision and allow
only its official Git source in cargo-deny. Keep the single-write whole-command
PTY test; do not disguise the regression with byte-at-a-time fixture input.

Dedicated qualification now checks out the explicit PR head (manual dispatch uses
its exact event SHA) and runs Cargo with --locked. Earlier pull-request runs
checked GitHub's merge tree; the subsequent manual 0cfe/cac runs bind branch SHAs.
PR319 became conflicting with main, explaining absent automatic source CI; merge
main forward and requalify the resulting exact tree before delivery.
