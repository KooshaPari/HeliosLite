-- Deletion intent is user data. Automatic rollback must not discard it.
CREATE TEMP TABLE forge_import_ledger_rollback_requires_preservation (
    must_preserve INTEGER NOT NULL CHECK (must_preserve = 1)
);
INSERT INTO forge_import_ledger_rollback_requires_preservation VALUES (0);
