CREATE TABLE forge_daemon_binding (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    token TEXT NOT NULL CHECK (length(token) = 64)
);
INSERT INTO forge_daemon_binding (singleton, token) VALUES (1, lower(hex(randomblob(32))));
CREATE TRIGGER forge_daemon_binding_no_update BEFORE UPDATE ON forge_daemon_binding
BEGIN SELECT RAISE(ABORT, 'daemon database binding is immutable'); END;
CREATE TRIGGER forge_daemon_binding_no_delete BEFORE DELETE ON forge_daemon_binding
BEGIN SELECT RAISE(ABORT, 'daemon database binding is immutable'); END;
