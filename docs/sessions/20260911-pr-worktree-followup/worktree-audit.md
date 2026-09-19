# Worktree cleanup audit

Audit date: 2026-09-11, approximately 11:38–11:52 UTC. Scope: the ten direct directories under `forgecode/.worktrees`. All ten are registered Git worktrees. Preserve all pending an explicit cleanup decision.

## Dispositions

| Directory | HEAD | Branch | Disposition and evidence |
| --- | --- | --- | --- |
| h017-ci | 00f40c4fcd30 | bundle/h017-ci | Conditional candidate. Clean including ignored entries. HEAD contained in cached fork/bundle/h017-ci and fork/main. No configured upstream. |
| h017-coverage | d43c060c2b24 | docs/h017-source-coverage-20260906 | Preserve uncommitted work. Two untracked files: `docs/consolidation/collect_inventory.py` and `docs/consolidation/source-inventory-20260907.json`. No tracked changes or ignored entries. HEAD contained in cached fork/docs/h017-source-coverage-20260906. No configured upstream. |
| h017-dist | 17e4e3493f33 | bundle/h017-dist | Conditional candidate. Clean including ignored entries. HEAD contained in cached fork/integration/forgecode-h0.1.7. No configured upstream. |
| h017-f3 | 00f40c4fcd30 | detached | Conditional candidate. Clean including ignored entries. HEAD contained in bundle/h017-ci and cached fork/main. No upstream. |
| h017-helios | 00f40c4fcd30 | detached | Conditional candidate. Clean including ignored entries. HEAD contained in bundle/h017-ci and cached fork/main. No upstream. |
| h017-integration | ec852a32bea4 | integration/forgecode-h0.1.7 | Preserve unique history. Clean including ignored entries, but one commit not reachable from other branch/remote/tag refs. Only containing named ref is its local branch. Upstream fork/main: ahead 1, behind 17. |
| h017-probe | 6ed5d37b6b45 | detached | Conditional candidate. Clean including ignored entries. HEAD contained in feat/dynamic-model-provider and cached origin/main. No upstream. |
| h017-release-reconcile | a9806dfef7ed | integration/h017-release-reconcile-20260907 | Conditional candidate. Clean including ignored entries. HEAD contained in cached fork/integration/forgecode-h0.1.7. No configured upstream. |
| h017-sandbox | c63a33a1dbd8 | bundle/h017-sandbox | Conditional candidate. Clean including ignored entries. HEAD contained in cached fork/integration/forgecode-h0.1.7. No configured upstream. |
| minimax-metadata | 50415749ae97 | fix/minimax-static-metadata | Conditional candidate. Clean including ignored entries. HEAD contained in cached fork/integration/forgecode-h0.1.7. No configured upstream. |

Totals: two preservation blockers and eight conditional cleanup candidates. All eight candidates have zero commits unique relative to other local branches, cached remote refs, and tags. This does not establish that their changes were merged into a particular main branch or that the worktrees are obsolete. Removing a worktree while retaining its branch is different from deleting that branch. No removal or ref operation was performed.

The unique integration commit is `ec852a32bea4`: `Merge remote-tracking branch 'fork/main' into integration/forgecode-h0.1.7`. Preserve its branch/history until reviewed. The coverage files were identified by filename only; their contents and secrets were not inspected.

## Activity evidence and limitations

- `lsof -nP -d cwd -Fpcn` showed no visible process working directory in any target worktree.
- `lsof -nP -Fpcn`, filtered to target paths, showed zero open-file matches. Both commands exited 0 without stderr.
- These are visibility-limited, point-in-time observations. No evidence of use is not proof of inactivity or obsolescence. Dormant editors, schedulers, remote users, or processes outside available visibility may still depend on these paths.
- Main checkout HEAD changed externally during the audit from `6ed5d37b6b45` to `ebf343a3a350`. The repository is live and audit results are not an atomic snapshot.
- Upstream reachability uses local cached refs only. No fetch was performed and remote existence/currentness is not established.
- The porcelain inventory had no locked/prunable indicators. Their absence does not prove inactivity.

## Validation and preservation

Read `AGENTS.md` first. Enumerated `git worktree list --porcelain` (41 worktrees repository-wide) and `.worktrees` directory entries. Inspected tracked/untracked status, ignored entries, HEAD and branch, configured upstream, ahead/behind counts, and named-ref containment using `git --no-optional-locks` / `GIT_OPTIONAL_LOCKS=0`. Unique commit counts exclude the worktree's own branch and compare against remaining heads, remote refs, and tags. For the integration HEAD, additionally checked containment across all named refs.

No prune, delete, reset, ref change, fetch, commit, or process termination was performed. No tests or lint were run because this is a read-only repository audit. This requested audit document is the only file written by this audit agent.

Before any cleanup: recheck status, ignored files, HEAD/ref reachability, and owner/process activity. Preserve the two coverage files and the unique integration history. Obtain explicit authorization for cleanup.

## Incidental external warning

Outside the ten-directory cleanup scope, `/private/tmp/forgecode-pr258-head` was detached at `23e416e79982`, clean, with three commits not reachable from branch/remote/tag refs and no named ref containing its HEAD. Preserve it pending a separate review. External worktrees were not classified as approved cleanup targets.
