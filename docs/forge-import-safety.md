# Forge history reconciliation

Selected import previews one source conversation at a time. A write requires the
preview digest, acknowledgement that source identity and past history are
unknown, and explicit restoration when the destination records a deletion. The
digest covers the selected source row, destination workspace, local disposition,
visible collision count, and configured legacy path and acknowledgement state.
It does not establish the source installation's identity or full-fidelity conversion.

When the configured legacy path is absent, guarded operations fail unless
`HELIOSLITE_ACCEPT_MISSING_LEGACY` is set to that exact path for the process.
This is an explicit acceptance that past history is unknown, not proof that no
history existed. The path is checked again for each operation. A later appearance
before that check is refused; a filesystem change after the check or a file
replacement at the same name is not ruled out. A concurrent WAL writer can also
change legacy history while a read snapshot is held. Ordinary reads may still show a
local-only view when the configured legacy database cannot be attached.

Deletion through a current daemon uses the `DeleteWithLedger` request. Older
daemons do not recognize this request, and current daemons refuse older V2 delete
requests; other V2 mutations retain their existing protocol. A current daemon
refuses all deletion requests because it cannot verify attached legacy history.
Direct and bulk deletion also refuse when deleting a local row would reveal a
same-ID attached legacy conversation. A bulk forget dry run reports only a match
count; legacy availability and collision checks run during deletion. Direct deletion
of a missing ID records a tombstone only after the legacy guard succeeds. Daemon deletion is unavailable
until it can establish equivalent legacy visibility. Old clients talking to old
daemons do not have this protection. A workspace supplied over the socket is an
assertion; this protocol does not establish caller authorization.

The ledger migration refuses downgrade because removing it would lose deletion
history. These safeguards do not establish a stable source file identity, protect
against all older writers, or establish complete end-to-end migration safety.
