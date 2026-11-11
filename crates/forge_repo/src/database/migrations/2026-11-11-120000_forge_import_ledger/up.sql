CREATE TABLE forge_import_ledger (
    conversation_id TEXT PRIMARY KEY NOT NULL,
    workspace_id BIGINT,
    origin TEXT NOT NULL,
    source_locator TEXT,
    source_digest TEXT,
    state TEXT NOT NULL CHECK (state IN ('imported', 'modified', 'deleted', 'present_unknown')),
    generation INTEGER NOT NULL DEFAULT 1 CHECK (typeof(generation) = 'integer' AND generation > 0)
);

-- Reusing an ID must not inherit an earlier row's provenance.
CREATE TRIGGER forge_import_record_insert
AFTER INSERT ON conversations
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM forge_import_ledger WHERE conversation_id = NEW.conversation_id
            AND workspace_id IS NOT NEW.workspace_id
    ) THEN RAISE(ABORT, 'conversation ID has a different or unknown ledger owner') END;
    UPDATE forge_import_ledger SET workspace_id = NEW.workspace_id,
        origin = 'unknown', state = 'present_unknown',
        source_locator = NULL, source_digest = NULL, generation = generation + 1
    WHERE conversation_id = NEW.conversation_id;
END;

-- Preserve provenance while recording local changes to user-visible content.
CREATE TRIGGER forge_import_record_update
AFTER UPDATE ON conversations
WHEN OLD.title IS NOT NEW.title OR OLD.context IS NOT NEW.context
  OR OLD.context_zstd IS NOT NEW.context_zstd OR OLD.is_compressed IS NOT NEW.is_compressed
  OR OLD.metrics IS NOT NEW.metrics OR OLD.parent_id IS NOT NEW.parent_id
  OR OLD.source IS NOT NEW.source OR OLD.cwd IS NOT NEW.cwd
  OR OLD.conversation_id IS NOT NEW.conversation_id OR OLD.workspace_id IS NOT NEW.workspace_id
BEGIN
    SELECT CASE WHEN (OLD.conversation_id IS NOT NEW.conversation_id OR OLD.workspace_id IS NOT NEW.workspace_id)
        AND EXISTS (SELECT 1 FROM forge_import_ledger WHERE conversation_id IN (OLD.conversation_id, NEW.conversation_id))
        THEN RAISE(ABORT, 'cannot reassign a conversation ID with ledger history') END;
    UPDATE forge_import_ledger SET state = 'modified', generation = generation + 1
    WHERE conversation_id = NEW.conversation_id AND state = 'imported';
END;

CREATE TRIGGER forge_import_record_delete
AFTER DELETE ON conversations
BEGIN
    SELECT CASE WHEN EXISTS (
        SELECT 1 FROM forge_import_ledger WHERE conversation_id = OLD.conversation_id
            AND workspace_id IS NOT OLD.workspace_id
    ) THEN RAISE(ABORT, 'conversation ID has a different or unknown ledger owner') END;
    INSERT INTO forge_import_ledger (conversation_id, workspace_id, origin, state)
    VALUES (OLD.conversation_id, OLD.workspace_id, 'unknown', 'deleted')
    ON CONFLICT(conversation_id) DO UPDATE SET
        origin = CASE WHEN forge_import_ledger.state IN ('imported', 'modified')
            AND forge_import_ledger.workspace_id IS OLD.workspace_id
            THEN forge_import_ledger.origin ELSE 'unknown' END,
        source_locator = CASE WHEN forge_import_ledger.state IN ('imported', 'modified')
            AND forge_import_ledger.workspace_id IS OLD.workspace_id
            THEN forge_import_ledger.source_locator ELSE NULL END,
        source_digest = CASE WHEN forge_import_ledger.state IN ('imported', 'modified')
            AND forge_import_ledger.workspace_id IS OLD.workspace_id
            THEN forge_import_ledger.source_digest ELSE NULL END,
        workspace_id = OLD.workspace_id,
        state = 'deleted', generation = forge_import_ledger.generation + 1;
END;
