# forge_daemon architecture/existence finding

Date: 2026-09-30. Status: EXPERIMENTAL / UNMOUNTED.

Frozen source contains a substantial `forge_daemon` Rust/Zig C-ABI process-launch experiment, but no dependency/caller was found in `forge_main` or `forge_app`; the surfaced workspace consumer is `benchmarks/forge_daemon_bench`. It therefore does not participate in a mounted HeliosLite user journey today.

The benchmark is not acceptance-grade as written: the baseline launches M `/usr/bin/true` processes in parallel, while daemon dispatch loops M calls sequentially. Both arms use a micro-process rather than an actual Forge task. The benchmark can explore spawn overhead but cannot establish product TTFT, throughput, correctness, cancellation, RSS/PSS, or user benefit.

Before integration, #327 requires equivalent-concurrency matched workloads, real Forge task behavior, correctness/failure semantics, p50/p95/p99 latency and resources, supported-platform analysis and a simpler-process-path control.

Current disposition: preserve experiment, exclude from mature product implementation credit, and do not expand into the runtime until the existence experiment passes.