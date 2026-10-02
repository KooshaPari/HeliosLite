# Mounted surface ledger — pass 1

Frozen source: `536a25cac1dc21ac97bbc86c7e9af74bd5932780`. Date: 2026-09-30.

This ledger distinguishes code presence from mounted/reachable product surfaces. A crate/binary is not automatically accepted mature scope.

| Surface | Entry evidence | Current classification | Next proof |
|---|---|---|---|
| Main CLI | `crates/forge_main/src/main.rs`, `cli.rs` | CURRENT / primary candidate surface | enumerate commands -> handlers -> actual services; smoke installed binary |
| TUI | `crates/forge_tui/src/main.rs` | CURRENT / human interface | journey map and terminal matrix |
| Tool execution | ToolRegistry -> ToolExecutor -> services | CURRENT / core spine | recovery reachability remains open |
| MCP client/server | forge_app/forge_infra/forge_services MCP modules | CURRENT / machine integration candidate | distinguish client, server and config-watch reachability; watcher failures open |
| DB daemon | `crates/forge_dbd/src/main.rs/server.rs` | CURRENT / persistence infrastructure | prove which CLI paths mount daemon vs repo fallback and ack atomicity |
| LSP | `crates/forge_lsp` | CURRENT / integration | watcher reload/debounce currently failing on macOS; do not call healthy |
| Desktop/Tauri | `desktop/src-tauri/src/main.rs/commands.rs` | PRESENT / scope authority unresolved | prove current product intent and mounted backend calls |
| Share CLI transport | `crates/forge_sharecli` with SSE/WS/e2e | PRESENT / integration candidate | identify real caller/command and auth/lifecycle |
| AgilePlus adapter | `crates/forge_agileplus/src/commands.rs` | PRESENT / adapter candidate | inspect whether mounted; do not infer authority from name |
| Helios bot | `crates/helios-bot` | PRESENT / auxiliary candidate | lineage/user-intent and deploy reachability |
| Forge3D | `crates/forge3d/src/main.rs/server.rs` | PRESENT / experimental candidate | establish accepted scope or exclude |
| Landing app | `apps/landing-helioslite` | AUXILIARY | product marketing, not runtime completion |
| Helper/tooling binaries | helioslite_helper, context-backfill, session-cleaner, vacuum | AUXILIARY/ops until journey requires them | mount/caller and release packaging |

Immediate rule: only surfaces with accepted intent + mounted caller + journey membership participate in mature product completion. Others remain PRESENT/UNRESOLVED.
