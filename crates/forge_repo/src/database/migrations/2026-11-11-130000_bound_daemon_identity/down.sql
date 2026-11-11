CREATE TEMP TABLE forge_daemon_binding_rollback_requires_preservation (
    must_preserve INTEGER NOT NULL CHECK (must_preserve = 1)
);
INSERT INTO forge_daemon_binding_rollback_requires_preservation VALUES (0);
