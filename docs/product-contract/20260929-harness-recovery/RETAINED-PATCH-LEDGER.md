# HeliosLite retained-patch ledger — pass 1

Date: 2026-09-29. Comparison control: `tailcallhq/forgecode@be1dcb4717a4d3bd811478353c5b0de535891a15`; frozen owned source: `536a25cac1dc21ac97bbc86c7e9af74bd5932780`.

Git compare reports the owned source is 742 commits ahead and not behind that pinned upstream control. This establishes a large maintained delta, **not 742 justified differentiators**. The diff includes governance, CI/release machinery, branding/landing assets, requirements/docs, dependencies and product code.

## First dispositions

- CI/release/governance/branding: supporting product/repository operations; not by themselves a reason for a fork.
- Historical side-by-side fork identity/config isolation: accepted historical intent candidate; can often be implemented as packaging/config overlay, so not automatically a deep-core differentiator.
- Generic provider breadth, MCP, CLI modes, semantic search: commodity/contested by upstream.
- Benchmark/evidence hardening in #322: candidate generally useful patch; prefer upstream contribution or thin retained patch if it survives native tests.
- Pheno/Tracera/AgilePlus integration, native Windows/Pine behavior, and any provider/subscription semantics absent upstream: candidate owned differentiation; still source/matched-test unverified.

## Decision rule

Reduce the 742-commit delta into behavioral obligations and classify each as UPSTREAMED/COMMODITY, CONTRIBUTE UPSTREAM, THIN OVERLAY/ADAPTER, RETAIN PATCH, or REJECT/SUPERSEDED. Generated docs, branch-protection files, release plumbing and branding do not inflate retained behavioral value.

Because this fork is ahead rather than thousands of commits behind the pinned upstream control, maintenance pressure has a different shape from KCode. That does not establish current upstream freshness: a later upstream head/release must also be frozen before the existence gate closes.

## Owned integration modules — candidate, not yet differentiation
Frozen HeliosLite contains owned workspace crates `forge_agileplus`, `forge_tracera` and `forge_sharecli`; the pinned upstream control does not surface those names. Their manifests describe delivery-quality scoring, Tracera telemetry transport, and ShareCLI relay behavior. This establishes owned implementation surface, **not that these concerns belong inside the coding CLI core**. The existence gate must compare in-core crates against external adapters/services using the same interfaces. If the behavior composes cleanly outside core, classify THIN OVERLAY/ADAPTER rather than RETAIN PATCH.

## Fork-only crate audit — pass 1

Exact crate-name comparison against pinned current Forgecode main finds 29 owned crate names absent upstream. Selected high-impact source review:

- `forge_agileplus`: describes itself as a Rust implementation of the AgilePlus delivery-quality OS, including a 31-pillar scorecard, sprint records and velocity predictor. **Architecture red flag:** the current user mission assigns AgilePlus as an external durable-development-effort system, not something HeliosLite should silently re-own. Default disposition: REJECT CORE DUPLICATION / ADAPTER CANDIDATE until a real consumer proves an embedded subset is necessary.
- `forge_tracera`: defines a `tracera.v1` outbound telemetry wire format, auth rotation, batching and persistent retry. **Architecture red flag:** Tracera is the canonical product/evidence graph, not merely an observability sink. This local wire contract must be reconciled against actual Tracera APIs; do not treat self-declared `canonical` wire types as accepted Tracera authority. Default: ADAPTER/PROPOSAL pending matched consumer.
- `forge_pheno_shell`: concrete multi-shell detection/completion abstraction including Windows-native, WSL and Git Bash. Candidate utility, but Pine owns the broader POSIX/Windows translation problem; do not expand this into a competing Pine implementation.
- `forge_pheno_winterminal`: concrete Windows Terminal profile/theme/config management. User-experience utility, not core coding-agent differentiation; likely optional adapter/plugin.
- `forge_sdk`: high-level re-export wrapper over Forge API/domain/config. Generic SDK surface is commodity/contested; reconcile against Agentora/HeliosCLI consumers before retention.
- `forge_daemon`: Zig kqueue/posix_spawn hot-path/daemon experiment. Performance differentiation requires matched benchmark and platform fallback; do not freeze architecture around it from comments alone.
- `forge_dbd`: single-writer SQLite daemon with client/protocol. Requires proof that a separate write daemon improves correctness/performance over simpler SQLite/WAL ownership; candidate custom subsystem under bootstrap challenge.
- `forge_guardian`: risk scoring layer feeding policy allow/deny/confirm. Security architecture must not delegate authorization to an LLM score. Keep deterministic authority separate; evaluate OPA/capability-policy prior art.
- `forge_sandbox`: docs claim OS isolation but explicitly call Windows Job Object/restricted-token backend a placeholder and Landlock is feature-gated. A sandbox API existing is not evidence of enforced isolation. Treat unsupported/disabled backends as non-green.
- `forge_cloud`: generic Cloudflare/local task dispatch abstraction. Commodity/contested; reject custom ownership absent a Helios-specific requirement not met by existing durable/task runtimes.

Other owned-only crates (`forge3d`, audit, drift, graph, mux, paste, plugin, render, repo_map, semantic/similarity/syntax, ShareCLI, TUI, ghostty-kit, helios-bot, etc.) remain source-classification work. No crate count is a differentiation count.

Key correction: earlier name searches suggested no AgilePlus/Tracera/Pine-adjacent implementation. Exact tree/crate inspection falsifies that for HeliosLite: named AgilePlus/Tracera and shell/Windows-terminal implementations do exist. Their existence does **not** establish accepted ownership; it raises a duplication/custody question.
## Mountedness correction — selected Helios surfaces

Selected manifest/caller tracing shows several high-impact owned-only crates are not merely dormant workspace members:
- `forge_agileplus` is a direct `forge_main` dependency and `TopLevelCommand::Agileplus` executes its command engine before normal UI startup. This is mounted duplicate product logic, not just a library proposal.
- `forge_tracera` is wired through `forge_main::telemetry::TraceraTelem`; main constructs it and lifecycle/command/error events can be submitted. However the bridge explicitly treats invalid config and delivery failures as disabled/best-effort. **Therefore it is telemetry, not admissible acceptance evidence.** A failed Tracera sink must never be interpreted as accepted product trace completion.
- `forge_sandbox` is a `forge_app` dependency, but the separately inspected crate documents platform gaps. Reachability does not establish enforcement.
- `forge_dbd` is a `forge_repo` dependency and advertises `FORGE_DBD_ENABLED`; actual runtime selection/recovery still needs caller tracing.
- `forge_pheno_shell` is a `forge_infra` dependency. `forge_pheno_winterminal`, `forge_sdk`, `forge_daemon`, `forge_guardian`, and `forge_cloud` require further mounted-call tracing; root workspace membership alone is insufficient.

Architecture consequence: `forge_agileplus` should not be allowed to become a second authoritative AgilePlus implementation. Preserve any useful CLI projection or scorecard code as an adapter/consumer candidate, but authority and durable work state stay external. `forge_tracera` may remain best-effort observability, but accepted evidence requires a separate acknowledgement/receipt path whose failure is non-green.