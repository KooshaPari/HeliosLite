# Recovered historical intent note — fork coexistence

Source: `feat/forge-dev-packaging-spec:docs/packaging/FORGE_DEV_PACKAGING.md`, inspected 2026-09-29.

The draft is owned by @KooshaPari and explicitly frames the fork as side-by-side with upstream: distinct binary, config/data root, update endpoint, release artifacts and installers. This is stronger historical intent evidence than generic fork naming, but it is a **draft historical specification**, not automatically current accepted naming.

Product-contract consequence: preserve a mature obligation for fork/upstream identity isolation and non-destructive coexistence unless later direct intent supersedes it. Do not blindly import the obsolete `forge-dev` spelling or external repository/URL proposals. Acceptance should prove that installing/running/updating the owned fork cannot silently execute or mutate upstream state, and vice versa.

This evidence strengthens the existence gate rather than bypassing it: if the owned fork survives, its artifact/config/runtime identity must be unambiguous. If upstream + thin adapter replaces it, migration must preserve owned state and not strand fork-specific configuration.
