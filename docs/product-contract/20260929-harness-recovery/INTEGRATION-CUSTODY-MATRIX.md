# Integration custody matrix — HeliosLite

Date: 2026-09-30. Status: OPEN; no destructive migration authorized.

| Surface | Current frozen implementation | Authority/journey finding | Candidate disposition | Blocking issue |
|---|---|---|---|---|
| AgilePlus | `forge_agileplus` mounted as top-level CLI; owns 31-pillar scorecard, sprint/velocity/grading logic | Accepted ADR establishes adoption/use, not Helios ownership; duplicates durable development-effort authority | EXTERNAL AUTHORITY + HELIOS PROJECTION/CLIENT | #324 |
| Tracera | `forge_tracera` substantial HTTP telemetry sink with auth/batching/retry/disk queue | Operational telemetry may be lossy; product acceptance/evidence cannot be best-effort or silently dropped | KEEP TELEMETRY ADAPTER WHERE USEFUL; STRICT ACKNOWLEDGED EVIDENCE ADAPTER SEPARATE | #325 |
| ShareCLI | `forge_sharecli` mounted as `helioslite share`; each command constructs its own hub | `publish` cannot reach subscribers in separate `serve`; `topics` always fresh/empty; `attach` uses another hub | SEPARATE SERVICE/PRODUCT OR VERSIONED CLIENT TO ONE LONG-LIVED RELAY; NO MATURE CREDIT TODAY | #323 |

## Accepted-versus-implementation rule
Historical/accepted adoption of an external system does not imply the worker runtime should reimplement that system's canonical state. Preserve useful projections and adapters; canonical effort/product/evidence truth remains with the designated external owner.

## Migration gate
Before extracting or replacing any embedded implementation, prove round-trip compatibility, preserve historical records, test unavailable/rejected external authority, and demonstrate that the thin adapter closes the same user journey. Until then, preserve code and mark the current authority contradiction explicitly.