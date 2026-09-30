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
