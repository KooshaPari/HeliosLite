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
