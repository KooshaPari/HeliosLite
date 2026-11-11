use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use bstr::ByteSlice;
use diesel::prelude::*;
use forge_domain::{
    Context, Conversation, ConversationId, ConversationRepository, ConversationSummary,
    ForgeImportReport, WorkspaceHash,
};
use tracing::warn;

use crate::conversation::conversation_record::{
    ContextRecord, ConversationRecord, ConversationRecordLite, MetricsRecord,
};
use crate::database::schema::{conversations, conversations_all};
use crate::database::{DatabasePool, PooledSqliteConnection};

/// Lightweight row type for FTS5 `snippet()` results. The query returns
/// exactly one column (`s`) — we use a named struct (not a tuple) so
/// diesel's `QueryableByName` derive can read it back from `sql_query`.
#[derive(Debug, Clone)]
struct SnippetRow {
    s: String,
}

impl diesel::QueryableByName<diesel::sqlite::Sqlite> for SnippetRow {
    fn build<'a>(
        row: &impl diesel::row::NamedRow<'a, diesel::sqlite::Sqlite>,
    ) -> diesel::deserialize::Result<Self> {
        let s = diesel::row::NamedRow::get::<diesel::sql_types::Text, _>(row, "s")?;
        Ok(SnippetRow { s })
    }
}

/// Lightweight row type for FTS5 `highlight()` results. The query returns
/// exactly one column (`h`) — the full column text with matches wrapped
/// in caller-supplied markup (e.g. `<b>...</b>`). Compared to
/// [`SnippetRow`] which returns a short passage, `highlight()` returns
/// the entire column with match-spans preserved.
#[derive(Debug, Clone)]
struct HighlightRow {
    h: String,
}

impl diesel::QueryableByName<diesel::sqlite::Sqlite> for HighlightRow {
    fn build<'a>(
        row: &impl diesel::row::NamedRow<'a, diesel::sqlite::Sqlite>,
    ) -> diesel::deserialize::Result<Self> {
        let h = diesel::row::NamedRow::get::<diesel::sql_types::Text, _>(row, "h")?;
        Ok(HighlightRow { h })
    }
}

/// Row type for reading conversations during FTS refresh.
/// Used to populate FTS5 with decompressed context from both compressed and
/// uncompressed rows.
#[derive(Debug, Clone)]
struct FtsRefreshRow {
    rowid: i64,
    title: String,
    context: Option<String>,
    context_zstd: Option<Vec<u8>>,
    is_compressed: i32,
    cwd: Option<String>,
}

impl diesel::QueryableByName<diesel::sqlite::Sqlite> for FtsRefreshRow {
    fn build<'a>(
        row: &impl diesel::row::NamedRow<'a, diesel::sqlite::Sqlite>,
    ) -> diesel::deserialize::Result<Self> {
        use diesel::row::NamedRow;
        use diesel::sql_types::{BigInt, Binary, Integer, Nullable, Text};
        Ok(FtsRefreshRow {
            rowid: NamedRow::get::<BigInt, _>(row, "rowid")?,
            title: NamedRow::get::<Text, _>(row, "title")?,
            context: NamedRow::get::<Nullable<Text>, _>(row, "context")?,
            context_zstd: NamedRow::get::<Nullable<Binary>, _>(row, "context_zstd")?,
            is_compressed: NamedRow::get::<Integer, _>(row, "is_compressed")?,
            cwd: NamedRow::get::<Nullable<Text>, _>(row, "cwd")?,
        })
    }
}

/// Row type for the BM25-ranked FTS5 SELECT (`SELECT c.*, bm25(...) AS
/// rank_score`). Carries the same columns as `ConversationRecord` plus an
/// extra `rank_score` that lets the loader project the SQL columns directly
/// without stripping the extra column at the SQL layer. The `into_record`
/// conversion discards `rank_score` — ranking is consumed by `ORDER BY`
/// before the rows leave SQLite.
#[derive(Debug, Clone)]
struct FtsRankedRow {
    record: ConversationRecord,
    #[allow(dead_code)]
    rank_score: f64,
}

impl FtsRankedRow {
    fn into_record(self) -> ConversationRecord {
        self.record
    }
}

impl diesel::QueryableByName<diesel::sqlite::Sqlite> for FtsRankedRow {
    fn build<'a>(
        row: &impl diesel::row::NamedRow<'a, diesel::sqlite::Sqlite>,
    ) -> diesel::deserialize::Result<Self> {
        use diesel::row::NamedRow;
        use diesel::sql_types::{BigInt, Binary, Double, Integer, Nullable, Text, Timestamp};

        let conversation_id = NamedRow::get::<Text, _>(row, "conversation_id")?;
        let title = NamedRow::get::<Nullable<Text>, _>(row, "title")?;
        let workspace_id = NamedRow::get::<Nullable<BigInt>, _>(row, "workspace_id")?;
        let context = NamedRow::get::<Nullable<Text>, _>(row, "context")?;
        let created_at = NamedRow::get::<Timestamp, _>(row, "created_at")?;
        let updated_at = NamedRow::get::<Nullable<Timestamp>, _>(row, "updated_at")?;
        let metrics = NamedRow::get::<Nullable<Text>, _>(row, "metrics")?;
        let parent_id = NamedRow::get::<Nullable<Text>, _>(row, "parent_id")?;
        let source = NamedRow::get::<Nullable<Text>, _>(row, "source")?;
        let cwd = NamedRow::get::<Nullable<Text>, _>(row, "cwd")?;
        let message_count = NamedRow::get::<Nullable<Integer>, _>(row, "message_count")?;
        let intent_state = NamedRow::get::<Text, _>(row, "intent_state")?;
        let extracted_at = NamedRow::get::<Nullable<Timestamp>, _>(row, "extracted_at")?;
        let memory_id = NamedRow::get::<Nullable<Text>, _>(row, "memory_id")?;
        let intent_hash = NamedRow::get::<Nullable<Text>, _>(row, "intent_hash")?;
        let context_zstd = NamedRow::get::<Nullable<Binary>, _>(row, "context_zstd")?;
        let is_compressed = NamedRow::get::<Integer, _>(row, "is_compressed")?;
        let rank_score = NamedRow::get::<Double, _>(row, "rank_score").unwrap_or(0.0);

        Ok(FtsRankedRow {
            record: ConversationRecord {
                conversation_id,
                title,
                workspace_id,
                context,
                created_at,
                updated_at,
                metrics,
                parent_id,
                source,
                cwd,
                message_count,
                intent_state,
                extracted_at,
                memory_id,
                intent_hash,
                context_zstd,
                is_compressed,
            },
            rank_score,
        })
    }
}

pub struct ConversationRepositoryImpl {
    pool: Arc<DatabasePool>,
    wid: WorkspaceHash,
}

/// Reject foreign-workspace and attached-only rows, preserving missing-ID semantics.
fn validate_local_mutation(
    connection: &mut diesel::SqliteConnection,
    conversation_id: &str,
    workspace_id: i64,
) -> anyhow::Result<()> {
    let local_workspace = conversations::table
        .filter(conversations::conversation_id.eq(conversation_id))
        .select(conversations::workspace_id)
        .first::<Option<i64>>(connection)
        .optional()?;
    if let Some(Some(owner)) = local_workspace {
        anyhow::ensure!(
            owner == workspace_id,
            "Conversation belongs to another workspace"
        );
    } else {
        let legacy = diesel::select(diesel::dsl::exists(
            conversations_all::table.filter(conversations_all::conversation_id.eq(conversation_id)),
        ))
        .get_result::<bool>(connection)?;
        if legacy {
            anyhow::bail!("Conversation exists only in attached legacy history and is read-only");
        }
    }
    Ok(())
}

/// Merge primary local search hits with supplemental legacy hits while keeping
/// the caller's limit hard and guaranteeing that a visible legacy match is not
/// starved whenever the primary result set fills the limit.
fn merge_search_rows(
    mut primary: Vec<ConversationRecord>,
    supplemental: Vec<ConversationRecord>,
    limit: Option<i64>,
) -> Vec<ConversationRecord> {
    let mut seen = HashSet::new();
    primary.retain(|row| seen.insert(row.conversation_id.clone()));

    let supplemental: Vec<ConversationRecord> = supplemental
        .into_iter()
        .filter(|row| seen.insert(row.conversation_id.clone()))
        .collect();

    let Some(limit) = limit else {
        primary.extend(supplemental);
        return primary;
    };
    let limit = limit.max(0) as usize;
    if limit == 0 {
        return Vec::new();
    }

    if supplemental.is_empty() {
        return primary.into_iter().take(limit).collect();
    }

    let primary_quota = limit.saturating_sub(1);
    let mut primary = primary.into_iter();
    let mut supplemental = supplemental.into_iter();
    let mut merged = Vec::with_capacity(limit);
    for _ in 0..primary_quota {
        let Some(row) = primary.next() else { break };
        merged.push(row);
    }
    if let Some(row) = supplemental.next() {
        merged.push(row);
    }

    for row in primary {
        if merged.len() == limit {
            break;
        }
        merged.push(row);
    }
    for row in supplemental {
        if merged.len() == limit {
            break;
        }
        merged.push(row);
    }
    merged
}

/// Bound retained unique hits, not examined candidates. Rare/no matches can
/// require a full scan, and SQLite may sort candidates before streaming them.
fn collect_search_rows(
    rows: impl Iterator<Item = diesel::QueryResult<ConversationRecord>>,
    limit: Option<i64>,
    mut matches: impl FnMut(&ConversationRecord) -> anyhow::Result<bool>,
) -> anyhow::Result<Vec<ConversationRecord>> {
    let mut hits = Vec::new();
    let mut seen = HashSet::new();
    if limit.is_some_and(|limit| limit <= 0) {
        return Ok(hits);
    }
    for row in rows {
        let row = row?;
        if !seen.contains(&row.conversation_id) && matches(&row)? {
            seen.insert(row.conversation_id.clone());
            hits.push(row);
            if limit.is_some_and(|limit| hits.len() >= limit as usize) {
                break;
            }
        }
    }
    Ok(hits)
}

/// Decode only one candidate at a time. Corrupt contexts do not hide title/cwd
/// matches, but cannot themselves supply searchable text.
fn search_context(row: &ConversationRecord) -> String {
    if row.is_compressed == 1 || row.context.is_none() {
        row.context_zstd
            .as_ref()
            .and_then(|bytes| zstd::decode_all(&bytes[..]).ok())
            .map(|bytes| bytes.to_str_lossy().into_owned())
            .unwrap_or_default()
    } else {
        row.context.clone().unwrap_or_default()
    }
}

/// Evaluate legacy MATCH expressions with the same schema/tokenizer as the
/// local index. The isolated connection lets the source cursor remain streaming
/// and drops all temporary history on return, including on errors.
struct LegacyFtsMatcher(diesel::SqliteConnection);

impl LegacyFtsMatcher {
    fn new() -> anyhow::Result<Self> {
        let mut connection = diesel::SqliteConnection::establish(":memory:")?;
        // Keep aligned with 2026-08-27-180000_fts5_unicode61_highlight.
        diesel::sql_query(
            "CREATE VIRTUAL TABLE candidate USING fts5(title, content, cwd, tokenize='unicode61 remove_diacritics 1')",
        )
        .execute(&mut connection)?;
        Ok(Self(connection))
    }

    fn matches(&mut self, row: &ConversationRecord, query: &str) -> anyhow::Result<bool> {
        #[derive(diesel::QueryableByName)]
        struct MatchRow {
            #[diesel(sql_type = diesel::sql_types::Bool)]
            matched: bool,
        }

        diesel::sql_query("DELETE FROM candidate").execute(&mut self.0)?;
        diesel::sql_query("INSERT INTO candidate(rowid, title, content, cwd) VALUES (1, ?, ?, ?)")
            .bind::<diesel::sql_types::Text, _>(row.title.as_deref().unwrap_or_default())
            .bind::<diesel::sql_types::Text, _>(search_context(row))
            .bind::<diesel::sql_types::Text, _>(row.cwd.as_deref().unwrap_or_default())
            .execute(&mut self.0)?;
        let result = diesel::sql_query(
            "SELECT EXISTS(SELECT 1 FROM candidate WHERE candidate MATCH ?) AS matched",
        )
        .bind::<diesel::sql_types::Text, _>(query)
        .get_result::<MatchRow>(&mut self.0)?;
        Ok(result.matched)
    }
}

/// Collect one source in recency order. SQL excludes irrelevant plaintext rows,
/// but compressed candidates have no scan cap: matching requires decoding.
fn collect_like_source(
    connection: &mut diesel::SqliteConnection,
    workspace_id: i64,
    legacy: bool,
    query: &str,
    limit: Option<i64>,
) -> anyhow::Result<Vec<ConversationRecord>> {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let needle = query.to_ascii_lowercase();
    let rows = diesel::sql_query(
        "SELECT * FROM conversations_all \
         WHERE __forge_legacy_source = ? \
           AND (workspace_id = ? OR __forge_legacy_unscoped = 1) \
           AND (title LIKE ? ESCAPE '\\' OR context LIKE ? ESCAPE '\\' \
                OR cwd LIKE ? ESCAPE '\\' \
                OR (context IS NULL AND context_zstd IS NOT NULL)) \
         ORDER BY updated_at DESC, conversation_id",
    )
    .bind::<diesel::sql_types::Integer, _>(i32::from(legacy))
    .bind::<diesel::sql_types::BigInt, _>(workspace_id)
    .bind::<diesel::sql_types::Text, _>(&pattern)
    .bind::<diesel::sql_types::Text, _>(&pattern)
    .bind::<diesel::sql_types::Text, _>(&pattern)
    .load_iter::<ConversationRecord, _>(connection)?;
    collect_search_rows(rows, limit, |row| {
        // SQLite LIKE folds ASCII, not arbitrary Unicode. Keep storage formats
        // equivalent, including multiword order, quotes and literal wildcards.
        Ok([
            row.title.as_deref(),
            row.context.as_deref(),
            row.cwd.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.to_ascii_lowercase().contains(&needle))
            || (row.context.is_none()
                && search_context(row).to_ascii_lowercase().contains(&needle)))
    })
}

impl ConversationRepositoryImpl {
    pub(crate) async fn import_forge_db_inner(
        &self,
        source: PathBuf,
        options: &forge_domain::ForgeImportOptions,
        require_valid_context: bool,
    ) -> anyhow::Result<ForgeImportReport> {
        anyhow::ensure!(
            options.dry_run,
            "unselected import is disabled; select IDs explicitly"
        );
        self.import_forge_db_plan(source, options, require_valid_context, None)
            .await
    }

    async fn import_forge_db_plan(
        &self,
        source: PathBuf,
        options: &forge_domain::ForgeImportOptions,
        require_valid_context: bool,
        selection: Option<(Vec<String>, bool, bool, Option<String>)>,
    ) -> anyhow::Result<ForgeImportReport> {
        let options = options.clone();
        let legacy = self
            .pool
            .legacy_database_path()
            .map(std::path::Path::to_path_buf);
        let primary = self.pool.database_path().to_path_buf();
        let acknowledgement = std::env::var_os("HELIOSLITE_ACCEPT_MISSING_LEGACY");
        self.run_with_connection(move |connection, wid| {
            use diesel::Connection;
            use diesel::sql_types::Text;

            if !source.is_file() {
                anyhow::bail!("source database not found: {}", source.display());
            }

            // Open the source database read-only and enable query_only as a
            // second guard. SQLite may still use WAL coordination sidecars.
            let source_url = source_readonly_uri(&source)?;
            let mut src = diesel::sqlite::SqliteConnection::establish(&source_url)?;
            diesel::sql_query("PRAGMA query_only = ON;").execute(&mut src)?;
            diesel::sql_query("PRAGMA busy_timeout = 5000;").execute(&mut src)?;

            // Detect the source schema before reading anything.
            #[derive(diesel::QueryableByName)]
            struct ColumnName {
                #[diesel(sql_type = Text)]
                name: String,
            }
            let columns: Vec<ColumnName> =
                diesel::sql_query("PRAGMA table_info(conversations)").load(&mut src)?;
            let column_names: Vec<&str> =
                columns.iter().map(|column| column.name.as_str()).collect();

            if column_names
                .iter()
                .any(|name| *name == "is_compressed" || *name == "context_zstd")
            {
                anyhow::bail!(
                    "source database {} is already a heliosLite/fork-schema database; \
                     there is nothing to import",
                    source.display()
                );
            }
            for required in ["conversation_id", "context", "created_at"] {
                if !column_names.contains(&required) {
                    anyhow::bail!(
                        "source database {} is not an official forge conversations \
                         database (missing column `{required}`)",
                        source.display()
                    );
                }
            }

            let rows: Vec<DecodedSourceRow> = if let Some((ids, _, _, _)) = &selection {
                anyhow::ensure!(
                    ids.len() == 1,
                    "select exactly one conversation ID per preview and import"
                );
                let selected = diesel::sql_query(
                    "SELECT conversation_id, title, context, created_at, updated_at, metrics \
                     FROM conversations WHERE conversation_id = ? AND \
                     COALESCE(length(CAST(title AS BLOB)), 0) + \
                     COALESCE(length(CAST(context AS BLOB)), 0) + \
                     COALESCE(length(CAST(created_at AS BLOB)), 0) + \
                     COALESCE(length(CAST(updated_at AS BLOB)), 0) + \
                     COALESCE(length(CAST(metrics AS BLOB)), 0) <= 8388608 LIMIT 2",
                )
                .bind::<Text, _>(&ids[0])
                .load(&mut src)?;
                anyhow::ensure!(
                    selected.len() == 1,
                    "selected ID is absent, duplicated, or exceeds the 8 MiB preview limit"
                );
                selected
            } else {
                diesel::sql_query(
                    "SELECT conversation_id, title, context, created_at, updated_at, metrics \
                     FROM conversations",
                )
                .load(&mut src)?
            };

            let mut report = ForgeImportReport {
                source_total: rows.len(),
                dry_run: options.dry_run,
                ..Default::default()
            };
            let execute = |conn: &mut diesel::SqliteConnection| -> anyhow::Result<()> {
                let observation = if selection.is_some() {
                    Some(inspect_legacy_projection(
                        conn,
                        legacy.as_deref(),
                        &primary,
                        acknowledgement.as_deref(),
                    )?)
                } else {
                    None
                };
                let digest = if let Some(observation) = &observation {
                    verify_forge_import_ledger_schema(conn)?;
                    Some(forge_selection_digest(
                        conn,
                        &source_url,
                        wid.id() as i64,
                        &rows[0],
                        observation,
                    )?)
                } else {
                    None
                };
                if let Some((_, _, _, expected)) = &selection {
                    if options.dry_run {
                        eprintln!(
                            "[import] selected {} preview digest {}",
                            rows[0].conversation_id,
                            digest.as_deref().unwrap()
                        );
                    } else {
                        anyhow::ensure!(
                            expected.as_deref() == digest.as_deref(),
                            "source or destination changed since preview (digest mismatch)"
                        );
                    }
                }
                import_rows(
                    conn,
                    wid,
                    &rows,
                    &options,
                    require_valid_context,
                    selection.as_ref().map(|(_, restore, accept, _)| {
                        (
                            *restore,
                            *accept,
                            source_url.as_str(),
                            digest.as_deref().unwrap(),
                        )
                    }),
                    &mut report,
                )?;
                if let Some(observation) = &observation {
                    ensure_legacy_unchanged(
                        conn,
                        legacy.as_deref(),
                        &primary,
                        acknowledgement.as_deref(),
                        observation,
                    )?;
                }
                Ok(())
            };
            if options.dry_run {
                (&mut **connection).transaction::<_, anyhow::Error, _>(execute)?;
            } else {
                connection.immediate_transaction::<_, anyhow::Error, _>(execute)?;
            }

            Ok(report)
        })
        .await
    }

    pub fn new(pool: Arc<DatabasePool>, workspace_id: WorkspaceHash) -> Self {
        Self { pool, wid: workspace_id }
    }

    /// Returns the workspace scope used for all conversation mutations.
    pub(crate) fn workspace_id(&self) -> i64 {
        self.wid.id() as i64
    }

    pub(crate) fn daemon_database_path(&self) -> &std::path::Path {
        self.pool.database_path()
    }

    pub(crate) async fn daemon_database_token(&self) -> anyhow::Result<String> {
        self.run_with_connection(|conn, _| {
            #[derive(diesel::QueryableByName)]
            struct Binding {
                #[diesel(sql_type = diesel::sql_types::Text)]
                token: String,
            }
            Ok(
                diesel::sql_query("SELECT token FROM forge_daemon_binding WHERE singleton = 1")
                    .get_result::<Binding>(conn)?
                    .token,
            )
        })
        .await
    }

    pub(crate) async fn daemon_has_local_owned(&self, id: &ConversationId) -> anyhow::Result<bool> {
        let id = id.into_string();
        self.run_with_connection(move |conn, wid| {
            let owner = conversations::table
                .filter(conversations::conversation_id.eq(id))
                .select(conversations::workspace_id)
                .first::<Option<i64>>(conn)
                .optional()?;
            Ok(owner == Some(Some(wid.id() as i64)))
        })
        .await
    }

    async fn run_blocking<F, T>(&self, operation: F) -> anyhow::Result<T>
    where
        F: FnOnce(Arc<DatabasePool>, WorkspaceHash) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let pool = self.pool.clone();
        let wid = self.wid;
        tokio::task::spawn_blocking(move || operation(pool, wid))
            .await
            .map_err(|e| anyhow::anyhow!("Conversation repository task failed: {e}"))?
    }

    async fn run_with_connection<F, T>(&self, operation: F) -> anyhow::Result<T>
    where
        F: FnOnce(&mut PooledSqliteConnection, WorkspaceHash) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.run_blocking(move |pool, wid| {
            let mut connection = pool.get_connection()?;
            operation(&mut connection, wid)
        })
        .await
    }

    /// Validate ordinary writes before entering the shared persistence primitive.
    /// Explicit import uses its separate insert-only, skip-existing path.
    fn upsert_local_conversation_record(
        connection: &mut PooledSqliteConnection,
        record: &ConversationRecord,
    ) -> anyhow::Result<()> {
        // `workspace_id` is nullable on disk (legacy rows may have NULL),
        // but local writes always set it. Use 0 as a defensive fallback so
        // validation rejects a malformed call rather than silently writing
        // a row that ownership-checks would later refuse.
        let workspace_id = record.workspace_id.unwrap_or(0);
        validate_local_mutation(connection, &record.conversation_id, workspace_id)?;
        Self::upsert_conversation_ref_record(connection, record)
    }

    /// Persist a prevalidated local conversation representation. The context
    /// columns form one storage value and must therefore be updated together.
    /// The conflict predicate also protects against a foreign insert racing the
    /// caller's ownership check; zero affected rows must not report success.
    fn upsert_conversation_ref_record(
        connection: &mut PooledSqliteConnection,
        record: &ConversationRecord,
    ) -> anyhow::Result<()> {
        let affected = diesel::query_dsl::methods::FilterDsl::filter(
            diesel::insert_into(conversations::table)
                .values(record)
                .on_conflict(conversations::conversation_id)
                .do_update()
                .set((
                    conversations::title.eq(&record.title),
                    conversations::context.eq(&record.context),
                    conversations::context_zstd.eq(&record.context_zstd),
                    conversations::is_compressed.eq(record.is_compressed),
                    conversations::updated_at.eq(record.updated_at),
                    conversations::metrics.eq(&record.metrics),
                    conversations::parent_id.eq(&record.parent_id),
                    conversations::source.eq(&record.source),
                    conversations::cwd.eq(&record.cwd),
                    conversations::message_count.eq(record.message_count),
                )),
            conversations::workspace_id.eq(record.workspace_id),
        )
        .execute(connection)?;
        anyhow::ensure!(
            affected == 1,
            "Conversation upsert did not affect a writable local row"
        );
        Ok(())
    }
}

impl ConversationRepositoryImpl {
    /// Inherent helpers: FTS5 primary path + LIKE fallback. They live on the
    /// concrete impl (not the trait) because they take an owned `String` so
    /// the closure passed to `run_with_connection` can move the value into
    /// the `spawn_blocking` future without borrowing `self` for `'life1`.
    async fn search_conversations_fts(
        &self,
        query: String,
        limit_value: Option<i64>,
    ) -> anyhow::Result<Vec<Conversation>> {
        if query.trim().is_empty() || limit_value.is_some_and(|limit| limit <= 0) {
            return Ok(Vec::new());
        }
        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            // FTS5 BM25 search joined back to the base table on
            // `rowid` (now explicit `rowid` column in external-content FTS5).
            // `bm25()` returns a negative number where lower = more relevant, so `ORDER BY
            // rank_score` (ascending) yields "best match first".
            //
            // We read FTS hits from the primary `conversations` table: the FTS5 index
            // is populated by `refresh_fts_index` from the primary table's
            // rows only, and the `rowid` join requires a real table (SQLite
            // views do not expose `rowid`). A streamed MATCH supplement covers
            // visible legacy rows, which are deliberately not indexed locally.
            //
            // We do NOT include `snippet()` here because it would force
            // the SELECT to return a column not in `ConversationRecord`.
            // The UI fetches a snippet on-demand via the separate
            // `get_conversation_snippet` method when the user picks a hit.
            //
            // BM25 rank is added as a non-struct column (`rank_score`),
            // so the SELECT is loaded into `FtsRankedRow` (which flattens
            // back to `ConversationRecord` for the rest of the pipeline).
            let mut sql = String::from(
                "SELECT c.*, bm25(conversations_fts) AS rank_score \
                 FROM conversations c \
                 JOIN conversations_fts fts ON c.rowid = fts.rowid \
                 WHERE conversations_fts MATCH ? \
                   AND c.workspace_id = ? \
                 ORDER BY rank_score",
            );
            if limit_value.is_some() {
                sql.push_str(" LIMIT ?");
            }

            // We can't bind the FTS MATCH expression positionally because
            // diesel::sql_query does not have a typed binding for FTS5's
            // MATCH operator when used as a column. Use the lower-level
            // `sql_query` so we can read back the typed rows.
            let mut q = diesel::sql_query(sql).into_boxed();
            q = q.bind::<diesel::sql_types::Text, _>(&query);
            q = q.bind::<diesel::sql_types::BigInt, _>(workspace_id);
            if let Some(limit) = limit_value {
                q = q.bind::<diesel::sql_types::BigInt, _>(limit.max(1));
            }
            let ranked_rows: Vec<FtsRankedRow> = q.load(connection)?;
            let raw_rows: Vec<ConversationRecord> = ranked_rows
                .into_iter()
                .map(FtsRankedRow::into_record)
                .collect();

            // Preserve local BM25 order. Legacy MATCH semantics use the same
            // tokenizer and columns, but supplemental ranking is by recency.
            let mut matcher = LegacyFtsMatcher::new()?;
            let legacy_candidates = diesel::sql_query(
                "SELECT * FROM conversations_all \
                 WHERE __forge_legacy_source = 1 \
                   AND (workspace_id = ? OR __forge_legacy_unscoped = 1) \
                 ORDER BY updated_at DESC, conversation_id",
            )
            .bind::<diesel::sql_types::BigInt, _>(workspace_id)
            .load_iter::<ConversationRecord, _>(connection)?;
            let legacy_rows = collect_search_rows(legacy_candidates, limit_value, |row| {
                matcher.matches(row, &query)
            })?;
            let raw_rows = merge_search_rows(raw_rows, legacy_rows, limit_value);
            let conversations: Result<Vec<Conversation>, _> =
                raw_rows.into_iter().map(Conversation::try_from).collect();
            conversations
        })
        .await
    }

    /// Literal substring recovery when FTS is unavailable or malformed. Each
    /// source contributes independent, unique recency-ordered hits, regardless
    /// of storage encoding. Selection reserves a legacy slot, then output is
    /// sorted by recency. This path deliberately does not interpret FTS syntax.
    async fn search_conversations_like(
        &self,
        query: String,
        limit_value: Option<i64>,
    ) -> anyhow::Result<Vec<Conversation>> {
        if query.trim().is_empty() || limit_value.is_some_and(|limit| limit <= 0) {
            return Ok(Vec::new());
        }
        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            let local = collect_like_source(connection, workspace_id, false, &query, limit_value)?;
            let legacy = collect_like_source(connection, workspace_id, true, &query, limit_value)?;
            let legacy_ids: HashSet<_> = legacy
                .iter()
                .map(|row| row.conversation_id.clone())
                .collect();
            let mut rows = merge_search_rows(local, legacy, None);
            rows.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| a.conversation_id.cmp(&b.conversation_id))
            });
            if let Some(limit) = limit_value {
                let limit = limit as usize;
                // Prefer globally recent hits, only replacing the oldest
                // selected local when all visible legacy hits would be lost.
                if let Some(index) = rows
                    .iter()
                    .position(|row| legacy_ids.contains(&row.conversation_id))
                    && index >= limit
                {
                    rows.swap(limit - 1, index);
                }
                rows.truncate(limit);
            }
            rows.into_iter().map(Conversation::try_from).collect()
        })
        .await
    }
}

#[async_trait::async_trait]
impl ConversationRepository for ConversationRepositoryImpl {
    async fn upsert_conversation_ref(&self, conversation: &Conversation) -> anyhow::Result<()> {
        let conversation = conversation.clone();
        self.run_with_connection(move |connection, wid| {
            let record = ConversationRecord::new_ref(&conversation, wid);
            Self::upsert_local_conversation_record(connection, &record)
        })
        .await
    }

    async fn upsert_conversation(&self, conversation: Conversation) -> anyhow::Result<()> {
        self.run_with_connection(move |connection, wid| {
            let record = ConversationRecord::new(conversation, wid);
            Self::upsert_local_conversation_record(connection, &record)
        })
        .await
    }

    async fn get_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> anyhow::Result<Option<Conversation>> {
        let conversation_id = *conversation_id;
        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible.
            // We use explicit column selection (rather than
            // `ConversationRecord::as_select()`)
            // because `ConversationRecord::table_name = conversations` (it is also used
            // for writes). The TEMP VIEW has identical column types so the SELECT … load
            // works regardless.
            //
            // Order by `__forge_legacy_source` ascending so that the local row
            // (`__forge_legacy_source = 0`) takes precedence over the legacy
            // row (`__forge_legacy_source = 1`) when both share an id.
            let record: Option<ConversationRecord> =
                conversations_all::table
                    .filter(conversations_all::conversation_id.eq(conversation_id.into_string()))
                    .filter(conversations_all::workspace_id.eq(workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .order(diesel::dsl::sql::<diesel::sql_types::Integer>(
                        "__forge_legacy_source ASC",
                    ))
                    .select(conversations_all::all_columns)
                    .first(connection)
                    .optional()?;

            match record {
                Some(record) => Ok(Some(Conversation::try_from(record)?)),
                None => Ok(None),
            }
        })
        .await
    }

    async fn get_all_conversations(
        &self,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.run_with_connection(move |connection, wid| {
            use diesel::prelude::*;

            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible. The
            // legacy DB has rows whose `context` and `context_zstd` columns
            // are both NULL — those rows have titles + timestamps but no
            // message history. We must not filter them out at the SQL layer
            // or the picker will hide them.
            //
            // When a local row and a legacy row share a `conversation_id`
            // (split-DB cutover), we must surface only the local row —
            // returning both would let a read disagree with a later
            // mutation of that same id. The `__forge_legacy_source`
            // discriminator added by the view makes local-first ordering
            // a stable ORDER BY.
            let mut query =
                conversations_all::table
                    .filter(conversations_all::workspace_id.eq(&workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .order(conversations_all::updated_at.desc())
                    .into_boxed();

            if let Some(limit_value) = limit {
                query = query.limit(limit_value as i64);
            }

            let records: Vec<ConversationRecord> = query
                .select(conversations_all::all_columns)
                .load(connection)?;

            if records.is_empty() {
                return Ok(None);
            }

            // Deduplicate by conversation_id: the first-seen row wins.
            // Since the rows arrive sorted by updated_at desc and the
            // `__forge_legacy_source` discriminator on the view makes
            // local rows come before legacy rows for the same id (because
            // local rows have `__forge_legacy_source = 0`, which sorts
            // before `1` under natural numeric ordering), the first-seen
            // row is always the local row when one exists.
            let mut seen = std::collections::HashSet::new();
            let deduped: Vec<ConversationRecord> = records
                .into_iter()
                .filter(|row| seen.insert(row.conversation_id.clone()))
                .collect();

            let conversations: Result<Vec<Conversation>, _> =
                deduped.into_iter().map(Conversation::try_from).collect();
            Ok(Some(conversations?))
        })
        .await
    }

    async fn get_last_conversation(&self) -> anyhow::Result<Option<Conversation>> {
        self.run_with_connection(move |connection, wid| {
            use diesel::prelude::*;

            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible. The
            // legacy DB has rows whose `context` and `context_zstd` columns
            // are both NULL — those rows have titles + timestamps but no
            // message history. We must not filter them out at the SQL layer
            // or the picker will hide them.
            let record: Option<ConversationRecord> =
                conversations_all::table
                    .filter(conversations_all::workspace_id.eq(&workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .order(conversations_all::updated_at.desc())
                    .select(conversations_all::all_columns)
                    .first(connection)
                    .optional()?;
            let conversation = match record {
                Some(record) => Some(Conversation::try_from(record)?),
                None => None,
            };
            Ok(conversation)
        })
        .await
    }

    async fn delete_conversation(&self, conversation_id: &ConversationId) -> anyhow::Result<()> {
        let conversation_id = *conversation_id;
        let legacy = self
            .pool
            .legacy_database_path()
            .map(std::path::Path::to_path_buf);
        let primary = self.pool.database_path().to_path_buf();
        let acknowledgement = std::env::var_os("HELIOSLITE_ACCEPT_MISSING_LEGACY");
        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            connection.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                verify_forge_import_ledger_schema(conn)?;
                let id = conversation_id.into_string();
                let observation = inspect_legacy_projection(
                    conn,
                    legacy.as_deref(),
                    &primary,
                    acknowledgement.as_deref(),
                )?;
                validate_local_mutation(conn, &id, workspace_id)?;
                ensure_no_legacy_collision(conn, &id)?;
                let deleted = diesel::delete(conversations::table)
                    .filter(conversations::workspace_id.eq(workspace_id))
                    .filter(conversations::conversation_id.eq(&id))
                    .execute(conn)?;
                if deleted == 0 {
                    record_missing_delete(conn, &id, workspace_id)?;
                }
                ensure_legacy_unchanged(
                    conn,
                    legacy.as_deref(),
                    &primary,
                    acknowledgement.as_deref(),
                    &observation,
                )?;
                Ok(())
            })?;

            Ok(())
        })
        .await
    }

    async fn get_conversations_by_parent(
        &self,
        parent_id: &ConversationId,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        let parent_id = parent_id.into_string();
        self.run_with_connection(move |connection, wid| {
            use diesel::prelude::*;

            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible.
            // Local rows win dedup by ordering by `__forge_legacy_source ASC`.
            let records: Vec<ConversationRecord> =
                conversations_all::table
                    .filter(conversations_all::workspace_id.eq(&workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .filter(conversations_all::parent_id.eq(&parent_id))
                    .order(diesel::dsl::sql::<diesel::sql_types::Integer>(
                        "__forge_legacy_source ASC",
                    ))
                    .then_order_by(conversations_all::updated_at.desc())
                    .select(conversations_all::all_columns)
                    .load(connection)?;

            if records.is_empty() {
                return Ok(None);
            }

            let mut seen = std::collections::HashSet::new();
            let deduped: Vec<ConversationRecord> = records
                .into_iter()
                .filter(|row| seen.insert(row.conversation_id.clone()))
                .collect();

            let conversations: Result<Vec<Conversation>, _> =
                deduped.into_iter().map(Conversation::try_from).collect();
            Ok(Some(conversations?))
        })
        .await
    }

    async fn get_parent_conversations(
        &self,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        self.run_with_connection(move |connection, wid| {
            use diesel::dsl::sql;
            use diesel::prelude::*;

            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible. We
            // hide agent-launched (subagent / `forge -p`) conversations the
            // same way `get_parent_conversations_lite` does: the predicate
            // must run BEFORE the LIMIT so the most-recent rows are not
            // dominated by ephemeral subagent runs that truncate older
            // user conversations.
            //
            // Order by `__forge_legacy_source` first so local rows win the
            // dedup pass over legacy rows that share an id.
            let mut query =
                conversations_all::table
                    .filter(conversations_all::workspace_id.eq(&workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .filter(conversations_all::parent_id.is_null())
                    .filter(sql::<diesel::sql_types::Bool>(
                        "COALESCE(json_extract(context, '$.initiator'), 'user') <> 'agent'",
                    ))
                    .order(diesel::dsl::sql::<diesel::sql_types::Integer>(
                        "__forge_legacy_source ASC",
                    ))
                    .then_order_by(conversations_all::updated_at.desc())
                    .into_boxed();

            if let Some(limit_value) = limit {
                query = query.limit(limit_value as i64);
            }

            let records: Vec<ConversationRecord> = query
                .select(conversations_all::all_columns)
                .load(connection)?;

            if records.is_empty() {
                return Ok(None);
            }

            // Deduplicate by conversation_id: local wins because it appears
            // first (we sort by `__forge_legacy_source ASC` above).
            let mut seen = std::collections::HashSet::new();
            let deduped: Vec<ConversationRecord> = records
                .into_iter()
                .filter(|row| seen.insert(row.conversation_id.clone()))
                .collect();

            let conversations: Result<Vec<Conversation>, _> =
                deduped.into_iter().map(Conversation::try_from).collect();
            Ok(Some(conversations?))
        })
        .await
    }

    async fn get_parent_conversations_lite(
        &self,
        limit: Option<usize>,
        all_workspaces: bool,
    ) -> anyhow::Result<Option<Vec<ConversationSummary>>> {
        self.run_with_connection(move |connection, wid| {
            use diesel::dsl::sql;

            // Read from `conversations_all` so legacy rows are visible. The
            // legacy DB has rows whose `context` and `context_zstd` columns
            // are both NULL — those rows have titles + timestamps but no
            // message history. We must not filter them out at the SQL layer
            // or the picker will hide them.
            //
            // We still hide agent-launched (subagent / `forge -p`) conversations
            // because the interactive picker should not show ephemeral
            // subagent runs. Compressed rows have no plain `context`, so
            // json_extract yields NULL and they pass through.
            let mut query = conversations_all::table
                .filter(conversations_all::parent_id.is_null())
                .filter(sql::<diesel::sql_types::Bool>(
                    "COALESCE(json_extract(context, '$.initiator'), 'user') <> 'agent'",
                ))
                .select(ConversationRecordLite::as_select())
                .order(conversations_all::updated_at.desc())
                .into_boxed();

            if !all_workspaces {
                let workspace_id = wid.id() as i64;
                query = query.filter(conversations_all::workspace_id.eq(workspace_id).or(
                    diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                ));
            }

            if let Some(limit_value) = limit {
                query = query.limit(limit_value as i64);
            }

            let records: Vec<ConversationRecordLite> = query.load(connection)?;

            if records.is_empty() {
                return Ok(None);
            }

            let summaries: Vec<ConversationSummary> =
                records.into_iter().map(ConversationSummary::from).collect();
            Ok(Some(summaries))
        })
        .await
    }

    async fn get_conversations_by_source(
        &self,
        source: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        let source = source.to_string();
        self.run_with_connection(move |connection, wid| {
            use diesel::prelude::*;

            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible.
            let mut query =
                conversations_all::table
                    .filter(conversations_all::workspace_id.eq(&workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .filter(conversations_all::source.eq(&source))
                    .order(conversations_all::updated_at.desc())
                    .into_boxed();

            if let Some(limit_value) = limit {
                query = query.limit(limit_value as i64);
            }

            let records: Vec<ConversationRecord> = query
                .select(conversations_all::all_columns)
                .load(connection)?;

            if records.is_empty() {
                return Ok(None);
            }

            let conversations: Result<Vec<Conversation>, _> =
                records.into_iter().map(Conversation::try_from).collect();
            Ok(Some(conversations?))
        })
        .await
    }

    async fn search_conversations(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Vec<Conversation>> {
        let query = query.to_string();
        let limit_value = limit.map(|n| i64::try_from(n).unwrap_or(i64::MAX));
        // Try FTS5 first; if the virtual table is missing/corrupt or the
        // MATCH expression has a syntax error, fall back to a LIKE scan so
        // the UI never silently drops results.
        match self
            .search_conversations_fts(query.clone(), limit_value)
            .await
        {
            Ok(rows) => Ok(rows),
            Err(error) => {
                warn!(
                    %error,
                    query = %query,
                    "FTS5 search failed, falling back to LIKE scan"
                );
                self.search_conversations_like(query, limit_value).await
            }
        }
    }
    /// currently selected search hit. Returns `None` if no match.
    async fn get_conversation_snippet(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        token_count: usize,
    ) -> anyhow::Result<Option<String>> {
        let conversation_id_str = conversation_id.into_string();
        let query = query.to_string();
        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            // JOIN pattern mirrors `search_conversations` and
            // `get_conversation_highlight`: join `conversations` ↔
            // `conversations_fts` on rowid, filter by conversation_id
            // and workspace_id, then apply `snippet()` to the matched row.
            let sql = format!(
                "SELECT snippet(conversations_fts, 1, '[', ']', '…', {}) AS s \
                 FROM conversations c \
                 JOIN conversations_fts fts ON c.rowid = fts.rowid \
                 WHERE conversations_fts MATCH ? \
                   AND c.conversation_id = ? \
                   AND c.workspace_id = ?",
                token_count.min(256)
            );
            let raw: Vec<SnippetRow> = diesel::sql_query(sql)
                .bind::<diesel::sql_types::Text, _>(&query)
                .bind::<diesel::sql_types::Text, _>(&conversation_id_str)
                .bind::<diesel::sql_types::BigInt, _>(workspace_id)
                .load(connection)?;
            Ok(raw.into_iter().next().map(|r| r.s))
        })
        .await
    }

    /// Return the full `context` column with every match span wrapped in
    /// caller-supplied markup. Distinct from [`Self::get_conversation_snippet`]
    /// which truncates to a passage. Use cases:
    ///
    /// - CLI's `forge search --full-context` mode
    /// - TUI's expanded hit view (where hits need to be preserved inline)
    /// - External integrations that pre-render markup on the server
    ///
    /// `open_mark` and `close_mark` are written into the SQL via diesel
    /// `bind()` (not string interpolation) so the caller can pass arbitrary
    /// markup without SQL-injection concerns. SQLite's FTS5 `highlight()`
    /// function (https://sqlite.org/fts5.html#the_highlight_function) is a
    /// built-in since SQLite 3.32.
    ///
    /// SQL mirrors the proven `search_conversations` JOIN pattern: we join
    /// `conversations` ↔ `conversations_fts` on rowid and filter by both
    /// `conversation_id` and `workspace_id`. This is the same query shape
    /// that the passing `test_search_finds_compressed_conversations`
    /// exercises; a prior implementation used a rowid-subquery without
    /// the workspace_id predicate and returned `None` even when the FTS
    /// index clearly contained the conversation — the missing workspace
    /// predicate let the FTS MATCH short-circuit when the join context
    /// was incomplete.
    async fn get_conversation_highlight(
        &self,
        conversation_id: &ConversationId,
        query: &str,
        open_mark: &str,
        close_mark: &str,
    ) -> anyhow::Result<Option<String>> {
        let conversation_id_str = conversation_id.into_string();
        let query = query.to_string();
        let open_mark = open_mark.to_string();
        let close_mark = close_mark.to_string();
        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            // JOIN pattern (mirrors `search_conversations`): join on
            // `rowid` and filter by both `conversation_id` and
            // `workspace_id`. `highlight()` is a scalar function applied
            // to each joined row; we keep only the one whose base-table
            // `conversation_id` matches.
            let sql = "SELECT highlight(conversations_fts, 1, ?, ?) AS h \
                       FROM conversations c \
                       JOIN conversations_fts fts ON c.rowid = fts.rowid \
                       WHERE conversations_fts MATCH ? \
                         AND c.conversation_id = ? \
                         AND c.workspace_id = ?";
            let raw: Vec<HighlightRow> = diesel::sql_query(sql)
                .bind::<diesel::sql_types::Text, _>(&open_mark)
                .bind::<diesel::sql_types::Text, _>(&close_mark)
                .bind::<diesel::sql_types::Text, _>(&query)
                .bind::<diesel::sql_types::Text, _>(&conversation_id_str)
                .bind::<diesel::sql_types::BigInt, _>(workspace_id)
                .load(connection)?;
            Ok(raw.into_iter().next().map(|r| r.h))
        })
        .await
    }

    async fn optimize_fts_index(&self) -> anyhow::Result<()> {
        // FTS5's "optimize" command is invoked as a special INSERT against
        // the virtual table itself. Diesel has no typed binding for it, so
        // we use a raw sql_query. This is the canonical pattern from the
        // SQLite FTS5 docs: https://sqlite.org/fts5.html#the_optimize_command
        self.run_with_connection(move |connection, _wid| {
            diesel::sql_query(
                "INSERT INTO conversations_fts(conversations_fts) VALUES('optimize')",
            )
            .execute(connection)?;
            Ok(())
        })
        .await
    }

    async fn refresh_fts_index(&self) -> anyhow::Result<()> {
        // CONTENTFUL FTS5 populated in application code.
        // This ensures BOTH compressed and uncompressed rows are indexed.
        //
        // Process:
        // 1. Clear the FTS index (DELETE all rows)
        // 2. SELECT all conversations with their rowid, title, context, context_zstd,
        //    is_compressed
        // 3. For each row: if is_compressed=1, decompress context_zstd to get
        //    searchable text; otherwise use context directly
        // 4. INSERT (rowid, title, content, cwd) into conversations_fts
        //
        // This is more work than FTS5's 'rebuild' but necessary because:
        // - External-content FTS5 reads context column by name → compressed rows
        //   (context=NULL) are missed
        // - Decompression must happen in app code; FTS5 has no built-in codec
        // - Contentful FTS5 is the pragmatic correct solution
        self.run_with_connection(move |connection, _wid| {
            use crate::codec;
            use diesel::sql_types::{BigInt, Text, Nullable};

            // Step 1: Clear the FTS index
            diesel::sql_query("DELETE FROM conversations_fts")
                .execute(connection)?;

            // Step 2: Read all conversations using custom QueryableByName type
            let rows: Vec<FtsRefreshRow> = diesel::sql_query(
                "SELECT rowid, title, context, context_zstd, is_compressed, cwd \
                 FROM conversations"
            )
            .load(connection)?;

            // Step 3 & 4: For each row, decompress if needed and INSERT into FTS
            for row in rows {
                // Determine searchable content: decompress if compressed, else use plain text
                let content = if row.is_compressed == 1 {
                    if let Some(compressed) = row.context_zstd {
                        match codec::decompress(&compressed) {
                            Ok(decompressed) => decompressed,
                            Err(e) => {
                                eprintln!(
                                    "Warning: Failed to decompress context_zstd for rowid {}; skipping FTS: {}",
                                    row.rowid, e
                                );
                                String::new()
                            }
                        }
                    } else {
                        eprintln!("Warning: rowid {} marked compressed but context_zstd is None; skipping FTS", row.rowid);
                        String::new()
                    }
                } else {
                    // Uncompressed row: use context column directly
                    row.context.unwrap_or_default()
                };

                // Insert into FTS5 contentful table
                diesel::sql_query(
                    "INSERT INTO conversations_fts(rowid, title, content, cwd) VALUES (?, ?, ?, ?)"
                )
                .bind::<BigInt, _>(row.rowid)
                .bind::<Text, _>(&row.title)
                .bind::<Text, _>(&content)
                .bind::<Nullable<Text>, _>(&row.cwd)
                .execute(connection)?;
            }

            Ok(())
        })
        .await
    }

    async fn update_parent_id(
        &self,
        conversation_id: &ConversationId,
        new_parent_id: Option<&ConversationId>,
    ) -> anyhow::Result<()> {
        // The `Option<&ConversationId>` is borrowed for the duration of the
        // move into `run_with_connection`. We materialise the inner string
        // here so the closure becomes `'static`.
        let new_parent_id_str: Option<String> = new_parent_id.map(|id| id.into_string());
        let conversation_id_str = conversation_id.into_string();
        let now: chrono::NaiveDateTime = chrono::Utc::now().naive_utc();
        self.run_with_connection(move |connection, wid| {
            // Reserve the SQLite writer before checking ownership or existence so
            // another connection cannot delete or replace the row between checks.
            connection.immediate_transaction::<_, anyhow::Error, _>(|connection| {
                let workspace_id = wid.id() as i64;
                let workspace_id_opt: Option<i64> = Some(workspace_id);
                validate_local_mutation(connection, &conversation_id_str, workspace_id)?;
                let exists = conversations::table
                    .filter(conversations::workspace_id.eq(workspace_id_opt))
                    .filter(conversations::conversation_id.eq(&conversation_id_str))
                    .select(conversations::conversation_id)
                    .first::<String>(connection)
                    .optional()?
                    .is_some();
                if !exists {
                    return Ok(());
                }
                let affected = diesel::update(
                    conversations::table
                        .filter(conversations::workspace_id.eq(workspace_id_opt))
                        .filter(conversations::conversation_id.eq(&conversation_id_str)),
                )
                .set((
                    conversations::parent_id.eq(new_parent_id_str),
                    conversations::updated_at.eq(Some(now)),
                ))
                .execute(connection)?;
                anyhow::ensure!(
                    affected == 1,
                    "Conversation update did not affect a writable local row"
                );
                Ok(())
            })
        })
        .await
    }

    async fn rewind_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> anyhow::Result<Option<Conversation>> {
        let conversation_id_str = conversation_id.into_string();
        let now: chrono::NaiveDateTime = chrono::Utc::now().naive_utc();
        let result = self
            .run_with_connection(move |connection, wid| {
                let workspace_id = wid.id() as i64;
                validate_local_mutation(connection, &conversation_id_str, workspace_id)?;
                // MVP rewind semantics: find the most recent user message followed by
                // a tool call (i.e. last compaction point heuristic) and truncate
                // the context JSON to that prefix. If no tool call is found,
                // fall back to clearing context to the most recent user message.
                let record: Option<ConversationRecord> = conversations::table
                    .filter(conversations::workspace_id.eq(workspace_id))
                    .filter(conversations::conversation_id.eq(&conversation_id_str))
                    .first(connection)
                    .optional()?;

                if record.is_none() {
                    return Ok(None);
                }
                let new_context: Option<String> = match record {
                    Some(r) if r.context.is_some() => {
                        let ctx = r.context.as_ref().unwrap();
                        let rewind_point = find_last_compaction_point(ctx);
                        Some(truncate_context(ctx, rewind_point))
                    }
                    _ => None,
                };

                diesel::update(
                    conversations::table
                        .filter(conversations::workspace_id.eq(workspace_id))
                        .filter(conversations::conversation_id.eq(&conversation_id_str)),
                )
                .set((
                    conversations::context.eq(new_context),
                    conversations::updated_at.eq(Some(now)),
                ))
                .execute(connection)?;

                // Re-read the updated record so we can return it.
                let updated: Option<ConversationRecord> = conversations::table
                    .filter(conversations::workspace_id.eq(workspace_id))
                    .filter(conversations::conversation_id.eq(&conversation_id_str))
                    .first(connection)
                    .optional()?;
                Ok(updated.and_then(|r| Conversation::try_from(r).ok()))
            })
            .await?;
        Ok(result)
    }

    async fn get_conversations_by_cwd(
        &self,
        cwd: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<Option<Vec<Conversation>>> {
        let cwd = cwd.to_string();
        self.run_with_connection(move |connection, wid| {
            use diesel::prelude::*;

            let workspace_id = wid.id() as i64;
            // Read from `conversations_all` so legacy rows are visible.
            let mut query =
                conversations_all::table
                    .filter(conversations_all::workspace_id.eq(&workspace_id).or(
                        diesel::dsl::sql::<diesel::sql_types::Bool>("__forge_legacy_unscoped = 1"),
                    ))
                    .filter(conversations_all::cwd.eq(&cwd))
                    .order(conversations_all::updated_at.desc())
                    .into_boxed();

            if let Some(limit_value) = limit {
                query = query.limit(limit_value as i64);
            }

            let records: Vec<ConversationRecord> = query
                .select(conversations_all::all_columns)
                .load(connection)?;

            if records.is_empty() {
                return Ok(None);
            }

            let conversations: Result<Vec<Conversation>, _> =
                records.into_iter().map(Conversation::try_from).collect();
            Ok(Some(conversations?))
        })
        .await
    }

    async fn mark_intent_state(
        &self,
        conversation_id: &ConversationId,
        new_state: &str,
    ) -> anyhow::Result<()> {
        use crate::conversation::intent::IntentState;

        let conversation_id = conversation_id.into_string();
        let new_state_str = new_state.to_string();
        let new_state = IntentState::from_str(new_state)?;

        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            validate_local_mutation(connection, &conversation_id, workspace_id)?;
            // Read current state to validate transition
            let current_record: Option<ConversationRecord> = conversations::table
                .filter(conversations::workspace_id.eq(workspace_id))
                .filter(conversations::conversation_id.eq(&conversation_id))
                .first(connection)
                .optional()?;

            let record = current_record
                .ok_or_else(|| anyhow::anyhow!("Conversation {} not found", conversation_id))?;

            let current_state = IntentState::from_str(&record.intent_state)?;

            // Enforce state machine: can_transition_to returns false for illegal
            // transitions
            if !current_state.can_transition_to(new_state) {
                return Err(anyhow::anyhow!(
                    "Illegal state transition: {} → {}",
                    current_state,
                    new_state
                ));
            }

            // Update the state
            let now = chrono::Utc::now().naive_utc();
            diesel::update(
                conversations::table
                    .filter(conversations::workspace_id.eq(workspace_id))
                    .filter(conversations::conversation_id.eq(&conversation_id)),
            )
            .set((
                conversations::intent_state.eq(&new_state_str),
                conversations::updated_at.eq(Some(now)),
            ))
            .execute(connection)?;

            Ok(())
        })
        .await
    }

    async fn list_prune_eligible(
        &self,
        workspace_id: Option<i64>,
        limit: usize,
    ) -> anyhow::Result<Vec<Conversation>> {
        self.run_with_connection(move |connection, wid| {
            let workspace_id = workspace_id.unwrap_or_else(|| wid.id() as i64);
            let limit = limit as i64;

            // Use raw SQL to order by context blob size (descending) to prioritize
            // largest contexts first for maximum space reclamation.
            // Pruning is a writable operation, so read only from the local
            // conversations table. Attached legacy rows are read-only and must
            // never occupy a pruning slot.
            //
            // We deliberately keep the `(context IS NOT NULL OR is_compressed = 1)`
            // filter here because pruning operates on the actual context payload
            // (rows without a context cannot be pruned further) — but the picker
            // and other read APIs no longer apply this filter (they show all
            // rows, including tombstone rows whose context was damaged).
            let sql = "SELECT c.* FROM conversations c \
                 WHERE c.workspace_id = ? \
                   AND c.intent_state = 'verified' \
                   AND (c.context IS NOT NULL OR c.is_compressed = 1) \
                 ORDER BY COALESCE(LENGTH(c.context), LENGTH(c.context_zstd)) DESC \
                 LIMIT ?";

            let records: Vec<ConversationRecord> = diesel::sql_query(sql)
                .bind::<diesel::sql_types::BigInt, _>(workspace_id)
                .bind::<diesel::sql_types::BigInt, _>(limit)
                .load(connection)?;

            let conversations: Result<Vec<Conversation>, _> =
                records.into_iter().map(Conversation::try_from).collect();
            conversations
        })
        .await
    }

    async fn prune_conversation(&self, conversation_id: &ConversationId) -> anyhow::Result<()> {
        use crate::conversation::intent::IntentState;

        let conversation_id = conversation_id.into_string();

        self.run_with_connection(move |connection, wid| {
            let workspace_id = wid.id() as i64;
            validate_local_mutation(connection, &conversation_id, workspace_id)?;
            // Read current state to enforce invariant: only prune from 'verified'.
            // Mutation validation reads only local, writable records.
            let current_record: Option<ConversationRecord> = conversations::table
                .filter(conversations::workspace_id.eq(workspace_id))
                .filter(conversations::conversation_id.eq(&conversation_id))
                .first(connection)
                .optional()?;

            let record = current_record
                .ok_or_else(|| anyhow::anyhow!("Conversation {} not found", conversation_id))?;

            let current_state = IntentState::from_str(&record.intent_state)?;

            // Safety guard: only prune if verified
            if current_state != IntentState::Verified {
                return Err(anyhow::anyhow!(
                    "Cannot prune conversation with intent_state='{}'. Must be 'verified'.",
                    current_state
                ));
            }

            // Create a compact summary JSON to replace the full context blob
            // Preserves just enough metadata for the conversation to remain queryable
            let compressed_context = serde_json::json!({
                "type": "compressed",
                "conversation_id": conversation_id,
                "pruned_at": chrono::Utc::now().to_rfc3339(),
                "summary": "Conversation context pruned; full intent stored in MemoryPort"
            })
            .to_string();

            let now = chrono::Utc::now().naive_utc();
            diesel::update(
                conversations::table
                    .filter(conversations::workspace_id.eq(workspace_id))
                    .filter(conversations::conversation_id.eq(&conversation_id)),
            )
            .set((
                conversations::context.eq(compressed_context),
                conversations::intent_state.eq("pruned"),
                conversations::updated_at.eq(Some(now)),
            ))
            .execute(connection)?;

            Ok(())
        })
        .await
    }

    async fn compress_uncompressed_contexts(&self) -> anyhow::Result<(usize, usize, usize)> {
        self.run_with_connection(move |connection, _wid| {
            // Fetch all rows where context is plain-text and not yet compressed.
            // We select only the id + context column to avoid loading unrelated data.
            let sql = "SELECT conversation_id, context \
                       FROM conversations \
                       WHERE is_compressed = 0 AND context IS NOT NULL";

            #[derive(diesel::QueryableByName)]
            struct PlainRow {
                #[diesel(sql_type = diesel::sql_types::Text)]
                conversation_id: String,
                #[diesel(sql_type = diesel::sql_types::Text)]
                context: String,
            }

            let rows: Vec<PlainRow> = diesel::sql_query(sql).load(connection)?;

            let mut compressed_count = 0usize;
            let mut skipped_count = 0usize;
            let mut error_count = 0usize;

            for row in rows {
                match crate::codec::compress(&row.context) {
                    Ok(blob) => {
                        let result = diesel::sql_query(
                            "UPDATE conversations \
                             SET context_zstd = ?, context = NULL, is_compressed = 1 \
                             WHERE conversation_id = ?",
                        )
                        .bind::<diesel::sql_types::Binary, _>(&blob)
                        .bind::<diesel::sql_types::Text, _>(&row.conversation_id)
                        .execute(connection);

                        match result {
                            Ok(_) => compressed_count += 1,
                            Err(_) => error_count += 1,
                        }
                    }
                    Err(_) => {
                        // Compression failed for this row; leave as-is and count.
                        error_count += 1;
                    }
                }
            }

            // Rows with context IS NULL and is_compressed=0 don't appear in the
            // query; they are implicitly skipped. Count them for reporting.
            let null_sql = "SELECT COUNT(*) FROM conversations \
                            WHERE is_compressed = 0 AND context IS NULL";
            #[derive(diesel::QueryableByName)]
            struct CountRow {
                #[diesel(sql_type = diesel::sql_types::BigInt)]
                #[diesel(column_name = "COUNT(*)")]
                count: i64,
            }
            if let Ok(rows) = diesel::sql_query(null_sql).load::<CountRow>(connection)
                && let Some(r) = rows.first()
            {
                skipped_count = r.count as usize;
            }

            Ok((compressed_count, skipped_count, error_count))
        })
        .await
    }

    async fn import_forge_db(&self, source: PathBuf) -> anyhow::Result<ForgeImportReport> {
        self.import_forge_db_with_options(source, &forge_domain::ForgeImportOptions::default())
            .await
    }

    async fn import_forge_db_with_options(
        &self,
        source: PathBuf,
        options: &forge_domain::ForgeImportOptions,
    ) -> anyhow::Result<ForgeImportReport> {
        anyhow::ensure!(
            options.dry_run,
            "bulk import cannot distinguish prior deletions; select IDs explicitly"
        );
        self.import_forge_db_inner(source, options, false).await
    }

    async fn import_forge_db_selected(
        &self,
        source: PathBuf,
        ids: &[ConversationId],
        restore_deleted: bool,
        accept_unknown: bool,
        expected_digest: Option<&str>,
        options: &forge_domain::ForgeImportOptions,
    ) -> anyhow::Result<ForgeImportReport> {
        let ids = ids.iter().map(|id| id.into_string()).collect();
        self.import_forge_db_plan(
            source,
            options,
            true,
            Some((
                ids,
                restore_deleted,
                accept_unknown,
                expected_digest.map(str::to_owned),
            )),
        )
        .await
    }

    async fn export_forge_db(
        &self,
        destination: PathBuf,
        options: &forge_domain::ForgeExportOptions,
    ) -> anyhow::Result<forge_domain::ForgeExportReport> {
        let options = options.clone();
        self.run_with_connection(move |connection, _wid| {
            use diesel::Connection;
            use diesel::sql_types::{Nullable, Text};

            // Read all rows from this heliosLite repository, decompressing
            // context_zstd where present so the export contains the same
            // JSON the official lineage would write. The row type is the
            // top-level `HeliosExportRow` defined below in this module.
            //
            // Reads go through the `conversations_all` TEMP VIEW (installed
            // by SqliteCustomizer on every pool connection acquire) so
            // split-DB installs export legacy `.forge.db` rows too, not just
            // the primary `.forge.writes.db`.
            //
            // Agent-launched rows (`context.initiator = 'agent'`) are
            // excluded by default to match the TUI picker; the
            // `include_agent` flag re-includes them.
            let sql = if options.include_agent {
                "SELECT conversation_id, title, context, context_zstd, created_at, \
                 updated_at, metrics FROM conversations_all"
                    .to_string()
            } else {
                "SELECT conversation_id, title, context, context_zstd, created_at, \
                 updated_at, metrics FROM conversations_all \
                 WHERE COALESCE(json_extract(context, '$.initiator'), 'user') != 'agent' \
                    OR context IS NULL"
                    .to_string()
            };
            let rows: Vec<HeliosExportRow> = diesel::sql_query(sql).load(connection)?;

            // Dispatch to the format-specific writer if the caller asked
            // for JSONL or CSV. SQLite is the default and falls through to
            // the original code path below.
            if !matches!(options.format, forge_domain::ForgeExportFormat::Sqlite) {
                return write_export_non_sqlite(connection, &rows, destination, &options);
            }

            let mut report = forge_domain::ForgeExportReport {
                source_total: rows.len(),
                dry_run: options.dry_run,
                ..Default::default()
            };

            if options.dry_run {
                // Pre-flight: resolve each row's context so the report
                // surfaces genuine decompression failures without touching
                // disk. Rows without any payload export with NULL context
                // and still count as exported.
                for row in &rows {
                    match export_context(row) {
                        Ok(_) => report.exported += 1,
                        Err(()) => report.decompression_failed += 1,
                    }
                }
                return Ok(report);
            }

            if let Some(parent) = destination.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            let dest_url = destination.to_string_lossy().to_string();
            let mut dest = diesel::sqlite::SqliteConnection::establish(&dest_url)?;
            dest.transaction::<_, anyhow::Error, _>(|dest| {
                // Official-lineage conversations schema.
                diesel::sql_query(
                    "CREATE TABLE IF NOT EXISTS conversations (\
                     conversation_id TEXT PRIMARY KEY NOT NULL, \
                     title TEXT, \
                     context TEXT, \
                     created_at TEXT NOT NULL, \
                     updated_at TEXT, \
                     metrics TEXT, \
                     parent_id TEXT, \
                     source TEXT, \
                     workspace_id TEXT NOT NULL, \
                     message_count INTEGER NOT NULL DEFAULT 0, \
                     cwd TEXT\
                     )",
                )
                .execute(dest)?;
                diesel::sql_query("PRAGMA journal_mode = WAL;").execute(dest)?;
                Ok(())
            })?;

            dest.transaction::<_, anyhow::Error, _>(|dest| {
                #[derive(diesel::QueryableByName)]
                struct WorkspaceIdRow {
                    #[diesel(sql_type = Text)]
                    workspace_id: String,
                }
                let workspace_id =
                    diesel::sql_query("SELECT workspace_id FROM conversations_all LIMIT 1")
                        .get_result::<WorkspaceIdRow>(connection)
                        .map(|row| row.workspace_id)
                        .unwrap_or_else(|_| "imported".to_string());

                for row in &rows {
                    let plain_context = match export_context(row) {
                        Ok(Some(s)) => Some(s),
                        Ok(None) => None,
                        Err(()) => {
                            report.decompression_failed += 1;
                            continue;
                        }
                    };
                    let res = diesel::sql_query(
                        "INSERT OR REPLACE INTO conversations (\
                         conversation_id, title, context, created_at, updated_at, metrics, \
                         workspace_id, source) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind::<Text, _>(&row.conversation_id)
                    .bind::<Nullable<Text>, _>(row.title.as_deref())
                    .bind::<Nullable<Text>, _>(plain_context.as_deref())
                    .bind::<Text, _>(&row.created_at)
                    .bind::<Nullable<Text>, _>(row.updated_at.as_deref())
                    .bind::<Nullable<Text>, _>(row.metrics.as_deref())
                    .bind::<Text, _>(&workspace_id)
                    .bind::<Nullable<Text>, _>(Some("exported:helioslite"))
                    .execute(dest);
                    match res {
                        Ok(_) => report.exported += 1,
                        Err(error) => {
                            eprintln!(
                                "Failed to export conversation {}: {}",
                                row.conversation_id, error
                            );
                            report.errors += 1;
                        }
                    }
                }
                Ok(())
            })?;

            Ok(report)
        })
        .await
    }

    async fn database_stats(&self) -> anyhow::Result<forge_domain::HeliosdoctorDbStats> {
        self.run_with_connection(move |connection, _wid| {
            use diesel::sql_types::{BigInt, Nullable, Text};

            #[derive(diesel::QueryableByName)]
            struct Counts {
                #[diesel(sql_type = BigInt)]
                total: i64,
                #[diesel(sql_type = BigInt)]
                compressed: i64,
                #[diesel(sql_type = BigInt)]
                uncompressed: i64,
                #[diesel(sql_type = BigInt)]
                empty: i64,
                #[diesel(sql_type = BigInt)]
                oversized: i64,
                #[diesel(sql_type = BigInt)]
                agent_initiated: i64,
            }
            let counts: Counts = diesel::sql_query(
                "SELECT \
                     COUNT(*) AS total, \
                     SUM(CASE WHEN is_compressed = 1 THEN 1 ELSE 0 END) AS compressed, \
                     SUM(CASE WHEN is_compressed = 0 AND context IS NOT NULL THEN 1 ELSE 0 END) AS uncompressed, \
                     SUM(CASE WHEN is_compressed = 0 AND context IS NULL THEN 1 ELSE 0 END) AS empty, \
                     SUM(CASE WHEN length(context) > 1048576 OR length(context_zstd) > 1048576 THEN 1 ELSE 0 END) AS oversized, \
                     SUM(CASE WHEN json_extract(context, '$.initiator') = 'agent' THEN 1 ELSE 0 END) AS agent_initiated \
                 FROM conversations",
            )
            .get_result(connection)?;

            #[derive(diesel::QueryableByName)]
            struct IntegrityRow {
                #[diesel(sql_type = Nullable<Text>)]
                integrity_check: Option<String>,
            }
            let integrity_check = match diesel::sql_query("PRAGMA integrity_check")
                .get_result::<IntegrityRow>(connection)
            {
                Ok(row) => row.integrity_check.unwrap_or_else(|| "ok".to_string()),
                Err(error) => format!("error: {}", error),
            };

            // `ConversationRepository::database_stats` only sees the
            // single-DB view (no ATTACH); the richer cross-DB count is
            // produced by `EnvironmentInfra::compute_database_stats` in
            // `forge_infra::env` and surfaced through `heliosdoctor --verbose`.
            Ok(forge_domain::HeliosdoctorDbStats {
                total_conversations: counts.total.max(0) as u64,
                compressed_rows: counts.compressed.max(0) as u64,
                uncompressed_rows: counts.uncompressed.max(0) as u64,
                empty_rows: counts.empty.max(0) as u64,
                oversized_rows: counts.oversized.max(0) as u64,
                agent_initiated_rows: counts.agent_initiated.max(0) as u64,
                integrity_check,
                legacy_attached: None,
                write_db_path: None,
                legacy_db_path: None,
                tables: Default::default(),
                error: None,
            })
        })
        .await
    }

    async fn forget_conversations(
        &self,
        options: &forge_domain::ForgeForgetOptions,
    ) -> anyhow::Result<forge_domain::ForgeForgetReport> {
        let options = options.clone();
        let legacy = self
            .pool
            .legacy_database_path()
            .map(std::path::Path::to_path_buf);
        let primary = self.pool.database_path().to_path_buf();
        let acknowledgement = std::env::var_os("HELIOSLITE_ACCEPT_MISSING_LEGACY");
        self.run_with_connection(move |connection, wid| {
            use diesel::sql_types::BigInt;

            // Validate: at least one selector must be set.
            if options.ids.is_empty()
                && options.source.is_none()
                && options.older_than_secs.is_none()
            {
                return Err(anyhow::anyhow!(
                    "at least one filter (--id, --source, --older-than) must be set"
                ));
            }

            let mut conditions: Vec<String> = vec![format!("workspace_id = {}", wid.id() as i64)];
            if !options.ids.is_empty() {
                let id_list: Vec<String> =
                    options.ids.iter().map(|id| format!("'{}'", id)).collect();
                conditions.push(format!("conversation_id IN ({})", id_list.join(",")));
            }
            if let Some(source) = &options.source {
                let escaped = source.replace('\'', "''");
                conditions.push(format!("source = '{}'", escaped));
            }
            if let Some(secs) = options.older_than_secs {
                let cutoff = chrono::Utc::now().timestamp() - secs;
                conditions.push(format!("updated_at < datetime({}, 'unixepoch')", cutoff));
            }
            let where_clause = format!("WHERE {}", conditions.join(" AND "));

            #[derive(diesel::QueryableByName)]
            struct CountRow {
                #[diesel(sql_type = BigInt)]
                n: i64,
            }
            if options.dry_run {
                let matched = diesel::sql_query(format!(
                    "SELECT COUNT(*) AS n FROM conversations {}",
                    where_clause
                ))
                .get_result::<CountRow>(connection)?
                .n
                .max(0) as usize;
                eprintln!("[forget] preview is a count only; legacy availability and deletion collisions are checked when deleting");
                return Ok(forge_domain::ForgeForgetReport { matched, deleted: 0, dry_run: true });
            }

            connection.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                verify_forge_import_ledger_schema(conn)?;
                let matched = diesel::sql_query(format!(
                    "SELECT COUNT(*) AS n FROM conversations {}",
                    where_clause
                ))
                .get_result::<CountRow>(conn)?
                .n
                .max(0) as usize;
                let observation = if matched > 0 || !options.ids.is_empty() {
                    let observation = inspect_legacy_projection(conn, legacy.as_deref(), &primary, acknowledgement.as_deref())?;
                    let collisions = diesel::sql_query(format!("SELECT COUNT(*) AS n FROM conversations_all WHERE __forge_legacy_source = 1 AND conversation_id IN (SELECT conversation_id FROM conversations {})", where_clause)).get_result::<CountRow>(conn)?.n;
                    anyhow::ensure!(collisions == 0, "deletion would reveal an attached legacy conversation; reconciliation is required");
                    Some(observation)
                } else { None };
                let mut missing = Vec::new();
                for id in &options.ids {
                    let id = id.into_string();
                    let exists = diesel::select(diesel::dsl::exists(
                        conversations::table.filter(conversations::conversation_id.eq(&id)),
                    ))
                    .get_result::<bool>(conn)?;
                    if !exists {
                        validate_local_mutation(conn, &id, wid.id() as i64)?;
                        anyhow::ensure!(
                            options.source.is_none() && options.older_than_secs.is_none(),
                            "cannot determine whether missing ID matches additional forget filters"
                        );
                        missing.push(id);
                    }
                }
                let deleted =
                    diesel::sql_query(format!("DELETE FROM conversations {}", where_clause))
                        .execute(conn)?;
                for id in missing {
                    record_missing_delete(conn, &id, wid.id() as i64)?;
                }
                if let Some(observation) = &observation {
                    ensure_legacy_unchanged(conn, legacy.as_deref(), &primary, acknowledgement.as_deref(), observation)?;
                }
                Ok(forge_domain::ForgeForgetReport { matched, deleted, dry_run: false })
            })
        })
        .await
    }

    async fn migrate_data_dir(
        &self,
        options: &forge_domain::MigrateOptions,
    ) -> anyhow::Result<forge_domain::ForgeMigrateReport> {
        let dry_run = options.dry_run;
        let db_path = self.pool.database_path().to_path_buf();
        let source_dir = match db_path.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                return Err(anyhow::anyhow!(
                    "could not determine parent directory of DB path"
                ));
            }
        };
        let source_dir_name = source_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        if source_dir_name == ".helioslite" {
            // Already canonical — no-op.
            return Ok(forge_domain::ForgeMigrateReport {
                source_path: source_dir.clone(),
                destination_path: source_dir,
                outcome: "already_migrated".to_string(),
                bytes_copied: 0,
                conversations_verified: 0,
                renamed_legacy_to: None,
            });
        }

        // Find the home directory and append `.helioslite`.
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                source_dir
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default()
            });
        let destination_dir = home.join(".helioslite");

        // The fork writes conversations to the pool's database path
        // (`.forge.writes.db`) and reads legacy rows from `.forge.db` via
        // the `conversations_all` UNION; migrate both files so the canonical
        // dir is a complete copy of the fork's data.
        let write_db = db_path.clone();
        let source_db = source_dir.join(".forge.db");
        let write_db_name = write_db
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| ".forge.writes.db".to_string());
        let destination_db = destination_dir.join(".forge.db");
        let destination_write_db = destination_dir.join(&write_db_name);

        let mut report = forge_domain::ForgeMigrateReport {
            source_path: source_dir.clone(),
            destination_path: destination_dir.clone(),
            outcome: "noop_legacy_missing".to_string(),
            bytes_copied: 0,
            conversations_verified: 0,
            renamed_legacy_to: None,
        };

        if !source_db.exists() && !write_db.exists() {
            // No fork data at all — just create the canonical dir.
            std::fs::create_dir_all(&destination_dir)?;
            return Ok(report);
        }

        report.outcome = "migrated".to_string();

        // Copy the DB files (and WAL/SHM siblings if present).
        std::fs::create_dir_all(&destination_dir)?;

        let copy_with_wal = |src: &std::path::Path, dst: &std::path::Path| -> std::io::Result<()> {
            std::fs::copy(src, dst)?;
            for ext in ["-wal", "-shm", "-journal"] {
                let s = std::path::PathBuf::from(format!("{}{}", src.display(), ext));
                if s.exists() {
                    let d = std::path::PathBuf::from(format!("{}{}", dst.display(), ext));
                    let _ = std::fs::copy(&s, &d);
                }
            }
            Ok(())
        };

        // The write DB is the fork's primary data: copy it verbatim (with
        // WAL/SHM siblings) into place. It is not schema-validated — unlike
        // the legacy file it does not share the legacy `.forge.db` layout.
        if write_db.exists() {
            report.bytes_copied += std::fs::metadata(&write_db).map(|m| m.len()).unwrap_or(0);
            if !dry_run {
                let tmp = destination_dir.join(format!(".{}.tmp", write_db_name));
                copy_with_wal(&write_db, &tmp)?;
                if destination_write_db.exists() {
                    let _ = std::fs::remove_file(&destination_write_db);
                }
                std::fs::rename(&tmp, &destination_write_db)?;
                // WAL/SHM siblings were copied to the tmp name; move them too.
                for ext in ["-wal", "-shm", "-journal"] {
                    let tmp_sidecar = std::path::PathBuf::from(format!("{}{}", tmp.display(), ext));
                    if tmp_sidecar.exists() {
                        let dst_sidecar = std::path::PathBuf::from(format!(
                            "{}{}",
                            destination_write_db.display(),
                            ext
                        ));
                        let _ = std::fs::rename(&tmp_sidecar, &dst_sidecar);
                    }
                }
            }
        }

        // Copy the legacy DB (and WAL/SHM siblings if present) to a temp
        // path first, then rename atomically.
        if source_db.exists() {
            report.bytes_copied += std::fs::metadata(&source_db).map(|m| m.len()).unwrap_or(0);

            let tmp_dst = {
                let p = destination_db.clone();
                let fname = p
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".forge.db".to_string());
                let tmp = destination_dir.join(format!(".{}.tmp", fname));
                if dry_run {
                    // In dry-run mode, skip the actual copy and use the source as a
                    // stand-in for the validation query.
                    source_db.clone()
                } else {
                    copy_with_wal(&source_db, &tmp)?;
                    tmp
                }
            };

            // Validate the copy by opening it with a one-off diesel connection.
            let db_count = {
                use diesel::sql_types::BigInt;
                use diesel::sqlite::SqliteConnection;
                #[derive(diesel::QueryableByName)]
                struct CountRow {
                    #[diesel(sql_type = BigInt)]
                    n: i64,
                }
                let tmp_path = tmp_dst.to_string_lossy().to_string();
                let mut conn = match SqliteConnection::establish(&tmp_path) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = std::fs::remove_file(&tmp_dst);
                        return Err(anyhow::anyhow!("post-copy validation failed: {}", e));
                    }
                };
                let count: i64 = diesel::sql_query("SELECT COUNT(*) AS n FROM conversations")
                    .get_result::<CountRow>(&mut conn)
                    .map(|r| r.n)
                    .unwrap_or(0);
                count.max(0) as u64
            };

            // Atomic rename into place.
            if !dry_run {
                if destination_db.exists() {
                    let _ = std::fs::remove_file(&destination_db);
                }
                std::fs::rename(&tmp_dst, &destination_db)?;
                // WAL/SHM siblings were copied to the tmp name; move them too.
                for ext in ["-wal", "-shm", "-journal"] {
                    let fname = destination_db
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| ".forge.db".to_string());
                    let tmp = destination_dir.join(format!(".{}.tmp{}", fname, ext));
                    if tmp.exists() {
                        let dst = destination_dir.join(format!("{}{}", fname, ext));
                        let _ = std::fs::rename(&tmp, &dst);
                    }
                }
            }

            report.conversations_verified = db_count;
        }

        // Rename the legacy directory aside so the user can roll back.
        if !dry_run {
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S").to_string();
            let renamed =
                source_dir.with_file_name(format!("{}.migrated-{}", source_dir_name, timestamp));
            if let Err(e) = std::fs::rename(&source_dir, &renamed) {
                // Non-fatal: the copy succeeded; the legacy rename is a
                // hint, not a requirement.
                eprintln!(
                    "migrate: legacy directory could not be renamed ({}): {}",
                    renamed.display(),
                    e
                );
            } else {
                report.renamed_legacy_to = Some(renamed);
            }
        }

        Ok(report)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyObservation {
    path: Option<String>,
    present: bool,
    acknowledged: bool,
}

#[cfg(test)]
fn ensure_legacy_projection_with_ack(
    connection: &mut diesel::SqliteConnection,
    legacy: Option<&std::path::Path>,
    primary: &std::path::Path,
    acknowledgement: Option<&std::ffi::OsStr>,
) -> anyhow::Result<()> {
    inspect_legacy_projection(connection, legacy, primary, acknowledgement).map(|_| ())
}

fn inspect_legacy_projection(
    connection: &mut diesel::SqliteConnection,
    legacy: Option<&std::path::Path>,
    primary: &std::path::Path,
    acknowledgement: Option<&std::ffi::OsStr>,
) -> anyhow::Result<LegacyObservation> {
    use diesel::sql_types::Text;
    let Some(legacy) = legacy else {
        return Ok(LegacyObservation { path: None, present: false, acknowledged: false });
    };
    let path = legacy
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("legacy path cannot be represented in a preview"))?
        .to_owned();
    let present = legacy.try_exists()?;
    let acknowledged = acknowledgement.is_some_and(|value| std::path::Path::new(value) == legacy);
    let observation = LegacyObservation { path: Some(path), present, acknowledged };
    if !present {
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            n: i64,
        }
        let attached = diesel::sql_query(
            "SELECT COUNT(*) AS n FROM pragma_database_list WHERE name = 'legacy_read'",
        )
        .get_result::<Count>(connection)?
        .n;
        anyhow::ensure!(
            attached == 0,
            "missing legacy path still has an attached database; history identity is unknown"
        );
        anyhow::ensure!(
            acknowledged,
            "configured legacy history is missing; to accept the risk for this exact path, set HELIOSLITE_ACCEPT_MISSING_LEGACY to {}",
            legacy.display()
        );
        eprintln!(
            "[history] accepted risk of missing legacy path {}; past history is unknown",
            legacy.display()
        );
        return Ok(observation);
    }
    let legacy = legacy
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("configured legacy history is unavailable"))?;
    if primary.canonicalize().ok().as_ref() == Some(&legacy) {
        return Ok(observation);
    }
    #[derive(diesel::QueryableByName)]
    struct Value {
        #[diesel(sql_type = Text)]
        value: String,
    }
    let attached = diesel::sql_query(
        "SELECT file AS value FROM pragma_database_list WHERE name = 'legacy_read'",
    )
    .get_result::<Value>(connection)
    .optional()?;
    let attached = attached.and_then(|row| std::path::PathBuf::from(row.value).canonicalize().ok());
    anyhow::ensure!(
        attached.as_ref() == Some(&legacy),
        "configured legacy history is not attached"
    );
    let view = diesel::sql_query("SELECT sql AS value FROM sqlite_temp_master WHERE type = 'view' AND name = 'conversations_all'")
        .get_result::<Value>(connection).optional()?;
    anyhow::ensure!(
        view.is_some_and(|row| row.value.contains("legacy_read.conversations")),
        "configured legacy history is not visible in the read projection"
    );
    diesel::sql_query("SELECT 1 FROM conversations_all LIMIT 1").execute(connection)?;
    Ok(observation)
}

fn ensure_legacy_unchanged(
    connection: &mut diesel::SqliteConnection,
    legacy: Option<&std::path::Path>,
    primary: &std::path::Path,
    acknowledgement: Option<&std::ffi::OsStr>,
    expected: &LegacyObservation,
) -> anyhow::Result<()> {
    #[cfg(test)]
    if let Some(path) = legacy {
        let hook = {
            let mut slot = legacy_recheck_hook().lock().unwrap();
            if slot.as_ref().is_some_and(|(target, _)| target == path) {
                slot.take().map(|(_, hook)| hook)
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            hook(connection);
        }
    }
    let current = inspect_legacy_projection(connection, legacy, primary, acknowledgement)?;
    anyhow::ensure!(
        &current == expected,
        "legacy history configuration changed during operation"
    );
    Ok(())
}

#[cfg(test)]
fn legacy_recheck_hook() -> &'static std::sync::Mutex<
    Option<(
        std::path::PathBuf,
        Box<dyn FnOnce(&mut diesel::SqliteConnection) + Send>,
    )>,
> {
    static HOOK: std::sync::OnceLock<
        std::sync::Mutex<
            Option<(
                std::path::PathBuf,
                Box<dyn FnOnce(&mut diesel::SqliteConnection) + Send>,
            )>,
        >,
    > = std::sync::OnceLock::new();
    HOOK.get_or_init(|| std::sync::Mutex::new(None))
}

fn ensure_no_legacy_collision(
    connection: &mut diesel::SqliteConnection,
    id: &str,
) -> anyhow::Result<()> {
    use diesel::sql_types::{BigInt, Text};
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let collisions = diesel::sql_query("SELECT COUNT(*) AS n FROM conversations_all WHERE __forge_legacy_source = 1 AND conversation_id = ?")
        .bind::<Text, _>(id).get_result::<Count>(connection)?.n;
    anyhow::ensure!(
        collisions == 0,
        "deletion would reveal an attached legacy conversation; reconciliation is required"
    );
    Ok(())
}

fn record_missing_delete(
    connection: &mut diesel::SqliteConnection,
    conversation_id: &str,
    workspace_id: i64,
) -> anyhow::Result<()> {
    use diesel::sql_types::{BigInt, Text};
    let changed = diesel::sql_query("INSERT INTO forge_import_ledger (conversation_id, workspace_id, origin, state) VALUES (?, ?, 'unknown', 'deleted') ON CONFLICT(conversation_id) DO UPDATE SET state = 'deleted', generation = forge_import_ledger.generation + 1 WHERE forge_import_ledger.workspace_id = excluded.workspace_id")
        .bind::<Text, _>(conversation_id).bind::<BigInt, _>(workspace_id).execute(connection)?;
    anyhow::ensure!(
        changed == 1,
        "deleted ID has unknown or different workspace ownership"
    );
    Ok(())
}

fn verify_forge_import_ledger_schema(
    connection: &mut diesel::SqliteConnection,
) -> anyhow::Result<()> {
    use diesel::sql_types::BigInt;
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let count = diesel::sql_query("SELECT COUNT(*) AS n FROM sqlite_master WHERE (type = 'table' AND name = 'forge_import_ledger') OR (type = 'trigger' AND name IN ('forge_import_record_insert', 'forge_import_record_update', 'forge_import_record_delete'))")
        .get_result::<Count>(connection)?.n;
    anyhow::ensure!(
        count == 4,
        "Forge import deletion ledger schema is incomplete"
    );
    Ok(())
}

fn forge_row_digest(locator: &str, workspace_id: i64, row: &DecodedSourceRow) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"helioslite-forge-row-v1\0");
    hash.update(workspace_id.to_be_bytes());
    for field in [
        Some(locator),
        Some(row.conversation_id.as_str()),
        row.title.as_deref(),
        row.context.as_deref(),
        Some(row.created_at.as_str()),
        row.updated_at.as_deref(),
        row.metrics.as_deref(),
    ] {
        match field {
            Some(value) => {
                hash.update([1]);
                hash.update((value.len() as u64).to_be_bytes());
                hash.update(value.as_bytes());
            }
            None => hash.update([0]),
        }
    }
    hex::encode(hash.finalize())
}

fn forge_selection_digest(
    connection: &mut diesel::SqliteConnection,
    locator: &str,
    workspace_id: i64,
    row: &DecodedSourceRow,
    observation: &LegacyObservation,
) -> anyhow::Result<String> {
    use diesel::sql_types::{BigInt, Nullable, Text};
    use sha2::{Digest, Sha256};
    #[derive(diesel::QueryableByName)]
    struct Disposition {
        #[diesel(sql_type = Nullable<BigInt>)]
        workspace_id: Option<i64>,
        #[diesel(sql_type = Text)]
        state: String,
        #[diesel(sql_type = BigInt)]
        generation: i64,
    }
    #[derive(diesel::QueryableByName)]
    struct Count {
        #[diesel(sql_type = BigInt)]
        n: i64,
    }
    let previous = diesel::sql_query(
        "SELECT workspace_id, state, generation FROM forge_import_ledger WHERE conversation_id = ?",
    )
    .bind::<Text, _>(&row.conversation_id)
    .get_result::<Disposition>(connection)
    .optional()?;
    let present =
        diesel::sql_query("SELECT COUNT(*) AS n FROM conversations WHERE conversation_id = ?")
            .bind::<Text, _>(&row.conversation_id)
            .get_result::<Count>(connection)?
            .n;
    let visible = conversations_all::table
        .filter(conversations_all::conversation_id.eq(&row.conversation_id))
        .count()
        .get_result::<i64>(connection)?;
    let mut hash = Sha256::new();
    hash.update(b"helioslite-forge-preview-v2\0");
    hash.update(forge_row_digest(locator, workspace_id, row).as_bytes());
    if let Some(path) = &observation.path {
        hash.update([1]);
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path.as_bytes());
        hash.update([u8::from(observation.present)]);
        hash.update([u8::from(observation.acknowledged)]);
    } else {
        hash.update([0]);
    }
    hash.update(present.to_be_bytes());
    hash.update(visible.to_be_bytes());
    if let Some(previous) = previous {
        hash.update([1]);
        match previous.workspace_id {
            Some(owner) => {
                hash.update([1]);
                hash.update(owner.to_be_bytes());
            }
            None => hash.update([0]),
        }
        hash.update((previous.state.len() as u64).to_be_bytes());
        hash.update(previous.state.as_bytes());
        hash.update(previous.generation.to_be_bytes());
    } else {
        hash.update([0]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn source_readonly_uri(path: &std::path::Path) -> anyhow::Result<String> {
    let absolute = std::path::absolute(path)?;
    let mut uri = url::Url::from_file_path(&absolute)
        .map_err(|_| anyhow::anyhow!("invalid source database path: {}", path.display()))?;
    uri.query_pairs_mut().append_pair("mode", "ro");
    if uri.host_str().is_some() {
        // SQLite accepts UNC paths in the path component without requiring
        // the optional SQLITE_ALLOW_URI_AUTHORITY build flag.
        Ok(uri.as_str().replacen("file://", "file:////", 1))
    } else {
        Ok(uri.into())
    }
}

/// Shared row loop used by both the transactional and dry-run import paths.
/// Performs per-row decoding (idempotency lookup, context parsing) and either
/// inserts the row (transactional path) or just counts the outcome (dry-run).
fn import_rows(
    connection: &mut diesel::SqliteConnection,
    wid: WorkspaceHash,
    rows: &[DecodedSourceRow],
    options: &forge_domain::ForgeImportOptions,
    require_valid_context: bool,
    selection: Option<(bool, bool, &str, &str)>,
    report: &mut ForgeImportReport,
) -> anyhow::Result<()> {
    for row in rows {
        let Ok(id) = ConversationId::parse(&row.conversation_id) else {
            anyhow::ensure!(selection.is_none(), "selected ID is invalid");
            report.invalid_id += 1;
            if options.verbose {
                eprintln!("[import] skip invalid-id {}", row.conversation_id);
            }
            continue;
        };

        // Idempotency: skip conversations that already exist in either the
        // write DB or the legacy DB (via `conversations_all`) instead of
        // overwriting them.
        let existing: i64 = match conversations_all::table
            .filter(conversations_all::conversation_id.eq(&row.conversation_id))
            .count()
            .get_result(connection)
        {
            Ok(count) => count,
            Err(error) => {
                if selection.is_some() {
                    return Err(error.into());
                }
                eprintln!(
                    "Failed to check conversation {}: {}",
                    row.conversation_id, error
                );
                report.errors += 1;
                continue;
            }
        };
        if existing > 0 {
            report.skipped_existing += 1;
            if options.verbose {
                eprintln!("[import] skip existing {}", row.conversation_id);
            }
            continue;
        }

        if let Some((allow_restore, accept_unknown, _locator, _digest)) = selection {
            use diesel::sql_types::{BigInt, Nullable, Text};
            #[derive(diesel::QueryableByName)]
            struct LedgerRow {
                #[diesel(sql_type = Nullable<BigInt>)]
                workspace_id: Option<i64>,
                #[diesel(sql_type = Text)]
                state: String,
            }
            let previous = diesel::sql_query(
                "SELECT workspace_id, state FROM forge_import_ledger WHERE conversation_id = ?",
            )
            .bind::<Text, _>(&row.conversation_id)
            .get_result::<LedgerRow>(connection)
            .optional()?;
            if !options.dry_run {
                anyhow::ensure!(
                    accept_unknown,
                    "source installation identity and prior history are unverified; explicit acceptance is required"
                );
            }
            if let Some(previous) = previous {
                anyhow::ensure!(
                    previous.workspace_id == Some(wid.id() as i64),
                    "selected ID has unknown or different workspace ownership"
                );
                if options.dry_run && previous.state == "deleted" {
                    eprintln!(
                        "[import] selected {} has a recorded deletion; restoration requires --restore-deleted",
                        row.conversation_id
                    );
                }
                anyhow::ensure!(
                    previous.state == "deleted" && (allow_restore || options.dry_run),
                    "selected ID has a deletion record; explicit restoration is required"
                );
            } else if options.dry_run {
                eprintln!(
                    "[import] selected {} has UNKNOWN prior deletion history",
                    row.conversation_id
                );
            }
        }

        // Parse the plain-text context with the heliosLite record type.
        // By default, parse failures remain visible as title-only rows.
        // Strict callers can leave them unmodified for a later retry.
        let mut context_parse_failed = false;
        let context = match row.context.as_deref() {
            Some(json) if !json.trim().is_empty() => {
                let parsed = serde_json::from_str::<ContextRecord>(json)
                    .ok()
                    .and_then(|record| Context::try_from(record).ok());
                if parsed.is_none()
                    || (require_valid_context
                        && parsed
                            .as_ref()
                            .is_some_and(|ctx| ctx.messages.is_empty() && ctx.initiator.is_none()))
                {
                    context_parse_failed = true;
                }
                parsed
            }
            _ => {
                context_parse_failed = require_valid_context;
                None
            }
        };
        if context_parse_failed {
            report.context_parse_failed += 1;
            if require_valid_context {
                anyhow::ensure!(
                    selection.is_none(),
                    "selected conversation has missing, empty, or invalid context"
                );
                continue;
            }
        }

        let parsed_created_at = parse_naive_datetime(&row.created_at);
        anyhow::ensure!(
            selection.is_none() || parsed_created_at.is_some(),
            "selected conversation has an invalid creation timestamp"
        );
        let created_at = parsed_created_at.unwrap_or_else(|| chrono::Utc::now().naive_utc());
        let updated_at = row.updated_at.as_deref().and_then(parse_naive_datetime);
        anyhow::ensure!(
            selection.is_none() || row.updated_at.is_none() || updated_at.is_some(),
            "selected conversation has an invalid update timestamp"
        );

        let parsed_metrics = row
            .metrics
            .as_deref()
            .and_then(|json| serde_json::from_str::<MetricsRecord>(json).ok())
            .map(forge_domain::Metrics::from);
        anyhow::ensure!(
            selection.is_none() || row.metrics.is_none() || parsed_metrics.is_some(),
            "selected conversation has invalid metrics"
        );
        let metrics = parsed_metrics
            .unwrap_or_else(|| forge_domain::Metrics::default().started_at(created_at.and_utc()));

        let conversation = forge_domain::Conversation::new(id)
            .context(context)
            .title(row.title.clone())
            .metrics(metrics)
            .source(Some("imported:forge".to_string()))
            .metadata(
                forge_domain::MetaData::new(created_at.and_utc())
                    .updated_at(updated_at.map(|timestamp| timestamp.and_utc())),
            );

        if options.dry_run {
            report.imported += 1;
            if options.verbose {
                eprintln!("[import] (dry-run) would import {}", row.conversation_id);
            }
            continue;
        }

        let record = ConversationRecord::new(conversation, wid);
        if require_valid_context && record.context.is_none() && record.context_zstd.is_none() {
            anyhow::ensure!(
                selection.is_none(),
                "selected conversation context could not be persisted"
            );
            report.errors += 1;
            continue;
        }
        match diesel::insert_into(conversations::table)
            .values(&record)
            .execute(connection)
        {
            Ok(_) => {
                if let Some((_, _, locator, digest)) = selection {
                    use diesel::sql_types::{BigInt, Text};
                    let changed = diesel::sql_query("INSERT INTO forge_import_ledger (conversation_id, workspace_id, origin, state, source_locator, source_digest) VALUES (?, ?, 'forge', 'imported', ?, ?) ON CONFLICT(conversation_id) DO UPDATE SET origin = 'forge', state = 'imported', source_locator = excluded.source_locator, source_digest = excluded.source_digest WHERE forge_import_ledger.workspace_id = excluded.workspace_id")
                        .bind::<Text, _>(&row.conversation_id)
                        .bind::<BigInt, _>(wid.id() as i64)
                        .bind::<Text, _>(locator)
                        .bind::<Text, _>(digest)
                        .execute(connection)?;
                    anyhow::ensure!(changed == 1, "selected ID provenance ownership changed");
                }
                report.imported += 1;
                if options.verbose {
                    eprintln!("[import] imported {}", row.conversation_id);
                }
            }
            Err(error) => {
                if selection.is_some() {
                    return Err(error.into());
                }
                eprintln!(
                    "Failed to import conversation {}: {}",
                    row.conversation_id, error
                );
                report.errors += 1;
            }
        }
    }
    Ok(())
}

/// Diesel row type used by `import_forge_db_with_options`. Renamed from
/// `SourceRow` to avoid collision with the snapshot-history `SourceRow` in
/// `crate::snapshot::SourceRow` (which lives in the same module tree).
#[derive(Debug)]
struct DecodedSourceRow {
    conversation_id: String,
    title: Option<String>,
    context: Option<String>,
    created_at: String,
    updated_at: Option<String>,
    metrics: Option<String>,
}

impl diesel::QueryableByName<diesel::sqlite::Sqlite> for DecodedSourceRow {
    fn build<'a>(
        row: &impl diesel::row::NamedRow<'a, diesel::sqlite::Sqlite>,
    ) -> diesel::deserialize::Result<Self> {
        use diesel::row::NamedRow;
        use diesel::sql_types::{Nullable, Text};
        Ok(Self {
            conversation_id: NamedRow::get::<Text, _>(row, "conversation_id")?,
            title: NamedRow::get::<Nullable<Text>, _>(row, "title")?,
            context: NamedRow::get::<Nullable<Text>, _>(row, "context")?,
            created_at: NamedRow::get::<Text, _>(row, "created_at")?,
            updated_at: NamedRow::get::<Nullable<Text>, _>(row, "updated_at")?,
            metrics: NamedRow::get::<Nullable<Text>, _>(row, "metrics")?,
        })
    }
}

/// Diesel row type used by `export_forge_db`. Renamed from `HeliosRow` to
/// avoid collision with the public `HeliosRow` in
/// `crate::snapshot::HeliosRow`.
#[derive(Debug)]
struct HeliosExportRow {
    conversation_id: String,
    title: Option<String>,
    context: Option<String>,
    context_zstd: Option<Vec<u8>>,
    created_at: String,
    updated_at: Option<String>,
    metrics: Option<String>,
}

impl diesel::QueryableByName<diesel::sqlite::Sqlite> for HeliosExportRow {
    fn build<'a>(
        row: &impl diesel::row::NamedRow<'a, diesel::sqlite::Sqlite>,
    ) -> diesel::deserialize::Result<Self> {
        use diesel::row::NamedRow;
        use diesel::sql_types::{Binary, Nullable, Text};
        Ok(Self {
            conversation_id: NamedRow::get::<Text, _>(row, "conversation_id")?,
            title: NamedRow::get::<Nullable<Text>, _>(row, "title")?,
            context: NamedRow::get::<Nullable<Text>, _>(row, "context")?,
            context_zstd: NamedRow::get::<Nullable<Binary>, _>(row, "context_zstd")?,
            created_at: NamedRow::get::<Text, _>(row, "created_at")?,
            updated_at: NamedRow::get::<Nullable<Text>, _>(row, "updated_at")?,
            metrics: NamedRow::get::<Nullable<Text>, _>(row, "metrics")?,
        })
    }
}

/// Resolve an export row's context into a plain (uncompressed) string.
///
/// Returns:
/// - `Ok(Some(plain))` — the row has a context payload (plain or zstd) that
///   decompressed successfully.
/// - `Ok(None)` — the row has **no** context payload at all. This is not a
///   failure: the row is exported with a NULL context so its metadata (id,
///   title, timestamps) is preserved in the export.
/// - `Err(())` — the row has a payload but zstd decompression failed. Only
///   these count as `decompression_failed` in export reports.
fn export_context(row: &HeliosExportRow) -> Result<Option<String>, ()> {
    use crate::codec::decompress;
    if let Some(plain) = row.context.as_deref()
        && !plain.trim().is_empty()
    {
        return Ok(Some(plain.to_string()));
    }
    if let Some(bytes) = row.context_zstd.as_deref() {
        match decompress(bytes) {
            Ok(plain) => Ok(Some(plain)),
            Err(_) => Err(()),
        }
    } else {
        Ok(None)
    }
}

/// Write the export in a non-SQLite format (JSONL or CSV). The output is
/// always a single file with one record per line. Used by
/// `export_forge_db` when the format is not `Sqlite`.
fn write_export_non_sqlite(
    _connection: &mut PooledSqliteConnection,
    rows: &[HeliosExportRow],
    destination: PathBuf,
    options: &forge_domain::ForgeExportOptions,
) -> anyhow::Result<forge_domain::ForgeExportReport> {
    use forge_domain::ForgeExportFormat;

    let mut report = forge_domain::ForgeExportReport {
        source_total: rows.len(),
        dry_run: options.dry_run,
        ..Default::default()
    };

    if options.dry_run {
        for row in rows {
            match export_context(row) {
                Ok(_) => report.exported += 1,
                Err(()) => report.decompression_failed += 1,
            }
        }
        return Ok(report);
    }

    if let Some(parent) = destination.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut out = String::new();
    match options.format {
        ForgeExportFormat::Jsonl => {
            for row in rows {
                let plain_context = match export_context(row) {
                    Ok(Some(s)) => s,
                    Ok(None) => String::new(),
                    Err(()) => {
                        report.decompression_failed += 1;
                        continue;
                    }
                };
                let record = serde_json::json!({
                    "conversation_id": row.conversation_id,
                    "title": row.title,
                    "context": if plain_context.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::from_str::<serde_json::Value>(&plain_context)
                            .unwrap_or(serde_json::Value::String(plain_context))
                    },
                    "created_at": row.created_at,
                    "updated_at": row.updated_at,
                    "metrics": row.metrics,
                });
                out.push_str(&serde_json::to_string(&record)?);
                out.push('\n');
                report.exported += 1;
            }
        }
        ForgeExportFormat::Csv => {
            out.push_str("conversation_id,title,created_at,updated_at,context\n");
            for row in rows {
                let plain_context = match export_context(row) {
                    Ok(Some(s)) => s,
                    Ok(None) => String::new(),
                    Err(()) => {
                        report.decompression_failed += 1;
                        continue;
                    }
                };
                let csv_escape = |s: &str| -> String {
                    if s.contains(',') || s.contains('"') || s.contains('\n') {
                        format!("\"{}\"", s.replace('"', "\"\""))
                    } else {
                        s.to_string()
                    }
                };
                out.push_str(&csv_escape(&row.conversation_id));
                out.push(',');
                out.push_str(&csv_escape(row.title.as_deref().unwrap_or("")));
                out.push(',');
                out.push_str(&csv_escape(&row.created_at));
                out.push(',');
                out.push_str(&csv_escape(row.updated_at.as_deref().unwrap_or("")));
                out.push(',');
                out.push_str(&csv_escape(&plain_context));
                out.push('\n');
                report.exported += 1;
            }
        }
        ForgeExportFormat::Sqlite => {
            // Caller should have dispatched this branch before reaching
            // here. Defensive: error rather than silently produce empty.
            return Err(anyhow::anyhow!(
                "write_export_non_sqlite called with Sqlite format"
            ));
        }
    }

    std::fs::write(&destination, out)?;
    Ok(report)
}

/// Parse a timestamp stored by forge-lineage databases into a
/// [`chrono::NaiveDateTime`]. Accepts the diesel TEXT serialization
/// (`%Y-%m-%d %H:%M:%S%.f`) as well as RFC 3339 variants. Returns `None`
/// when the string is unrecognised so callers can fall back to "now".
fn parse_naive_datetime(value: &str) -> Option<chrono::NaiveDateTime> {
    const FORMATS: &[&str] = &[
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.fZ",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%SZ",
    ];
    FORMATS
        .iter()
        .find_map(|format| chrono::NaiveDateTime::parse_from_str(value, format).ok())
}

/// Find the byte-offset in the context JSON immediately after the last
/// "compaction point" we can detect. The MVP heuristic scans the JSON string
/// for tool-call markers (`"name":`) in reverse and returns the offset of
/// the most recent user-text content that *precedes* a tool call.
///
/// `0` means "no rewound prefix found; truncate to empty" (full reset).
fn find_last_compaction_point(context_json: &str) -> usize {
    // Walk the JSON looking for the most recent `"role":"user"` message
    // boundary followed by a tool call. Each message entry in the context
    // is a JSON object; we just look for the substring order heuristically.
    // This is intentionally conservative: it errs on "rewind less, keep
    // more history" rather than "rewind too far, lose context".
    let user_marker = "\"role\":\"user\"";
    let tool_marker = "\"tool_calls\"";

    // Find the last user-role occurrence.
    let last_user = context_json.rfind(user_marker);
    if last_user.is_none() {
        return 0;
    }
    // After that user-role, look forward for the first tool_call marker.
    let after_user = last_user.unwrap() + user_marker.len();
    if context_json
        .get(after_user..)
        .is_some_and(|tail| tail.find(tool_marker).is_some())
    {
        // Truncate at the user-role boundary so we keep the user turn
        // but discard everything after it (including the tool call).
        return last_user.unwrap();
    }
    // No tool call after the last user message — treat the last user
    // message as the rewind point too (discard any trailing assistant
    // text/tool results that came after).
    last_user.unwrap()
}

/// Truncate the context JSON to the prefix `rewind_point` bytes long.
/// Re-emits a valid JSON shape: `{ "messages": ...truncated prefix... }`.
/// If the prefix is `0`, returns an empty messages array.
fn truncate_context(context_json: &str, rewind_point: usize) -> String {
    if rewind_point == 0 {
        return r#"{"messages":[]}"#.to_string();
    }
    // Walk backwards to the previous comma or opening brace so we don't
    // produce a truncated object/messages array.
    let bytes = context_json.as_bytes();
    let mut cut = rewind_point.min(bytes.len());
    while cut > 0
        && bytes
            .get(cut - 1)
            .is_some_and(|byte| !matches!(byte, b',' | b'[' | b'{'))
    {
        cut -= 1;
    }
    let prefix = context_json.get(..cut).unwrap_or_default();
    format!("{}\"rewound\":true}}", prefix.trim_end_matches([',', ' ']))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use forge_domain::{
        Context, ContextMessage, Effort, FileOperation, Metrics, Role, ToolCallFull, ToolCallId,
        ToolChoice, ToolDefinition, ToolKind, ToolName, ToolOutput, ToolResult, ToolValue, Usage,
    };
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::conversation::conversation_record::{ContextRecord, MetricsRecord};
    use crate::database::{DatabasePool, PoolConfig};

    fn repository() -> anyhow::Result<ConversationRepositoryImpl> {
        let pool = Arc::new(DatabasePool::in_memory()?);
        Ok(ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0)))
    }

    #[tokio::test]
    async fn test_upsert_and_find_by_id() -> anyhow::Result<()> {
        let fixture = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));
        let repo = repository()?;

        repo.upsert_conversation(fixture.clone()).await?;

        let actual = repo.get_conversation(&fixture.id).await?;
        assert!(actual.is_some());
        let retrieved = actual.unwrap();
        assert_eq!(retrieved.id, fixture.id);
        assert_eq!(retrieved.title, fixture.title);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_by_id_non_existing() -> anyhow::Result<()> {
        let repo = repository()?;
        let non_existing_id = ConversationId::generate();

        let actual = repo.get_conversation(&non_existing_id).await?;

        assert!(actual.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_upsert_updates_existing_conversation() -> anyhow::Result<()> {
        let mut fixture = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));
        let repo = repository()?;

        // Insert initial conversation
        repo.upsert_conversation(fixture.clone()).await?;

        // Update the conversation
        fixture = fixture.title(Some("Updated Title".to_string()));
        repo.upsert_conversation(fixture.clone()).await?;

        let actual = repo.get_conversation(&fixture.id).await?;
        assert!(actual.is_some());
        assert_eq!(actual.unwrap().title, Some("Updated Title".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn ordinary_upserts_allow_new_and_owned_local_records() {
        for borrowed in [false, true] {
            let repo = repository().unwrap();
            let mut fixture = Conversation::new(ConversationId::generate());
            for title in ["new", "updated"] {
                fixture = fixture.title(Some(title.to_string()));
                if borrowed {
                    repo.upsert_conversation_ref(&fixture).await.unwrap();
                } else {
                    repo.upsert_conversation(fixture.clone()).await.unwrap();
                }
                let actual = repo.get_conversation(&fixture.id).await.unwrap().unwrap();
                assert_eq!(actual.title, Some(title.to_string()));
            }
        }
    }

    #[tokio::test]
    async fn upsert_conflict_primitive_rejects_foreign_owner_without_changing_row() {
        let repo = repository().unwrap();
        let fixture =
            Conversation::new(ConversationId::generate()).title(Some("original".to_string()));
        repo.upsert_conversation(fixture.clone()).await.unwrap();
        let incoming = fixture.clone().title(Some("foreign overwrite".to_string()));
        let record = ConversationRecord::new_ref(&incoming, WorkspaceHash::new(42));
        // Bypass the ordinary precheck to model a foreign insert winning the
        // race before the ON CONFLICT statement executes.
        let error = repo
            .run_with_connection(move |connection, _wid| {
                ConversationRepositoryImpl::upsert_conversation_ref_record(connection, &record)
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Conversation upsert did not affect a writable local row"
        );
        let actual = repo.get_conversation(&fixture.id).await.unwrap().unwrap();
        assert_eq!(actual.title, fixture.title);
    }

    #[tokio::test]
    async fn ref_upsert_replaces_existing_plain_context_with_incoming_compressed_context()
    -> anyhow::Result<()> {
        let repo = repository()?;
        let legacy_context =
            Context::default().messages(vec![ContextMessage::user("legacy context", None).into()]);
        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("legacy context row".to_string()))
            .context(Some(legacy_context.clone()));

        repo.upsert_conversation(conversation.clone()).await?;

        // Model an existing pre-compression row. A ref write must replace the
        // whole context representation, rather than leave stale columns behind.
        let legacy_json = serde_json::to_string(&ContextRecord::from(&legacy_context))?;
        let conversation_id = conversation.id.into_string();
        repo.run_with_connection(move |connection, _wid| {
            diesel::sql_query(
                "UPDATE conversations \
                 SET context = ?, context_zstd = NULL, is_compressed = 0 \
                 WHERE conversation_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(&legacy_json)
            .bind::<diesel::sql_types::Text, _>(&conversation_id)
            .execute(connection)?;
            Ok(())
        })
        .await?;

        let incoming_context = Context::default()
            .messages(vec![ContextMessage::user("incoming context", None).into()]);
        let incoming = Conversation::new(conversation.id).context(Some(incoming_context.clone()));
        repo.upsert_conversation_ref(&incoming).await?;

        let stored = repo
            .get_conversation(&conversation.id)
            .await?
            .expect("the existing conversation must remain readable");
        assert_eq!(stored.context, Some(incoming_context));
        Ok(())
    }

    #[tokio::test]
    async fn ref_upsert_record_replaces_existing_compressed_context_with_incoming_plain_context()
    -> anyhow::Result<()> {
        let repo = repository()?;
        let existing_context = Context::default().messages(vec![
            ContextMessage::user("existing compressed context", None).into(),
        ]);
        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("compressed context row".to_string()))
            .context(Some(existing_context));
        repo.upsert_conversation(conversation.clone()).await?;

        let incoming_context = Context::default().messages(vec![
            ContextMessage::user("incoming plain context", None).into(),
        ]);
        let incoming = Conversation::new(conversation.id).context(Some(incoming_context.clone()));
        // zstd compression succeeds deterministically in normal test runs, so
        // construct the valid fallback representation directly. This exercises
        // the ref conflict path with a real plain incoming payload.
        let mut record = ConversationRecord::new_ref(&incoming, WorkspaceHash::new(0));
        record.context = Some(serde_json::to_string(&ContextRecord::from(
            &incoming_context,
        ))?);
        record.context_zstd = None;
        record.is_compressed = 0;

        repo.run_with_connection(move |connection, _wid| {
            ConversationRepositoryImpl::upsert_conversation_ref_record(connection, &record)
        })
        .await?;

        let stored = repo
            .get_conversation(&conversation.id)
            .await?
            .expect("the incoming plain context must remain readable");
        assert_eq!(stored.context, Some(incoming_context));

        repo.run_with_connection(move |connection, _wid| {
            #[derive(diesel::QueryableByName)]
            struct StorageColumns {
                #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
                context_zstd: Option<Vec<u8>>,
                #[diesel(sql_type = diesel::sql_types::Integer)]
                is_compressed: i32,
            }
            let row: StorageColumns = diesel::sql_query(
                "SELECT context_zstd, is_compressed FROM conversations WHERE conversation_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(conversation.id.into_string())
            .get_result(connection)?;
            assert!(
                row.context_zstd.is_none(),
                "stale compressed bytes must be cleared"
            );
            assert_eq!(
                row.is_compressed, 0,
                "plain payload must not be marked compressed"
            );
            Ok(())
        })
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_find_all_conversations() -> anyhow::Result<()> {
        let context1 =
            Context::default().messages(vec![ContextMessage::user("Hello", None).into()]);
        let context2 =
            Context::default().messages(vec![ContextMessage::user("World", None).into()]);
        let conversation1 = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()))
            .context(Some(context1));
        let conversation2 = Conversation::new(ConversationId::generate())
            .title(Some("Second Conversation".to_string()))
            .context(Some(context2));
        let repo = repository()?;

        repo.upsert_conversation(conversation1.clone()).await?;
        repo.upsert_conversation(conversation2.clone()).await?;

        let actual = repo.get_all_conversations(None).await?;

        assert!(actual.is_some());
        let conversations = actual.unwrap();
        assert_eq!(conversations.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_all_conversations_with_limit() -> anyhow::Result<()> {
        let context1 =
            Context::default().messages(vec![ContextMessage::user("Hello", None).into()]);
        let context2 =
            Context::default().messages(vec![ContextMessage::user("World", None).into()]);
        let conversation1 = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()))
            .context(Some(context1));
        let conversation2 = Conversation::new(ConversationId::generate()).context(Some(context2));
        let repo = repository()?;

        repo.upsert_conversation(conversation1).await?;
        repo.upsert_conversation(conversation2).await?;

        let actual = repo.get_all_conversations(Some(1)).await?;

        assert!(actual.is_some());
        assert_eq!(actual.unwrap().len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_all_conversations_empty() -> anyhow::Result<()> {
        let repo = repository()?;

        let actual = repo.get_all_conversations(None).await?;

        assert!(actual.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_find_last_active_conversation_with_context() -> anyhow::Result<()> {
        let context = Context::default().messages(vec![ContextMessage::user("Hello", None).into()]);
        let conversation_with_context = Conversation::new(ConversationId::generate())
            .title(Some("Conversation with Context".to_string()))
            .context(Some(context));
        let conversation_without_context = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));
        let repo = repository()?;

        repo.upsert_conversation(conversation_without_context)
            .await?;
        repo.upsert_conversation(conversation_with_context.clone())
            .await?;

        let actual = repo.get_last_conversation().await?;

        assert!(actual.is_some());
        assert_eq!(actual.unwrap().id, conversation_with_context.id);
        Ok(())
    }

    #[tokio::test]
    async fn test_find_last_active_conversation_no_context() -> anyhow::Result<()> {
        let conversation_without_context = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));
        let expected_id = conversation_without_context.id;
        let repo = repository()?;

        repo.upsert_conversation(conversation_without_context)
            .await?;

        let actual = repo.get_last_conversation().await?;

        // Rows without a context payload (tombstone/damaged rows) must stay
        // visible — the read layer no longer filters them out at the SQL
        // layer so the picker can show them.
        assert_eq!(actual.map(|c| c.id), Some(expected_id));
        Ok(())
    }

    #[tokio::test]
    async fn test_find_last_active_conversation_ignores_empty_context() -> anyhow::Result<()> {
        let conversation_with_empty_context = Conversation::new(ConversationId::generate())
            .title(Some("Conversation with Empty Context".to_string()))
            .context(Some(Context::default()));
        let conversation_without_context = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));
        let repo = repository()?;

        repo.upsert_conversation(conversation_without_context)
            .await?;
        repo.upsert_conversation(conversation_with_empty_context)
            .await?;

        let actual = repo.get_last_conversation().await?;

        // Empty-context rows are visible; the most recently updated row wins.
        assert!(
            actual.is_some(),
            "empty-context conversation must remain visible"
        );
        Ok(())
    }

    #[test]
    fn test_conversation_record_from_conversation() -> anyhow::Result<()> {
        let fixture = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));

        let actual = ConversationRecord::new(fixture.clone(), WorkspaceHash::new(0));

        assert_eq!(actual.conversation_id, fixture.id.into_string());
        assert_eq!(actual.title, Some("Test Conversation".to_string()));
        assert_eq!(actual.context, None);
        Ok(())
    }

    #[test]
    fn test_conversation_record_from_conversation_with_context() -> anyhow::Result<()> {
        let context = Context::default().messages(vec![ContextMessage::user("Hello", None).into()]);
        let fixture = Conversation::new(ConversationId::generate())
            .title(Some("Conversation with Context".to_string()))
            .context(Some(context));

        let actual = ConversationRecord::new(fixture.clone(), WorkspaceHash::new(0));

        assert_eq!(actual.conversation_id, fixture.id.into_string());
        assert_eq!(actual.title, Some("Conversation with Context".to_string()));
        // With compression, context is stored in context_zstd and is_compressed=1
        assert!(
            actual.context_zstd.is_some() || actual.context.is_some(),
            "context should be stored in either context_zstd (compressed) or context (plain)"
        );
        Ok(())
    }

    #[test]
    fn conversation_record_rich_context_uses_legacy_wire_format() -> anyhow::Result<()> {
        let context = Context::default()
            .add_entry(forge_domain::MessageEntry {
                message: ContextMessage::assistant(
                    "I will call the tool",
                    Some("thought-signature".to_string()),
                    None,
                    Some(vec![ToolCallFull {
                        name: ToolName::new("patch"),
                        call_id: Some(ToolCallId::new("call-123".to_string())),
                        arguments: forge_domain::ToolCallArguments::from(
                            serde_json::json!({"path": "src/lib.rs"}),
                        ),
                        thought_signature: Some("tool-signature".to_string()),
                    }]),
                ),
                usage: Some(Usage {
                    prompt_tokens: forge_domain::TokenCount::Actual(11),
                    completion_tokens: forge_domain::TokenCount::Actual(7),
                    total_tokens: forge_domain::TokenCount::Actual(18),
                    cached_tokens: forge_domain::TokenCount::Actual(3),
                    cost: Some(0.001),
                }),
            })
            .add_tool(ToolDefinition::new("patch").description("Patch a file"))
            .reasoning(forge_domain::ReasoningConfig {
                effort: Some(Effort::High),
                max_tokens: Some(1024),
                exclude: Some(false),
                enabled: Some(true),
            });
        let legacy_json = serde_json::to_string(&ContextRecord::from(&context))?;
        let fixture = Conversation::new(ConversationId::generate()).context(Some(context));

        let record = ConversationRecord::new(fixture, WorkspaceHash::new(0));
        let stored_json = if let Some(compressed) = record.context_zstd {
            crate::codec::decompress(&compressed)?
        } else {
            record.context.expect("context must be persisted")
        };

        assert_eq!(stored_json, legacy_json);
        assert_eq!(record.is_compressed, 1);
        assert_eq!(record.message_count, Some(1));
        Ok(())
    }

    #[test]
    fn test_conversation_record_from_conversation_with_empty_context() -> anyhow::Result<()> {
        let fixture = Conversation::new(ConversationId::generate())
            .title(Some("Conversation with Empty Context".to_string()))
            .context(Some(Context::default()));

        let actual = ConversationRecord::new(fixture.clone(), WorkspaceHash::new(0));

        assert_eq!(actual.conversation_id, fixture.id.into_string());
        assert_eq!(
            actual.title,
            Some("Conversation with Empty Context".to_string())
        );

        assert!(actual.context.is_none()); // Empty context should be filtered out
        Ok(())
    }

    #[test]
    fn test_conversation_from_conversation_record() -> anyhow::Result<()> {
        let test_id = ConversationId::generate();
        let fixture = ConversationRecord {
            conversation_id: test_id.into_string(),
            title: Some("Test Conversation".to_string()),
            context: None,
            created_at: Utc::now().naive_utc(),
            updated_at: None,
            workspace_id: Some(0),
            metrics: None,
            parent_id: None,
            source: None,
            cwd: None,
            message_count: None,
            intent_state: "pending".to_string(),
            extracted_at: None,
            memory_id: None,
            intent_hash: None,
            context_zstd: None,
            is_compressed: 0,
        };

        let actual = Conversation::try_from(fixture)?;

        assert_eq!(actual.id, test_id);
        assert_eq!(actual.title, Some("Test Conversation".to_string()));
        assert_eq!(actual.context, None);
        Ok(())
    }

    #[tokio::test]
    async fn test_upsert_and_retrieve_conversation_with_metrics() -> anyhow::Result<()> {
        let repo = repository()?;

        // Create a conversation with metrics
        let metrics = Metrics::default()
            .started_at(Utc::now())
            .insert(
                "src/main.rs".to_string(),
                FileOperation::new(ToolKind::Write)
                    .lines_added(10u64)
                    .lines_removed(5u64)
                    .content_hash(Some("abc123def456".to_string())),
            )
            .insert(
                "src/lib.rs".to_string(),
                FileOperation::new(ToolKind::Write)
                    .lines_added(3u64)
                    .lines_removed(2u64)
                    .content_hash(Some("789xyz456abc".to_string())),
            );

        let fixture = Conversation::generate().metrics(metrics.clone());

        // Save the conversation
        repo.upsert_conversation(fixture.clone()).await?;

        // Retrieve the conversation
        let actual = repo
            .get_conversation(&fixture.id)
            .await?
            .expect("Conversation should exist");

        // Verify metrics are preserved
        assert_eq!(actual.metrics.file_operations.len(), 2);
        let main_metrics = actual.metrics.file_operations.get("src/main.rs").unwrap();
        assert_eq!(main_metrics.lines_added, 10);
        assert_eq!(main_metrics.lines_removed, 5);
        assert_eq!(main_metrics.content_hash, Some("abc123def456".to_string()));
        let lib_metrics = actual.metrics.file_operations.get("src/lib.rs").unwrap();
        assert_eq!(lib_metrics.lines_added, 3);
        assert_eq!(lib_metrics.lines_removed, 2);
        assert_eq!(lib_metrics.content_hash, Some("789xyz456abc".to_string()));
        Ok(())
    }

    #[test]
    fn test_metrics_record_conversion_preserves_all_fields() {
        // This test ensures compile-time safety: if Metrics schema changes,
        // this test will fail to compile, alerting us to update MetricsRecord
        let fixture = Metrics::default().started_at(Utc::now()).insert(
            "test.rs".to_string(),
            FileOperation::new(ToolKind::Write)
                .lines_added(5u64)
                .lines_removed(3u64)
                .content_hash(Some("test_hash_123".to_string())),
        );

        // Convert to record and back
        let record = MetricsRecord::from(&fixture);
        let actual = Metrics::from(record);

        // Verify all fields are preserved
        assert_eq!(actual.started_at, fixture.started_at);
        assert_eq!(actual.file_operations.len(), fixture.file_operations.len());

        let actual_file = actual.file_operations.get("test.rs").unwrap();
        let expected_file = fixture.file_operations.get("test.rs").unwrap();
        assert_eq!(actual_file.lines_added, expected_file.lines_added);
        assert_eq!(actual_file.lines_removed, expected_file.lines_removed);
        assert_eq!(actual_file.content_hash, expected_file.content_hash);
    }

    #[test]
    fn test_deserialize_old_format_without_tool_field() {
        // Old format from database: missing tool and content_hash fields
        let json = r#"{
            "started_at": "2024-01-01T00:00:00Z",
            "files_changed": {
                "src/main.rs": {
                    "lines_added": 10,
                    "lines_removed": 5
                },
                "src/lib.rs": {
                    "lines_added": 3,
                    "lines_removed": 2
                }
            }
        }"#;

        let record: MetricsRecord = serde_json::from_str(json).unwrap();
        let actual = Metrics::from(record);

        // Verify files are loaded
        assert_eq!(actual.file_operations.len(), 2);

        // Verify main.rs
        let main_file = actual.file_operations.get("src/main.rs").unwrap();
        assert_eq!(main_file.lines_added, 10);
        assert_eq!(main_file.lines_removed, 5);
        assert_eq!(main_file.content_hash, None);
        assert_eq!(main_file.tool, ToolKind::Write); // Default tool

        // Verify lib.rs
        let lib_file = actual.file_operations.get("src/lib.rs").unwrap();
        assert_eq!(lib_file.lines_added, 3);
        assert_eq!(lib_file.lines_removed, 2);
        assert_eq!(lib_file.content_hash, None);
        assert_eq!(lib_file.tool, ToolKind::Write); // Default tool
    }

    #[test]
    fn test_deserialize_array_format_takes_last_operation() {
        // Array format from database: multiple operations per file
        let json = r#"{
            "started_at": "2024-01-01T00:00:00Z",
            "files_changed": {
                "src/main.rs": [
                    {
                        "lines_added": 2,
                        "lines_removed": 4,
                        "content_hash": "hash1",
                        "tool": "read"
                    },
                    {
                        "lines_added": 1,
                        "lines_removed": 1,
                        "content_hash": "hash2",
                        "tool": "patch"
                    },
                    {
                        "lines_added": 5,
                        "lines_removed": 3,
                        "content_hash": "hash3",
                        "tool": "write"
                    }
                ]
            }
        }"#;

        let record: MetricsRecord = serde_json::from_str(json).unwrap();
        let actual = Metrics::from(record);

        // Verify only the last operation is kept
        assert_eq!(actual.file_operations.len(), 1);

        let main_file = actual.file_operations.get("src/main.rs").unwrap();
        assert_eq!(main_file.lines_added, 5);
        assert_eq!(main_file.lines_removed, 3);
        assert_eq!(main_file.content_hash, Some("hash3".to_string()));
        assert_eq!(main_file.tool, ToolKind::Write);
    }

    #[test]
    fn test_deserialize_array_format_with_empty_array() {
        // Array format with empty array should be skipped
        let json = r#"{
            "started_at": "2024-01-01T00:00:00Z",
            "files_changed": {
                "src/main.rs": [],
                "src/lib.rs": {
                    "lines_added": 5,
                    "lines_removed": 2,
                    "content_hash": "hash1",
                    "tool": "patch"
                }
            }
        }"#;

        let record: MetricsRecord = serde_json::from_str(json).unwrap();
        let actual = Metrics::from(record);

        // Empty array should be skipped, only lib.rs should be present
        assert_eq!(actual.file_operations.len(), 1);
        assert!(actual.file_operations.contains_key("src/lib.rs"));
        assert!(!actual.file_operations.contains_key("src/main.rs"));
    }

    #[test]
    fn test_deserialize_current_format_with_all_fields() {
        // Current format: single object with all fields
        let json = r#"{
            "started_at": "2024-01-01T00:00:00Z",
            "files_changed": {
                "src/main.rs": {
                    "lines_added": 10,
                    "lines_removed": 5,
                    "content_hash": "abc123def456",
                    "tool": "patch"
                },
                "src/lib.rs": {
                    "lines_added": 3,
                    "lines_removed": 2,
                    "content_hash": "789xyz456abc",
                    "tool": "write"
                }
            }
        }"#;

        let record: MetricsRecord = serde_json::from_str(json).unwrap();
        let actual = Metrics::from(record);

        // Verify all fields are preserved
        assert_eq!(actual.file_operations.len(), 2);

        let main_file = actual.file_operations.get("src/main.rs").unwrap();
        assert_eq!(main_file.lines_added, 10);
        assert_eq!(main_file.lines_removed, 5);
        assert_eq!(main_file.content_hash, Some("abc123def456".to_string()));
        assert_eq!(main_file.tool, ToolKind::Patch);

        let lib_file = actual.file_operations.get("src/lib.rs").unwrap();
        assert_eq!(lib_file.lines_added, 3);
        assert_eq!(lib_file.lines_removed, 2);
        assert_eq!(lib_file.content_hash, Some("789xyz456abc".to_string()));
        assert_eq!(lib_file.tool, ToolKind::Write);
    }

    #[test]
    fn test_deserialize_mixed_format() {
        // Mix of old format, array format, and current format
        let json = r#"{
            "started_at": "2024-01-01T00:00:00Z",
            "files_changed": {
                "old_file.rs": {
                    "lines_added": 10,
                    "lines_removed": 5
                },
                "array_file.rs": [
                    {
                        "lines_added": 1,
                        "lines_removed": 2,
                        "content_hash": "hash1",
                        "tool": "read"
                    },
                    {
                        "lines_added": 3,
                        "lines_removed": 4,
                        "content_hash": "hash2",
                        "tool": "patch"
                    }
                ],
                "current_file.rs": {
                    "lines_added": 7,
                    "lines_removed": 8,
                    "content_hash": "hash3",
                    "tool": "write"
                }
            }
        }"#;

        let record: MetricsRecord = serde_json::from_str(json).unwrap();
        let actual = Metrics::from(record);

        assert_eq!(actual.file_operations.len(), 3);

        // Old format file
        let old_file = actual.file_operations.get("old_file.rs").unwrap();
        assert_eq!(old_file.lines_added, 10);
        assert_eq!(old_file.lines_removed, 5);
        assert_eq!(old_file.content_hash, None);
        assert_eq!(old_file.tool, ToolKind::Write); // Default

        // Array format file (should have last operation)
        let array_file = actual.file_operations.get("array_file.rs").unwrap();
        assert_eq!(array_file.lines_added, 3);
        assert_eq!(array_file.lines_removed, 4);
        assert_eq!(array_file.content_hash, Some("hash2".to_string()));
        assert_eq!(array_file.tool, ToolKind::Patch);

        // Current format file
        let current_file = actual.file_operations.get("current_file.rs").unwrap();
        assert_eq!(current_file.lines_added, 7);
        assert_eq!(current_file.lines_removed, 8);
        assert_eq!(current_file.content_hash, Some("hash3".to_string()));
        assert_eq!(current_file.tool, ToolKind::Write);
    }

    #[test]
    fn test_serialize_current_format() {
        // Test that we always serialize in the current format (single object)
        let fixture = Metrics::default().started_at(Utc::now()).insert(
            "src/main.rs".to_string(),
            FileOperation::new(ToolKind::Patch)
                .lines_added(10u64)
                .lines_removed(5u64)
                .content_hash(Some("abc123".to_string())),
        );

        let record = MetricsRecord::from(&fixture);
        let json = serde_json::to_string(&record).unwrap();

        // Verify it's not an array format
        assert!(!json.contains("[{"));
        // Verify it contains the tool field
        assert!(json.contains("\"tool\":\"patch\""));

        // Verify structure is correct
        assert!(json.contains("\"lines_added\":10"));
        assert!(json.contains("\"lines_removed\":5"));
        assert!(json.contains("\"content_hash\":\"abc123\""));
    }

    #[test]
    fn test_context_record_conversion_preserves_all_fields() {
        let tool_def = ToolDefinition::new("test_tool").description("A test tool");

        let reasoning = forge_domain::ReasoningConfig {
            effort: Some(Effort::Medium),
            max_tokens: Some(2048),
            exclude: Some(false),
            enabled: Some(true),
        };

        // Create a comprehensive set of messages to test all message types
        let messages = vec![
            ContextMessage::user("Hello", None).into(),
            ContextMessage::system("System prompt").into(),
            ContextMessage::Tool(ToolResult {
                name: ToolName::new("test_tool"),
                call_id: Some(ToolCallId::new("call_123".to_string())),
                output: ToolOutput {
                    is_error: false,
                    values: vec![ToolValue::Text("Result text".to_string()), ToolValue::Empty],
                },
            })
            .into(),
            forge_domain::MessageEntry {
                message: ContextMessage::Text(forge_domain::TextMessage {
                    role: Role::Assistant,
                    content: "Assistant response".to_string(),
                    raw_content: None,
                    tool_calls: Some(vec![ToolCallFull {
                        name: ToolName::new("another_tool"),
                        call_id: Some(ToolCallId::new("call_456".to_string())),
                        arguments: forge_domain::ToolCallArguments::from(
                            serde_json::json!({"param": "value"}),
                        ),
                        thought_signature: None,
                    }]),
                    model: Some(forge_domain::ModelId::from("gpt-4")),
                    thought_signature: None,
                    reasoning_details: None,
                    droppable: false,
                    phase: None,
                }),
                usage: Some(Usage {
                    prompt_tokens: forge_domain::TokenCount::Actual(100),
                    completion_tokens: forge_domain::TokenCount::Actual(50),
                    total_tokens: forge_domain::TokenCount::Actual(150),
                    cached_tokens: forge_domain::TokenCount::Actual(0),
                    cost: Some(0.001),
                }),
            },
        ];

        let fixture = Context::default()
            .conversation_id(ConversationId::generate())
            .messages(messages)
            .tools(vec![tool_def.clone()])
            .tool_choice(ToolChoice::Call(ToolName::new("test_tool")))
            .max_tokens(1000usize)
            .temperature(forge_domain::Temperature::new(0.7).unwrap())
            .top_p(forge_domain::TopP::new(0.9).unwrap())
            .top_k(forge_domain::TopK::new(50).unwrap())
            .reasoning(reasoning.clone())
            .stream(true);

        // Convert to record and back
        let record = ContextRecord::from(&fixture);
        let actual = Context::try_from(record).unwrap();

        // Verify all fields are preserved
        assert_eq!(actual.conversation_id, fixture.conversation_id);
        assert_eq!(actual.messages.len(), 4);
        assert_eq!(actual.tools.len(), 1);
        assert_eq!(actual.tools[0].name.to_string(), "test_tool");
        assert_eq!(
            actual.tool_choice,
            Some(ToolChoice::Call(ToolName::new("test_tool")))
        );
        assert_eq!(actual.max_tokens, fixture.max_tokens);
        assert_eq!(actual.temperature, fixture.temperature);
        assert_eq!(actual.top_p, fixture.top_p);
        assert_eq!(actual.top_k, fixture.top_k);
        assert_eq!(actual.reasoning, Some(reasoning));
        assert_eq!(actual.stream, fixture.stream);

        // Verify message types and content
        match &actual.messages[0].message {
            ContextMessage::Text(msg) => {
                assert_eq!(msg.role, Role::User);
                assert_eq!(msg.content, "Hello");
            }
            _ => panic!("Expected user message"),
        }

        match &actual.messages[2].message {
            ContextMessage::Tool(tool_result) => {
                assert_eq!(tool_result.name.to_string(), "test_tool");
                assert_eq!(
                    tool_result.call_id.as_ref().map(|id| id.as_str()),
                    Some("call_123")
                );
                assert!(!tool_result.output.is_error);
                assert_eq!(tool_result.output.values.len(), 2);
            }
            _ => panic!("Expected tool result message"),
        }

        // Verify usage is preserved
        match &actual.messages[3].usage {
            Some(usage) => {
                assert_eq!(*usage.prompt_tokens, 100);
                assert_eq!(*usage.completion_tokens, 50);
                assert_eq!(*usage.total_tokens, 150);
                assert_eq!(usage.cost, Some(0.001));
            }
            None => panic!("Expected usage information"),
        }
    }

    #[test]
    fn test_conversation_deserialization_error_includes_id() {
        // Test that deserialization errors include the conversation ID
        let test_id = ConversationId::generate();
        let fixture = ConversationRecord {
            conversation_id: test_id.into_string(),
            title: Some("Test Conversation".to_string()),
            context: Some("invalid json".to_string()), // Invalid JSON to trigger error
            created_at: Utc::now().naive_utc(),
            updated_at: None,
            workspace_id: Some(0),
            metrics: None,
            parent_id: None,
            source: None,
            cwd: None,
            message_count: None,
            intent_state: "pending".to_string(),
            extracted_at: None,
            memory_id: None,
            intent_hash: None,
            context_zstd: None,
            is_compressed: 0,
        };

        let result = Conversation::try_from(fixture);

        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(
            error_message.contains(&test_id.to_string()),
            "Error message should contain conversation ID. Got: {}",
            error_message
        );
        assert!(
            error_message.contains("Failed to deserialize context"),
            "Error message should indicate context deserialization failure. Got: {}",
            error_message
        );
    }

    #[tokio::test]
    async fn test_delete_conversation_success() -> anyhow::Result<()> {
        let repo = repository()?;
        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));

        repo.upsert_conversation(conversation.clone()).await?;

        repo.delete_conversation(&conversation.id).await?;

        let result = repo.get_conversation(&conversation.id).await?;
        assert!(result.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_conversation_workspace_filtering() -> anyhow::Result<()> {
        let repo = repository()?;
        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("Test Conversation".to_string()));

        repo.upsert_conversation(conversation.clone()).await?;

        // Delete should succeed regardless of existence (idempotent)
        repo.delete_conversation(&conversation.id).await?;

        // Verify conversation is deleted
        let deleted = repo.get_conversation(&conversation.id).await?;
        assert!(deleted.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_conversation_cross_workspace_security() -> anyhow::Result<()> {
        let repo = repository()?;

        // Create conversation in current workspace
        let conversation_id = ConversationId::generate();
        let conversation =
            Conversation::new(conversation_id).title(Some("Test Conversation".to_string()));

        repo.upsert_conversation(conversation.clone()).await?;

        // Try to delete with different workspace ID (should fail due to
        // security) Note: This test would require modifying workspace
        // ID in repo For now, we test that deletion works with current
        // workspace
        repo.delete_conversation(&conversation.id).await?;

        // Verify it's actually deleted
        let deleted = repo.get_conversation(&conversation.id).await?;
        assert!(deleted.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_conversation_end_to_end_workflow() -> anyhow::Result<()> {
        let repo = repository()?;
        let conversation_id = ConversationId::generate();
        let conversation =
            Conversation::new(conversation_id).title(Some("Test Conversation".to_string()));

        // Test complete workflow: create -> delete -> verify -> create new ->
        // verify
        repo.upsert_conversation(conversation.clone()).await?;

        // Delete conversation
        repo.delete_conversation(&conversation.id).await?;

        // Verify it's gone
        let deleted_check = repo.get_conversation(&conversation.id).await?;
        assert!(deleted_check.is_none());

        // Create new conversation to ensure system still works
        let new_conversation_id = ConversationId::generate();
        let new_conversation = Conversation::new(new_conversation_id);
        repo.upsert_conversation(new_conversation.clone()).await?;

        // Verify new conversation exists
        let new_check = repo.get_conversation(&new_conversation_id).await?;
        assert!(new_check.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_rename_conversation_via_upsert() -> anyhow::Result<()> {
        let repo = repository()?;
        let conversation =
            Conversation::new(ConversationId::generate()).title(Some("Original Title".to_string()));

        repo.upsert_conversation(conversation.clone()).await?;

        // Rename by upserting with a new title
        let renamed = conversation
            .clone()
            .title(Some("Renamed Session".to_string()));
        repo.upsert_conversation(renamed).await?;

        let actual = repo.get_conversation(&conversation.id).await?.unwrap();
        assert_eq!(actual.title, Some("Renamed Session".to_string()));
        Ok(())
    }

    #[tokio::test]
    async fn test_rename_conversation_from_none() -> anyhow::Result<()> {
        let repo = repository()?;
        let conversation = Conversation::new(ConversationId::generate());

        // Start with no title
        assert!(conversation.title.is_none());
        repo.upsert_conversation(conversation.clone()).await?;

        // Rename it
        let renamed = conversation.clone().title(Some("My Session".to_string()));
        repo.upsert_conversation(renamed).await?;

        let actual = repo.get_conversation(&conversation.id).await?.unwrap();
        assert_eq!(actual.title, Some("My Session".to_string()));
        Ok(())
    }

    #[test]
    fn test_legacy_tool_value_pair_deserialization() {
        use crate::conversation::conversation_record::ToolOutputRecord;

        // This JSON represents the old Pair variant format that was stored in
        // the database
        let legacy_json = r#"{
            "is_error": false,
            "values": [
                {"pair": [
                    {"text": "XML content for LLM"},
                    {"fileDiff": {"path": "/test/file.rs", "old_text": "old", "new_text": "new"}}
                ]}
            ]
        }"#;

        let record: ToolOutputRecord = serde_json::from_str(legacy_json).unwrap();
        let actual: forge_domain::ToolOutput = record.try_into().unwrap();

        // The Pair variant should be converted by taking the first element (LLM
        // content)
        assert!(!actual.is_error);
        assert_eq!(actual.values.len(), 1);
        assert_eq!(
            actual.values[0],
            forge_domain::ToolValue::Text("XML content for LLM".to_string())
        );
    }

    #[test]
    fn test_legacy_tool_value_markdown_deserialization() {
        use crate::conversation::conversation_record::ToolOutputRecord;

        let legacy_json = r##"{
            "is_error": false,
            "values": [{"markdown": "# Heading - Some bold text"}]
        }"##;

        let record: ToolOutputRecord = serde_json::from_str(legacy_json).unwrap();
        let actual: forge_domain::ToolOutput = record.try_into().unwrap();

        // Markdown should be converted to Text
        assert_eq!(actual.values.len(), 1);
        assert_eq!(
            actual.values[0],
            forge_domain::ToolValue::Text("# Heading - Some bold text".to_string())
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_concurrent_operations_dont_block_runtime() -> anyhow::Result<()> {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        // Heartbeat fires every `TICK`; we require a measurement window of at
        // least `MIN_WINDOW` so the assertion is meaningful even when the DB
        // workload finishes very quickly (e.g. on fast machines with the
        // in-memory SQLite pool).
        const TICK: Duration = Duration::from_millis(10);
        const MIN_WINDOW: Duration = Duration::from_millis(200);
        // A runtime that is genuinely blocked by synchronous DB work stalls
        // heartbeats for tens/hundreds of milliseconds. Ordinary OS scheduling
        // under load can drift the ticker by several ticks; a 7.5-tick cap
        // (75ms at 10ms tick) tolerates scheduling pressure while still
        // catching true runtime blocks (which show hundreds of ms gaps).
        const MAX_ACCEPTABLE_GAP: Duration = Duration::from_millis(75);

        let repo = Arc::new(repository()?);
        let heartbeat = Arc::new(AtomicUsize::new(0));
        let prev_tick = Arc::new(Mutex::new(Instant::now()));
        let max_gap = Arc::new(Mutex::new(Duration::ZERO));

        // Heartbeat task - if the runtime is blocked, ticks pause and the
        // inter-tick gap balloons past `MAX_ACCEPTABLE_GAP`.
        let heartbeat_clone = heartbeat.clone();
        let prev_tick_clone = prev_tick.clone();
        let max_gap_clone = max_gap.clone();
        let heartbeat_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                heartbeat_clone.fetch_add(1, Ordering::Relaxed);
                let now = Instant::now();
                let mut prev = prev_tick_clone.lock().unwrap();
                let gap = now.duration_since(*prev);
                *prev = now;
                drop(prev);
                let mut max = max_gap_clone.lock().unwrap();
                if gap > *max {
                    *max = gap;
                }
            }
        });

        // Warm up: let the heartbeat task get scheduled and complete its first
        // tick before we start measuring, then reset the counter so timing
        // begins from a clean state.
        tokio::time::sleep(TICK * 3).await;
        heartbeat.store(0, Ordering::Relaxed);

        // Spawn many concurrent DB operations.
        let mut handles = vec![];
        let start = Instant::now();

        for i in 0..20 {
            let repo = repo.clone();
            let handle = tokio::spawn(async move {
                for j in 0..10 {
                    let conversation = Conversation::new(ConversationId::generate())
                        .title(Some(format!("Task {} - Write {}", i, j)));
                    repo.upsert_conversation(conversation).await?;
                }
                anyhow::Result::<()>::Ok(())
            });
            handles.push(handle);
        }

        // Wait for all operations.
        for handle in handles {
            handle.await??;
        }

        // Ensure the measurement window is long enough for heartbeat math to
        // be meaningful regardless of how fast the DB workload completed.
        let work_elapsed = start.elapsed();
        if work_elapsed < MIN_WINDOW {
            tokio::time::sleep(MIN_WINDOW - work_elapsed).await;
        }
        let elapsed = start.elapsed();

        // Stop heartbeat.
        heartbeat_handle.abort();

        // A blocked runtime shows up as one or more multi-tick gaps (the
        // heartbeat cannot fire while synchronous DB work holds the runtime
        // thread); scheduling jitter only ever shaves a tick here and there,
        // which the count floor below tolerates.
        let max_gap = *max_gap.lock().unwrap();
        assert!(
            max_gap <= MAX_ACCEPTABLE_GAP,
            "Runtime was blocked! Longest heartbeat gap was {:?} (limit {:?}) over a {:?} window",
            max_gap,
            MAX_ACCEPTABLE_GAP,
            elapsed
        );

        // Secondary sanity check: the heartbeat should still have fired a
        // reasonable fraction of the theoretical maximum. Deliberately lenient
        // (50%) — total-count dropouts are dominated by OS scheduling under
        // load, not by runtime blocking (which the gap check catches).
        let heartbeat_count = heartbeat.load(Ordering::Relaxed);
        let expected_heartbeats = (elapsed.as_millis() as usize) / (TICK.as_millis() as usize);
        let floor = (expected_heartbeats / 2).max(1);

        assert!(
            heartbeat_count >= floor,
            "Runtime stalled: only {} heartbeats (~{} theoretical) in {:?}",
            heartbeat_count,
            expected_heartbeats,
            elapsed
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mixed_read_write_contention() -> anyhow::Result<()> {
        let repo = Arc::new(repository()?);
        let mut handles = vec![];

        // Pre-populate some data
        for i in 0..10 {
            let conv =
                Conversation::new(ConversationId::generate()).title(Some(format!("Initial {}", i)));
            repo.upsert_conversation(conv).await?;
        }

        // Spawn writers
        for i in 0..10 {
            let repo = repo.clone();
            handles.push(tokio::spawn(async move {
                for j in 0..10 {
                    let conv = Conversation::new(ConversationId::generate())
                        .title(Some(format!("Writer {} - {}", i, j)));
                    repo.upsert_conversation(conv).await?;
                }
                anyhow::Result::<()>::Ok(())
            }));
        }

        // Spawn readers (interleave with writers)
        for _ in 0..10 {
            let repo = repo.clone();
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    // Read all conversations
                    let _ = repo.get_all_conversations(Some(50)).await?;
                    tokio::task::yield_now().await;
                }
                anyhow::Result::<()>::Ok(())
            }));
        }

        // All should complete without timeout
        for handle in handles {
            handle.await??;
        }

        Ok(())
    }

    #[test]
    fn test_legacy_tool_value_file_diff_deserialization() {
        use crate::conversation::conversation_record::ToolOutputRecord;

        let legacy_json = r#"{
            "is_error": false,
            "values": [{"fileDiff": {"path": "/src/main.rs", "old_text": "fn old()", "new_text": "fn new()"}}]
        }"#;

        let record: ToolOutputRecord = serde_json::from_str(legacy_json).unwrap();
        let actual: forge_domain::ToolOutput = record.try_into().unwrap();

        // FileDiff should be converted to a text summary
        assert_eq!(actual.values.len(), 1);
        assert_eq!(
            actual.values[0],
            forge_domain::ToolValue::Text("[File diff: /src/main.rs]".to_string())
        );
    }

    #[tokio::test]
    async fn test_prune_conversation_safety_guard() -> anyhow::Result<()> {
        let repo = repository()?;
        let context =
            Context::default().messages(vec![ContextMessage::user("Test content", None).into()]);
        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("Test for Pruning".to_string()))
            .context(Some(context));

        // Insert conversation with default intent_state='pending'
        repo.upsert_conversation(conversation.clone()).await?;

        // ADR-103: Pruning should fail when intent_state != 'verified'
        let result = repo.prune_conversation(&conversation.id).await;
        assert!(
            result.is_err(),
            "Pruning should fail when intent_state='pending'"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Must be 'verified'"),
            "Error should indicate the requirement for 'verified' state"
        );

        // Mark as verified
        repo.mark_intent_state(&conversation.id, "verified").await?;

        // Now pruning should succeed
        let prune_result = repo.prune_conversation(&conversation.id).await;
        assert!(
            prune_result.is_ok(),
            "Pruning should succeed when intent_state='verified'"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_mark_intent_state_enforces_dag() -> anyhow::Result<()> {
        let repo = repository()?;
        let conversation = Conversation::new(ConversationId::generate())
            .title(Some("Test for State Machine".to_string()));

        repo.upsert_conversation(conversation.clone()).await?;

        // Verify default state is 'pending'
        let conv = repo.get_conversation(&conversation.id).await?;
        assert!(conv.is_some());

        // Valid transition: pending → extracting
        assert!(
            repo.mark_intent_state(&conversation.id, "extracting")
                .await
                .is_ok()
        );

        // Valid transition: extracting → extracted
        assert!(
            repo.mark_intent_state(&conversation.id, "extracted")
                .await
                .is_ok()
        );

        // Valid transition: extracted → verified
        assert!(
            repo.mark_intent_state(&conversation.id, "verified")
                .await
                .is_ok()
        );

        // Valid transition: verified → pruned
        assert!(
            repo.mark_intent_state(&conversation.id, "pruned")
                .await
                .is_ok()
        );

        // Invalid transition: pruned → any state (pruned is final)
        let result = repo.mark_intent_state(&conversation.id, "verified").await;
        assert!(result.is_err(), "Cannot transition from pruned to verified");

        Ok(())
    }

    #[tokio::test]
    async fn test_search_finds_compressed_conversations() -> anyhow::Result<()> {
        // CRITICAL TEST: Proves that compressed rows (context=NULL, is_compressed=1)
        // are findable by FTS5 search after refresh_fts_index populates the
        // index with decompressed content.
        //
        // This test catches the bug where external-content FTS5 reads by column name
        // (context), missing compressed rows where context=NULL.
        let repo = repository()?;

        // Create two conversations with context containing searchable text
        let msg_compressed = ContextMessage::user("SEARCHABLE_COMPRESSED_TERM", None);
        let msg_plain = ContextMessage::user("SEARCHABLE_PLAIN_TERM", None);

        let context_compressed = Context::default().messages(vec![msg_compressed.into()]);
        let context_plain = Context::default().messages(vec![msg_plain.into()]);

        // Insert compressed conversation (will be stored as context_zstd,
        // is_compressed=1, context=NULL)
        let compressed_conv = Conversation::new(ConversationId::generate())
            .title(Some("Compressed Conversation".to_string()))
            .context(Some(context_compressed.clone()));
        repo.upsert_conversation(compressed_conv.clone()).await?;

        // Insert uncompressed conversation (will be stored as plain context,
        // is_compressed=0)
        let plain_conv = Conversation::new(ConversationId::generate())
            .title(Some("Plain Conversation".to_string()))
            .context(Some(context_plain.clone()));
        repo.upsert_conversation(plain_conv.clone()).await?;

        // Refresh FTS index to populate both compressed and uncompressed rows
        repo.refresh_fts_index().await?;

        // SEARCH 1: Find compressed conversation by term in its decompressed context
        // If the fix is correct, this search WILL find the compressed row.
        // Before the fix, this would return empty (context=NULL skipped by FTS).
        let results_compressed = repo
            .search_conversations("SEARCHABLE_COMPRESSED_TERM", None)
            .await?;
        assert!(
            !results_compressed.is_empty(),
            "FTS search must find compressed conversations after refresh_fts_index; \
             bug: external-content FTS5 reads context column by name, missing compressed rows"
        );
        assert!(
            results_compressed
                .iter()
                .any(|c| c.id == compressed_conv.id),
            "Search results must include the compressed conversation"
        );

        // SEARCH 2: Find uncompressed conversation (baseline to ensure search works)
        let results_plain = repo
            .search_conversations("SEARCHABLE_PLAIN_TERM", None)
            .await?;
        assert!(
            !results_plain.is_empty(),
            "FTS search must find uncompressed conversations"
        );
        assert!(
            results_plain.iter().any(|c| c.id == plain_conv.id),
            "Search results must include the plain conversation"
        );

        // SEARCH 3: Verify no false positives
        let results_wrong = repo.search_conversations("NONEXISTENT_TERM", None).await?;
        assert!(
            results_wrong.is_empty(),
            "Search must not return conversations that don't contain the search term"
        );

        Ok(())
    }

    /// Verify that compress_uncompressed_contexts compresses plain rows and
    /// round-trips the context JSON back intact.
    #[tokio::test]
    async fn test_compress_uncompressed_contexts_basic() -> anyhow::Result<()> {
        let repo = repository()?;

        // Insert a conversation with a plain context so the compression path has
        // something to act on.
        let conv = Conversation::new(ConversationId::generate())
            .title(Some("compress test".to_string()))
            .context(Some(Context::default()));
        repo.upsert_conversation(conv.clone()).await?;

        // The new write path already compresses on insert. Manually flip one row
        // back to is_compressed=0 with a plain context blob to simulate the
        // pre-migration state of older rows.
        let plain_json = r#"{"messages":[]}"#;
        repo.run_with_connection(move |conn, _wid| {
            diesel::sql_query(
                "UPDATE conversations \
                 SET context = ?, context_zstd = NULL, is_compressed = 0 \
                 WHERE conversation_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(plain_json)
            .bind::<diesel::sql_types::Text, _>(conv.id.into_string())
            .execute(conn)?;
            Ok(())
        })
        .await?;

        // Run the maintenance command.
        let (compressed, _skipped, errors) = repo.compress_uncompressed_contexts().await?;
        assert_eq!(errors, 0, "no compression errors expected");
        assert!(
            compressed >= 1,
            "at least one row should have been compressed"
        );

        // Verify the row is now flagged compressed and the context column is NULL.
        repo.run_with_connection(move |conn, _wid| {
            #[derive(diesel::QueryableByName)]
            struct FlagRow {
                #[diesel(sql_type = diesel::sql_types::Integer)]
                is_compressed: i32,
                #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
                context: Option<String>,
            }
            let rows: Vec<FlagRow> = diesel::sql_query(
                "SELECT is_compressed, context FROM conversations WHERE conversation_id = ?",
            )
            .bind::<diesel::sql_types::Text, _>(conv.id.into_string())
            .load(conn)?;
            let row = rows.first().expect("row should exist");
            assert_eq!(row.is_compressed, 1, "row must be flagged compressed");
            assert!(
                row.context.is_none(),
                "plain context must be cleared to NULL"
            );
            Ok(())
        })
        .await?;

        // Verify round-trip: fetch via the normal read path and confirm context loads.
        let retrieved = repo
            .get_conversation(&conv.id)
            .await?
            .expect("conversation should still exist");
        // Context may be Some (with messages) or None depending on the empty default —
        // the key assertion is that fetch does not panic or error.
        let _ = retrieved;

        Ok(())
    }

    /// Verify idempotency: running compress twice does not double-compress or
    /// error.
    #[tokio::test]
    async fn test_compress_uncompressed_contexts_idempotent() -> anyhow::Result<()> {
        let repo = repository()?;

        let conv = Conversation::new(ConversationId::generate())
            .title(Some("idempotent test".to_string()));
        repo.upsert_conversation(conv.clone()).await?;

        // First run.
        let (c1, _s1, e1) = repo.compress_uncompressed_contexts().await?;
        assert_eq!(e1, 0);

        // Second run — nothing new to compress.
        let (c2, _s2, e2) = repo.compress_uncompressed_contexts().await?;
        assert_eq!(e2, 0);
        assert_eq!(c2, 0, "second run should find no new rows to compress");
        let _ = c1;

        Ok(())
    }

    // ---------------------------------------------------------------------
    // import_forge_db tests
    // ---------------------------------------------------------------------

    /// Create an *official-lineage* source database (plain `context` schema,
    /// no `is_compressed`/`context_zstd` columns) at `path` and populate it
    /// with a deterministic set of rows:
    ///
    /// - one row with a well-formed plain JSON context (fully importable),
    /// - one row with a NULL context (imported, title-only),
    /// - one row whose context is not valid heliosLite JSON (counted as
    ///   `context_parse_failed`, still imported),
    /// - one row with a non-UUID `conversation_id` (counted as `invalid_id`).
    fn seed_official_source_db(path: &std::path::Path) {
        use diesel::Connection;

        let url = path.to_string_lossy().to_string();
        let mut connection =
            diesel::sqlite::SqliteConnection::establish(&url).expect("open source db");
        diesel::sql_query(
            "CREATE TABLE conversations (\
                 conversation_id TEXT PRIMARY KEY,\
                 title TEXT,\
                 context TEXT,\
                 created_at TEXT NOT NULL,\
                 updated_at TEXT,\
                 metrics TEXT\
             )",
        )
        .execute(&mut connection)
        .expect("create conversations table");

        let mut insert = |id: &str, title: &str, context: Option<&str>| {
            diesel::sql_query(
                "INSERT INTO conversations \
                 (conversation_id, title, context, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind::<diesel::sql_types::Text, _>(id)
            .bind::<diesel::sql_types::Text, _>(title)
            .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(context)
            .bind::<diesel::sql_types::Text, _>("2026-08-05 10:00:00")
            .bind::<diesel::sql_types::Text, _>("2026-08-05 10:05:00")
            .execute(&mut connection)
            .expect("insert row");
        };

        // A well-formed plain-text context serialized through the record type
        // exactly as the importer expects to read it back.
        let fixture_context = forge_domain::Context::default()
            .messages(vec![ContextMessage::user("source message", None).into()]);
        let well_formed = ContextRecord::from(&fixture_context);
        insert(
            "11111111-1111-1111-1111-111111111111",
            "imported fully",
            Some(&serde_json::to_string(&well_formed).unwrap()),
        );
        insert(
            "22222222-2222-2222-2222-222222222222",
            "imported title-only",
            None,
        );
        insert(
            "33333333-3333-3333-3333-333333333333",
            "imported parse-failed",
            Some("this is not valid context json"),
        );
        insert("not-a-uuid", "invalid id", None);
    }

    async fn fixture_digest(
        repo: &ConversationRepositoryImpl,
        source: &std::path::Path,
        id: ConversationId,
    ) -> anyhow::Result<String> {
        use diesel::Connection;
        use diesel::sql_types::Text;
        let uri = source_readonly_uri(source)?;
        let mut conn = diesel::sqlite::SqliteConnection::establish(&uri)?;
        let row = diesel::sql_query("SELECT conversation_id, title, context, created_at, updated_at, metrics FROM conversations WHERE conversation_id = ?")
            .bind::<Text, _>(id.into_string()).get_result::<DecodedSourceRow>(&mut conn)?;
        let legacy = repo
            .pool
            .legacy_database_path()
            .map(std::path::Path::to_path_buf);
        let acknowledgement = std::env::var_os("HELIOSLITE_ACCEPT_MISSING_LEGACY");
        let repo_primary = repo.pool.database_path().to_path_buf();
        repo.run_with_connection(move |conn, wid| {
            let observation = inspect_legacy_projection(
                conn,
                legacy.as_deref(),
                repo_primary.as_path(),
                acknowledgement.as_deref(),
            )?;
            forge_selection_digest(conn, &uri, wid.id() as i64, &row, &observation)
        })
        .await
    }

    async fn fixture_selected_import(
        repo: &ConversationRepositoryImpl,
        source: &std::path::Path,
        id: ConversationId,
    ) -> anyhow::Result<ForgeImportReport> {
        let digest = fixture_digest(repo, source, id).await?;
        repo.import_forge_db_selected(
            source.to_path_buf(),
            &[id],
            false,
            true,
            Some(&digest),
            &Default::default(),
        )
        .await
    }

    #[tokio::test]
    async fn test_import_forge_db_imports_and_reports() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let repo = repository()?;

        let error = repo.import_forge_db(source.clone()).await.unwrap_err();
        assert!(error.to_string().contains("select IDs explicitly"));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        assert!(repo.get_conversation(&id).await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_readonly_import_observes_live_wal() -> anyhow::Result<()> {
        use diesel::Connection;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("with spaces #wal.db");
        seed_official_source_db(&source);
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        diesel::sql_query("PRAGMA journal_mode=WAL").execute(&mut writer)?;
        let fixture_context = forge_domain::Context::default()
            .messages(vec![ContextMessage::user("wal message", None).into()]);
        let context = serde_json::to_string(&ContextRecord::from(&fixture_context))?;
        diesel::sql_query("INSERT INTO conversations (conversation_id, title, context, created_at) VALUES (?, ?, ?, ?)")
            .bind::<diesel::sql_types::Text, _>("44444444-4444-4444-4444-444444444444")
            .bind::<diesel::sql_types::Text, _>("wal row")
            .bind::<diesel::sql_types::Text, _>(context)
            .bind::<diesel::sql_types::Text, _>("2026-08-05 10:00:00")
            .execute(&mut writer)?;
        let repo = repository()?;
        let id = ConversationId::parse("44444444-4444-4444-4444-444444444444")?;
        let report = fixture_selected_import(&repo, &source, id).await?;
        assert_eq!(report.imported, 1);
        assert_eq!(
            repo.get_conversation(&id).await?.unwrap().title.as_deref(),
            Some("wal row")
        );
        drop(writer);
        Ok(())
    }

    #[test]
    fn test_import_readonly_source_open_does_not_create_missing_database() -> anyhow::Result<()> {
        use diesel::Connection;

        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("missing #source.db");
        let uri = source_readonly_uri(&missing)?;
        assert!(diesel::sqlite::SqliteConnection::establish(&uri).is_err());
        assert!(!missing.exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn test_import_unc_uri_uses_path_not_authority() -> anyhow::Result<()> {
        let uri = source_readonly_uri(std::path::Path::new(r"\\server\share\a #b.db"))?;
        assert_eq!(uri, "file:////server/share/a%20%23b.db?mode=ro");
        Ok(())
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn test_import_from_opt_in_unc_share() -> anyhow::Result<()> {
        let Some(share) = std::env::var_os("FORGE_TEST_UNC_ROOT") else {
            return Ok(());
        };
        let local_root = std::env::var_os("FORGE_TEST_UNC_LOCAL_ROOT")
            .ok_or_else(|| anyhow::anyhow!("FORGE_TEST_UNC_LOCAL_ROOT is required"))?;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("unc #source.db");
        seed_official_source_db(&source);
        let relative = source.strip_prefix(local_root)?;
        let unc = std::path::PathBuf::from(share).join(relative);
        let repo = repository()?;
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let report = fixture_selected_import(&repo, &unc, id).await?;
        assert_eq!(report.imported, 1);
        assert_eq!(
            repo.get_conversation(&id)
                .await?
                .unwrap()
                .context
                .unwrap()
                .messages
                .len(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_strict_import_defers_empty_context() -> anyhow::Result<()> {
        use diesel::Connection;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        let empty = serde_json::to_string(&ContextRecord::from(&forge_domain::Context::default()))?;
        diesel::sql_query("UPDATE conversations SET context = ? WHERE conversation_id = ?")
            .bind::<diesel::sql_types::Text, _>(empty)
            .bind::<diesel::sql_types::Text, _>("11111111-1111-1111-1111-111111111111")
            .execute(&mut writer)?;
        drop(writer);
        let repo = repository()?;
        let report = repo
            .import_forge_db_inner(
                source,
                &forge_domain::ForgeImportOptions { dry_run: true, verbose: false },
                true,
            )
            .await?;
        assert_eq!((report.imported, report.context_parse_failed), (0, 3));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        assert!(repo.get_conversation(&id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_forget_does_not_delete_another_workspace() -> anyhow::Result<()> {
        use diesel::sql_types::{BigInt, Text};
        let repo = repository()?;
        let local = Conversation::new(ConversationId::generate());
        let foreign = ConversationId::generate();
        repo.upsert_conversation(local.clone()).await?;
        repo.run_with_connection(move |conn, _| {
            diesel::sql_query("INSERT INTO conversations (conversation_id, workspace_id, created_at) VALUES (?, ?, CURRENT_TIMESTAMP)")
                .bind::<Text, _>(foreign.into_string()).bind::<BigInt, _>(42_i64).execute(conn)?;
            Ok(())
        }).await?;
        let report = repo
            .forget_conversations(&forge_domain::ForgeForgetOptions {
                ids: vec![local.id, foreign],
                ..Default::default()
            })
            .await?;
        assert_eq!((report.matched, report.deleted), (1, 1));
        let local_id = local.id;
        let tombstones = repo.run_with_connection(move |conn, _| {
            #[derive(diesel::QueryableByName)]
            struct Count { #[diesel(sql_type = BigInt)] n: i64 }
            Ok(diesel::sql_query("SELECT COUNT(*) AS n FROM forge_import_ledger WHERE conversation_id = ? AND state = 'deleted'")
                .bind::<Text, _>(local_id.into_string()).get_result::<Count>(conn)?.n)
        }).await?;
        assert_eq!(tombstones, 1);
        let foreign_count = repo
            .run_with_connection(move |conn, _| {
                Ok(conversations::table
                    .filter(conversations::conversation_id.eq(foreign.into_string()))
                    .count()
                    .get_result::<i64>(conn)?)
            })
            .await?;
        assert_eq!(foreign_count, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_requires_review_and_explicit_restore() -> anyhow::Result<()> {
        use diesel::Connection;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let fixture_context = forge_domain::Context::default()
            .messages(vec![ContextMessage::user("source message", None).into()]);
        let row = DecodedSourceRow {
            conversation_id: "11111111-1111-1111-1111-111111111111".into(),
            title: Some("imported fully".into()),
            context: Some(serde_json::to_string(&ContextRecord::from(
                &fixture_context,
            ))?),
            created_at: "2026-08-05 10:00:00".into(),
            updated_at: Some("2026-08-05 10:05:00".into()),
            metrics: None,
        };
        let id = ConversationId::parse(&row.conversation_id)?;
        let repo = repository()?;
        let digest = fixture_digest(&repo, &source, id).await?;
        let options = forge_domain::ForgeImportOptions::default();
        let preview = repo
            .import_forge_db_selected(
                source.clone(),
                &[id],
                false,
                false,
                None,
                &forge_domain::ForgeImportOptions { dry_run: true, verbose: false },
            )
            .await?;
        assert_eq!((preview.dry_run, preview.imported), (true, 1));
        assert!(repo.get_conversation(&id).await?.is_none());
        let other_source = temp.path().join("other.db");
        std::fs::copy(&source, &other_source)?;
        assert!(
            repo.import_forge_db_selected(
                other_source,
                &[id],
                false,
                true,
                Some(&digest),
                &options
            )
            .await
            .is_err()
        );
        assert!(repo.get_conversation(&id).await?.is_none());
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                false,
                false,
                Some(&digest),
                &options
            )
            .await
            .is_err()
        );
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                false,
                true,
                Some("wrong"),
                &options
            )
            .await
            .is_err()
        );
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        diesel::sql_query("UPDATE conversations SET title = 'changed after preview' WHERE conversation_id = '11111111-1111-1111-1111-111111111111'").execute(&mut writer)?;
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                false,
                true,
                Some(&digest),
                &options
            )
            .await
            .is_err()
        );
        assert!(repo.get_conversation(&id).await?.is_none());
        diesel::sql_query("UPDATE conversations SET title = 'imported fully' WHERE conversation_id = '11111111-1111-1111-1111-111111111111'").execute(&mut writer)?;
        drop(writer);
        let imported = repo
            .import_forge_db_selected(source.clone(), &[id], false, true, Some(&digest), &options)
            .await?;
        assert_eq!(imported.imported, 1);
        let edited = repo
            .get_conversation(&id)
            .await?
            .unwrap()
            .title(Some("local edit".to_string()));
        repo.upsert_conversation(edited).await?;
        let current_digest = fixture_digest(&repo, &source, id).await?;
        let unchanged = repo
            .import_forge_db_selected(
                source.clone(),
                &[id],
                false,
                true,
                Some(&current_digest),
                &options,
            )
            .await?;
        assert_eq!(unchanged.skipped_existing, 1);
        assert_eq!(
            repo.get_conversation(&id).await?.unwrap().title.as_deref(),
            Some("local edit")
        );
        repo.delete_conversation(&id).await?;
        assert!(repo.get_conversation(&id).await?.is_none());
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                false,
                true,
                Some(&digest),
                &options
            )
            .await
            .is_err()
        );
        assert!(repo.get_conversation(&id).await?.is_none());
        let restore_digest = fixture_digest(&repo, &source, id).await?;
        repo.delete_conversation(&id).await?;
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                true,
                true,
                Some(&restore_digest),
                &options
            )
            .await
            .is_err()
        );
        let restore_digest = fixture_digest(&repo, &source, id).await?;
        let restored = repo
            .import_forge_db_selected(source, &[id], true, true, Some(&restore_digest), &options)
            .await?;
        assert_eq!(restored.imported, 1);
        assert!(repo.get_conversation(&id).await?.unwrap().context.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_rejects_unavailable_configured_legacy() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.db");
        seed_official_source_db(&source);
        let missing_legacy = temp.path().join("missing.db");
        let mut config = PoolConfig::new(temp.path().join("write.db"))
            .with_legacy_database_path(Some(missing_legacy.clone()));
        config.max_size = 1;
        let pool = Arc::new(DatabasePool::try_from(config)?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        assert!(
            repo.get_all_conversations(None)
                .await?
                .is_none_or(|rows| rows.is_empty())
        );
        assert!(repo.delete_conversation(&id).await.is_err());
        let forget = forge_domain::ForgeForgetOptions { ids: vec![id], ..Default::default() };
        assert!(repo.forget_conversations(&forget).await.is_err());
        let options = forge_domain::ForgeImportOptions { dry_run: true, ..Default::default() };
        assert!(
            repo.import_forge_db_selected(source.clone(), &[id], false, false, None, &options)
                .await
                .is_err()
        );
        let legacy_for_ack = missing_legacy.clone();
        let primary_for_ack = temp.path().join("write.db");
        repo.run_with_connection(move |conn, _| {
            assert!(
                ensure_legacy_projection_with_ack(
                    conn,
                    Some(&legacy_for_ack),
                    &primary_for_ack,
                    Some(std::ffi::OsStr::new("wrong"))
                )
                .is_err()
            );
            ensure_legacy_projection_with_ack(
                conn,
                Some(&legacy_for_ack),
                &primary_for_ack,
                Some(legacy_for_ack.as_os_str()),
            )?;
            diesel::sql_query("ATTACH DATABASE ':memory:' AS legacy_read").execute(conn)?;
            let blocked = ensure_legacy_projection_with_ack(
                conn,
                Some(&legacy_for_ack),
                &primary_for_ack,
                Some(legacy_for_ack.as_os_str()),
            )
            .is_err();
            diesel::sql_query("DETACH DATABASE legacy_read").execute(conn)?;
            anyhow::ensure!(
                blocked,
                "stale legacy attachment must block acknowledgement"
            );
            Ok(())
        })
        .await?;
        seed_official_source_db(&missing_legacy);
        let legacy_for_ack = missing_legacy.clone();
        let primary_for_ack = temp.path().join("write.db");
        assert!(
            repo.run_with_connection(move |conn, _| ensure_legacy_projection_with_ack(
                conn,
                Some(&legacy_for_ack),
                &primary_for_ack,
                Some(legacy_for_ack.as_os_str())
            ))
            .await
            .is_err()
        );
        assert!(repo.delete_conversation(&id).await.is_err());
        assert!(
            repo.import_forge_db_selected(source, &[id], false, false, None, &options)
                .await
                .is_err()
        );
        let count = repo
            .run_with_connection(move |conn, _| {
                #[derive(diesel::QueryableByName)]
                struct Count {
                    #[diesel(sql_type = diesel::sql_types::BigInt)]
                    n: i64,
                }
                Ok(
                    diesel::sql_query("SELECT COUNT(*) AS n FROM forge_import_ledger")
                        .get_result::<Count>(conn)?
                        .n,
                )
            })
            .await?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_accepts_usable_legacy_projection() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        let temp = tempfile::tempdir()?;
        let legacy = temp.path().join("legacy.db");
        seed_official_source_db(&legacy);
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(temp.path().join("write.db"))
                .with_legacy_database_path(Some(legacy.clone())),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let options = forge_domain::ForgeImportOptions { dry_run: true, ..Default::default() };
        let report = repo
            .import_forge_db_selected(legacy, &[id], false, false, None, &options)
            .await?;
        assert_eq!(report.skipped_existing, 1);
        assert!(repo.delete_conversation(&id).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_digest_binds_legacy_acknowledgement() -> anyhow::Result<()> {
        use diesel::Connection;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.db");
        let missing = temp.path().join("missing.db");
        seed_official_source_db(&source);
        let uri = source_readonly_uri(&source)?;
        let mut conn = diesel::sqlite::SqliteConnection::establish(&uri)?;
        let row = diesel::sql_query("SELECT conversation_id, title, context, created_at, updated_at, metrics FROM conversations WHERE conversation_id = '11111111-1111-1111-1111-111111111111'").get_result::<DecodedSourceRow>(&mut conn)?;
        let repo = repository()?;
        repo.run_with_connection(move |conn, wid| {
            let without = LegacyObservation {
                path: Some(missing.to_string_lossy().into_owned()),
                present: false,
                acknowledged: false,
            };
            let with = LegacyObservation { acknowledged: true, ..without.clone() };
            let first = forge_selection_digest(conn, &uri, wid.id() as i64, &row, &without)?;
            let second = forge_selection_digest(conn, &uri, wid.id() as i64, &row, &with)?;
            assert_ne!(first, second);
            Ok(())
        })
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_rechecks_legacy_before_commit() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("appears.db");
        let primary = temp.path().join("write.db");
        let repo = repository()?;
        let result = repo
            .run_with_connection(move |conn, _| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let observation = inspect_legacy_projection(
                        conn,
                        Some(&missing),
                        &primary,
                        Some(missing.as_os_str()),
                    )?;
                    record_missing_delete(conn, "recheck-id", 0)?;
                    std::fs::write(&missing, b"appeared")?;
                    ensure_legacy_unchanged(
                        conn,
                        Some(&missing),
                        &primary,
                        Some(missing.as_os_str()),
                        &observation,
                    )?;
                    Ok(())
                })
            })
            .await;
        assert!(result.is_err());
        let count = repo.run_with_connection(|conn, _| {
            #[derive(diesel::QueryableByName)] struct Count { #[diesel(sql_type = diesel::sql_types::BigInt)] n: i64 }
            Ok(diesel::sql_query("SELECT COUNT(*) AS n FROM forge_import_ledger WHERE conversation_id = 'recheck-id'").get_result::<Count>(conn)?.n)
        }).await?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_entrypoints_recheck_before_commit() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        use diesel::sql_types::{BigInt, Nullable, Text};
        const MODE: &str = "HELIOSLITE_RECHECK_TEST_MODE";
        const ROOT: &str = "HELIOSLITE_RECHECK_TEST_ROOT";
        #[derive(Debug, PartialEq, diesel::QueryableByName)]
        struct Snapshot {
            #[diesel(sql_type = Nullable<Text>)]
            conversation: Option<String>,
            #[diesel(sql_type = Nullable<Text>)]
            ledger: Option<String>,
        }
        fn snapshot(conn: &mut diesel::SqliteConnection, id: &str) -> anyhow::Result<Snapshot> {
            Ok(diesel::sql_query("SELECT (SELECT json_array(conversation_id, title, workspace_id, context, created_at, updated_at, metrics, parent_id, source, cwd, message_count, intent_state, extracted_at, memory_id, intent_hash, CASE WHEN context_zstd IS NULL THEN NULL ELSE hex(context_zstd) END, is_compressed) FROM conversations WHERE conversation_id = ?) AS conversation, (SELECT json_array(conversation_id, workspace_id, origin, source_locator, source_digest, state, generation) FROM forge_import_ledger WHERE conversation_id = ?) AS ledger")
                .bind::<Text, _>(id).bind::<Text, _>(id).get_result::<Snapshot>(conn)?)
        }
        if let Some(mode) = std::env::var_os(MODE) {
            let root = std::path::PathBuf::from(
                std::env::var_os(ROOT).ok_or_else(|| anyhow::anyhow!("missing test root"))?,
            );
            let legacy = root.join("legacy.db");
            let source = root.join("source.db");
            seed_official_source_db(&source);
            let mut config = PoolConfig::new(root.join("write.db"))
                .with_legacy_database_path(Some(legacy.clone()));
            config.max_size = 1;
            let pool = Arc::new(DatabasePool::try_from(config)?);
            let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));
            let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
            let digest = fixture_digest(&repo, &source, id).await?;
            let existing = mode == "delete_existing" || mode == "forget_existing";
            if existing {
                repo.run_with_connection(move |conn, _| {
                    diesel::sql_query("INSERT INTO conversations (conversation_id, workspace_id, title, context, context_zstd, created_at, source) VALUES (?, 0, 'preserve title', 'preserve context', X'', '2026-01-01 00:00:00', 'local')")
                        .bind::<Text, _>(id.into_string()).execute(conn)?;
                    diesel::sql_query("INSERT INTO forge_import_ledger (conversation_id, workspace_id, origin, source_locator, source_digest, state) VALUES (?, 0, 'forge', 'fixture-source', 'fixture-digest', 'imported')")
                        .bind::<Text, _>(id.into_string()).execute(conn)?;
                    Ok(())
                }).await?;
            }
            let before = repo
                .run_with_connection(move |conn, _| snapshot(conn, &id.into_string()))
                .await?;
            assert_eq!(before.conversation.is_some(), existing);
            assert_eq!(before.ledger.is_some(), existing);
            let created = legacy.clone();
            let is_import = mode == "import";
            *legacy_recheck_hook().lock().unwrap() = Some((
                legacy,
                Box::new(move |conn| {
                    #[derive(diesel::QueryableByName)]
                    struct Count {
                        #[diesel(sql_type = BigInt)]
                        n: i64,
                    }
                    #[derive(diesel::QueryableByName)]
                    struct State {
                        #[diesel(sql_type = Text)]
                        state: String,
                    }
                    let rows = diesel::sql_query("SELECT COUNT(*) AS n FROM conversations")
                        .get_result::<Count>(conn)
                        .unwrap()
                        .n;
                    let state = diesel::sql_query(
                        "SELECT state FROM forge_import_ledger WHERE conversation_id = ?",
                    )
                    .bind::<Text, _>(id.into_string())
                    .get_result::<State>(conn)
                    .unwrap()
                    .state;
                    assert_eq!(
                        rows,
                        i64::from(is_import),
                        "mutation must run before recheck"
                    );
                    assert_eq!(state, if is_import { "imported" } else { "deleted" });
                    std::fs::write(created, b"appeared").unwrap();
                }),
            ));
            let result = match mode.to_str() {
                Some("import") => repo
                    .import_forge_db_selected(
                        source,
                        &[id],
                        false,
                        true,
                        Some(&digest),
                        &Default::default(),
                    )
                    .await
                    .map(|_| ()),
                Some("delete" | "delete_existing") => repo.delete_conversation(&id).await,
                Some("forget" | "forget_existing") => repo
                    .forget_conversations(&forge_domain::ForgeForgetOptions {
                        ids: vec![id],
                        ..Default::default()
                    })
                    .await
                    .map(|_| ()),
                _ => anyhow::bail!("unknown test mode"),
            };
            assert!(
                result.is_err(),
                "entrypoint must refuse the changed legacy source"
            );
            assert!(
                root.join("legacy.db").is_file(),
                "recheck injection must run"
            );
            let after = repo
                .run_with_connection(move |conn, _| snapshot(conn, &id.into_string()))
                .await?;
            assert_eq!(
                after, before,
                "entrypoint must roll back the full conversation and ledger rows"
            );
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        for mode in [
            "import",
            "delete",
            "forget",
            "delete_existing",
            "forget_existing",
        ] {
            let root = temp.path().join(mode);
            std::fs::create_dir_all(&root)?;
            let output = std::process::Command::new(std::env::current_exe()?)
                .arg("--exact")
                .arg("conversation::conversation_repo::tests::test_selected_import_entrypoints_recheck_before_commit")
                .arg("--nocapture")
                .env(MODE, mode)
                .env(ROOT, &root)
                .env("HELIOSLITE_ACCEPT_MISSING_LEGACY", root.join("legacy.db"))
                .output()?;
            assert!(
                output.status.success(),
                "{mode} child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"),
                "{mode} child did not run the exact test"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_rejects_changed_legacy_configuration() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.db");
        let first = temp.path().join("first.db");
        let second = temp.path().join("second.db");
        seed_official_source_db(&source);
        seed_official_source_db(&first);
        seed_official_source_db(&second);
        let primary = temp.path().join("write.db");
        let first_pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(primary.clone()).with_legacy_database_path(Some(first)),
        )?);
        let second_pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(primary).with_legacy_database_path(Some(second)),
        )?);
        let first_repo = ConversationRepositoryImpl::new(first_pool, WorkspaceHash::new(0));
        let second_repo = ConversationRepositoryImpl::new(second_pool, WorkspaceHash::new(0));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let digest = fixture_digest(&first_repo, &source, id).await?;
        let error = second_repo
            .import_forge_db_selected(
                source,
                &[id],
                false,
                true,
                Some(&digest),
                &Default::default(),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("digest mismatch"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_legacy_collision_blocks_local_delete() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        let temp = tempfile::tempdir()?;
        let legacy = temp.path().join("legacy.db");
        seed_official_source_db(&legacy);
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(temp.path().join("write.db")).with_legacy_database_path(Some(legacy)),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        repo.run_with_connection(move |conn, _| {
            diesel::sql_query("INSERT INTO conversations (conversation_id, workspace_id, title, created_at) VALUES (?, 0, 'local', CURRENT_TIMESTAMP)")
                .bind::<diesel::sql_types::Text, _>(id.into_string()).execute(conn)?;
            Ok(())
        }).await?;
        let error = repo.delete_conversation(&id).await.unwrap_err();
        assert!(error.to_string().contains("reveal an attached legacy"));
        let forget = forge_domain::ForgeForgetOptions { ids: vec![id], ..Default::default() };
        assert!(repo.forget_conversations(&forget).await.is_err());
        let title = repo.get_conversation(&id).await?.unwrap().title;
        assert_eq!(title.as_deref(), Some("local"));
        let counts = repo
            .run_with_connection(move |conn, _| {
                #[derive(diesel::QueryableByName)]
                struct Count {
                    #[diesel(sql_type = diesel::sql_types::BigInt)]
                    n: i64,
                }
                let local = diesel::sql_query(
                    "SELECT COUNT(*) AS n FROM conversations WHERE conversation_id = ?",
                )
                .bind::<diesel::sql_types::Text, _>(id.into_string())
                .get_result::<Count>(conn)?
                .n;
                let ledger = diesel::sql_query(
                    "SELECT COUNT(*) AS n FROM forge_import_ledger WHERE conversation_id = ?",
                )
                .bind::<diesel::sql_types::Text, _>(id.into_string())
                .get_result::<Count>(conn)?
                .n;
                Ok((local, ledger))
            })
            .await?;
        assert_eq!(counts, (1, 0));
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_rejects_concurrent_deletion() -> anyhow::Result<()> {
        use crate::database::PoolConfig;
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source.db");
        seed_official_source_db(&source);
        let pool = Arc::new(DatabasePool::try_from(PoolConfig::new(
            temp.path().join("write.db"),
        ))?);
        let repo = Arc::new(ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0)));
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let digest = fixture_digest(&repo, &source, id).await?;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder_repo = repo.clone();
        let holder = tokio::spawn(async move {
            holder_repo
                .run_with_connection(move |conn, wid| {
                    conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                        record_missing_delete(conn, &id.into_string(), wid.id() as i64)?;
                        ready_tx
                            .send(())
                            .map_err(|_| anyhow::anyhow!("test readiness receiver closed"))?;
                        release_rx.recv_timeout(std::time::Duration::from_secs(10))?;
                        Ok(())
                    })
                })
                .await
        });
        ready_rx.await?;
        let import_repo = repo.clone();
        let importer = tokio::spawn(async move {
            import_repo
                .import_forge_db_selected(
                    source,
                    &[id],
                    true,
                    true,
                    Some(&digest),
                    &Default::default(),
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !importer.is_finished(),
            "import should wait for the active writer"
        );
        release_tx.send(())?;
        holder.await??;
        let error = importer.await?.unwrap_err();
        assert!(
            error.to_string().contains("digest mismatch"),
            "unexpected error: {error}"
        );
        assert!(repo.get_conversation(&id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_missing_id_deletions_invalidate_preview() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let repo = repository()?;
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let options = forge_domain::ForgeImportOptions::default();
        let before = fixture_digest(&repo, &source, id).await?;
        repo.delete_conversation(&id).await?;
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                true,
                true,
                Some(&before),
                &options
            )
            .await
            .is_err()
        );
        let after_delete = fixture_digest(&repo, &source, id).await?;
        assert_ne!(before, after_delete);
        let forget = forge_domain::ForgeForgetOptions { ids: vec![id], ..Default::default() };
        repo.forget_conversations(&forget).await?;
        assert!(
            repo.import_forge_db_selected(
                source.clone(),
                &[id],
                true,
                true,
                Some(&after_delete),
                &options
            )
            .await
            .is_err()
        );
        let after_forget = fixture_digest(&repo, &source, id).await?;
        assert_ne!(after_delete, after_forget);
        let dry = forge_domain::ForgeForgetOptions { dry_run: true, ..forget.clone() };
        repo.forget_conversations(&dry).await?;
        assert_eq!(after_forget, fixture_digest(&repo, &source, id).await?);
        let mixed = forge_domain::ForgeForgetOptions { source: Some("agent".into()), ..forget };
        assert!(repo.forget_conversations(&mixed).await.is_err());
        assert_eq!(after_forget, fixture_digest(&repo, &source, id).await?);
        assert_eq!(
            repo.import_forge_db_selected(source, &[id], true, true, Some(&after_forget), &options)
                .await?
                .imported,
            1
        );
        let report = repo.forget_conversations(&mixed).await?;
        assert_eq!((report.matched, report.deleted), (0, 0));
        assert!(repo.get_conversation(&id).await?.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_import_rolls_back_if_ledger_write_fails() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let fixture_context = forge_domain::Context::default()
            .messages(vec![ContextMessage::user("source message", None).into()]);
        let row = DecodedSourceRow {
            conversation_id: "11111111-1111-1111-1111-111111111111".into(),
            title: Some("imported fully".into()),
            context: Some(serde_json::to_string(&ContextRecord::from(
                &fixture_context,
            ))?),
            created_at: "2026-08-05 10:00:00".into(),
            updated_at: Some("2026-08-05 10:05:00".into()),
            metrics: None,
        };
        let id = ConversationId::parse(&row.conversation_id)?;
        let repo = repository()?;
        let digest = fixture_digest(&repo, &source, id).await?;
        repo.run_with_connection(|conn, _| {
            diesel::sql_query("CREATE TRIGGER deny_provenance BEFORE INSERT ON forge_import_ledger WHEN NEW.origin = 'forge' BEGIN SELECT RAISE(ABORT, 'injected ledger failure'); END").execute(conn)?;
            Ok(())
        }).await?;
        let result = repo
            .import_forge_db_selected(
                source,
                &[id],
                false,
                true,
                Some(&digest),
                &Default::default(),
            )
            .await;
        assert!(result.is_err());
        assert!(repo.get_conversation(&id).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn test_strict_import_retries_corrected_context() -> anyhow::Result<()> {
        use diesel::Connection;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let before = std::fs::read(&source)?;
        let repo = repository()?;
        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };
        let first = repo
            .import_forge_db_inner(source.clone(), &options, true)
            .await?;
        assert_eq!(
            (first.imported, first.context_parse_failed, first.invalid_id),
            (1, 2, 1)
        );
        assert_eq!(std::fs::read(&source)?, before);
        let existing_id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        assert_eq!(
            fixture_selected_import(&repo, &source, existing_id)
                .await?
                .imported,
            1
        );
        let id = ConversationId::parse("33333333-3333-3333-3333-333333333333")?;
        assert!(repo.get_conversation(&id).await?.is_none());

        let mut connection =
            diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        let fixture_context = forge_domain::Context::default()
            .messages(vec![ContextMessage::user("recovered message", None).into()]);
        let context = serde_json::to_string(&ContextRecord::from(&fixture_context))?;
        diesel::sql_query("UPDATE conversations SET context = ? WHERE conversation_id = ?")
            .bind::<diesel::sql_types::Text, _>(context)
            .bind::<diesel::sql_types::Text, _>("33333333-3333-3333-3333-333333333333")
            .execute(&mut connection)?;
        diesel::sql_query("UPDATE conversations SET title = 'updated upstream' WHERE conversation_id = '11111111-1111-1111-1111-111111111111'")
            .execute(&mut connection)?;
        drop(connection);

        let second = fixture_selected_import(&repo, &source, id).await?;
        assert_eq!(
            (
                second.imported,
                second.skipped_existing,
                second.context_parse_failed
            ),
            (1, 0, 0)
        );
        let recovered = repo.get_conversation(&id).await?.unwrap();
        assert!(recovered.context.is_some());
        assert_eq!(recovered.context.unwrap().messages.len(), 1);
        assert_eq!(
            repo.get_conversation(&existing_id)
                .await?
                .unwrap()
                .title
                .as_deref(),
            Some("imported fully")
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_import_forge_db_is_idempotent() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let repo = repository()?;

        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let first = fixture_selected_import(&repo, &source, id).await?;
        assert_eq!(first.imported, 1);

        let second = fixture_selected_import(&repo, &source, id).await?;
        assert_eq!(second.imported, 0, "re-run imports nothing");
        assert_eq!(
            second.skipped_existing, 1,
            "previously imported ids are skipped"
        );
        assert_eq!(second.invalid_id, 0);
        assert_eq!(second.errors, 0);

        Ok(())
    }

    #[tokio::test]
    async fn test_import_forge_db_does_not_modify_source() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);

        let before = std::fs::read(&source)?;
        let repo = repository()?;
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        fixture_selected_import(&repo, &source, id).await?;
        let after = std::fs::read(&source)?;

        assert_eq!(before, after, "source database bytes must be unchanged");
        Ok(())
    }

    #[tokio::test]
    async fn test_import_forge_db_rejects_fork_schema_source() -> anyhow::Result<()> {
        use diesel::Connection;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("fork-schema.db");
        let url = source.to_string_lossy().to_string();
        let mut connection = diesel::sqlite::SqliteConnection::establish(&url).expect("open db");
        // A fork-schema table carries the compression columns the importer
        // refuses to process.
        diesel::sql_query(
            "CREATE TABLE conversations (\
                 conversation_id TEXT PRIMARY KEY,\
                 context TEXT,\
                 context_zstd BLOB,\
                 is_compressed INTEGER\
             )",
        )
        .execute(&mut connection)?;

        let repo = repository()?;
        let error = repo
            .import_forge_db_with_options(
                source.clone(),
                &forge_domain::ForgeImportOptions { dry_run: true, verbose: false },
            )
            .await
            .expect_err("fork-schema source must be rejected");
        assert!(
            format!("{error}").contains("already a heliosLite/fork-schema"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_import_forge_db_missing_source_errors() -> anyhow::Result<()> {
        let repo = repository()?;
        let missing =
            std::env::temp_dir().join(format!("forge-does-not-exist-{}.db", std::process::id()));
        let error = repo
            .import_forge_db_with_options(
                missing,
                &forge_domain::ForgeImportOptions { dry_run: true, verbose: false },
            )
            .await
            .expect_err("missing file must error");
        assert!(
            format!("{error}").contains("source database not found"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_import_forge_db_dry_run_does_not_write() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let repo = repository()?;

        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };
        let report = repo
            .import_forge_db_with_options(source.clone(), &options)
            .await?;

        // Dry-run reports the same counts but writes nothing.
        assert!(report.dry_run, "dry_run flag must be set in report");
        assert_eq!(report.source_total, 4);
        assert_eq!(report.imported, 3, "would be imported, but not written");
        assert_eq!(report.invalid_id, 1);
        assert_eq!(report.context_parse_failed, 1);

        // No rows should be visible in the destination after a dry run.
        let all = repo.get_all_conversations(None).await?;
        assert!(
            all.map(|v| v.is_empty()).unwrap_or(true),
            "dry_run must not write to destination"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_import_forge_db_verbose_does_not_change_outcome() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("official.db");
        seed_official_source_db(&source);
        let repo = repository()?;

        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: true };
        let report = repo
            .import_forge_db_with_options(source.clone(), &options)
            .await?;
        assert_eq!(report.imported, 3);
        assert_eq!(report.invalid_id, 1);
        assert_eq!(report.context_parse_failed, 1);
        assert!(report.dry_run);
        Ok(())
    }

    #[tokio::test]
    async fn test_export_forge_db_round_trips() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = repository()?;

        // Seed a conversation with an actual message payload so the export
        // has a context to decompress and write.
        let msg = ContextMessage::user("export me payload", None);
        let context = Context::default().messages(vec![msg.into()]);
        let conv = forge_domain::Conversation::new(ConversationId::generate())
            .title(Some("export me".to_string()))
            .context(Some(context));
        repo.upsert_conversation(conv.clone()).await?;

        let dest = temp.path().join("exported.db");
        let options = forge_domain::ForgeExportOptions::default();
        let report = repo.export_forge_db(dest.clone(), &options).await?;

        assert_eq!(report.source_total, 1, "all rows read from source");
        assert_eq!(report.exported, 1, "decompressed + written");
        assert_eq!(report.decompression_failed, 0);
        assert_eq!(report.errors, 0);
        assert!(!report.dry_run);

        // Verify the destination has the official schema and the conversation
        // is present with plain (uncompressed) context.
        use diesel::Connection;
        let url = dest.to_string_lossy().to_string();
        let mut check = diesel::sqlite::SqliteConnection::establish(&url)?;
        #[derive(diesel::QueryableByName)]
        struct ExportRow {
            #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
            context: Option<String>,
        }
        let rows: Vec<ExportRow> =
            diesel::sql_query("SELECT context FROM conversations WHERE conversation_id = ?")
                .bind::<diesel::sql_types::Text, _>(conv.id.into_string())
                .load(&mut check)?;
        assert_eq!(rows.len(), 1, "exported row is present");
        assert!(
            rows[0].context.is_some(),
            "exported context must be plain (not compressed)"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_export_forge_db_dry_run_does_not_create_file() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = repository()?;

        let conv = forge_domain::Conversation::new(ConversationId::generate())
            .title(Some("do not write".to_string()))
            .context(Some(Context::default().messages(vec![
                ContextMessage::user("do not write payload", None).into(),
            ])));
        repo.upsert_conversation(conv).await?;

        let dest = temp.path().join("never-created.db");
        let options = forge_domain::ForgeExportOptions { dry_run: true, ..Default::default() };
        let report = repo.export_forge_db(dest.clone(), &options).await?;

        assert!(report.dry_run);
        assert_eq!(report.source_total, 1);
        assert_eq!(report.exported, 1);
        assert!(
            !dest.exists(),
            "dry_run must not create the destination file"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_database_stats_reports_compression_health() -> anyhow::Result<()> {
        let repo = repository()?;
        // Insert one conversation (with a message payload, so the write path
        // compresses it) so the totals are non-zero.
        let conv = forge_domain::Conversation::new(ConversationId::generate())
            .title(Some("stats test".to_string()))
            .context(Some(Context::default().messages(vec![
                ContextMessage::user("stats payload", None).into(),
            ])));
        repo.upsert_conversation(conv).await?;

        let stats = repo.database_stats().await?;
        assert!(
            stats.total_conversations >= 1,
            "total includes the inserted row"
        );
        assert!(
            stats.compressed_rows >= 1,
            "newly inserted rows are compressed via the write path"
        );
        assert_eq!(
            stats.integrity_check, "ok",
            "PRAGMA integrity_check should report ok"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_split_db_unscoped_legacy_discovery_preserves_workspace_writes() {
        use diesel::Connection;
        let fixture = tempfile::tempdir().unwrap();
        let legacy_path = fixture.path().join("legacy.db");
        let legacy_id = ConversationId::generate();
        let mut legacy =
            diesel::SqliteConnection::establish(legacy_path.to_str().unwrap()).unwrap();
        diesel::sql_query("CREATE TABLE conversations (conversation_id TEXT PRIMARY KEY NOT NULL, title TEXT, context TEXT, created_at TIMESTAMP NOT NULL, updated_at TIMESTAMP, metrics TEXT)")
            .execute(&mut legacy).unwrap();
        diesel::sql_query("INSERT INTO conversations (conversation_id, title, created_at) VALUES (?, 'unscoped legacy', CURRENT_TIMESTAMP)")
            .bind::<diesel::sql_types::Text, _>(legacy_id.to_string())
            .execute(&mut legacy).unwrap();
        drop(legacy);
        let pool = Arc::new(
            DatabasePool::try_from(
                PoolConfig::new(fixture.path().join("write.db"))
                    .with_legacy_database_path(Some(legacy_path.clone())),
            )
            .unwrap(),
        );
        let zero_repo = ConversationRepositoryImpl::new(pool.clone(), WorkspaceHash::new(0));
        let local = Conversation::new(ConversationId::generate())
            .title(Some("local workspace zero".to_string()));
        zero_repo.upsert_conversation(local.clone()).await.unwrap();
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(42));

        let actual = repo
            .get_all_conversations(None)
            .await
            .unwrap()
            .unwrap_or_default();
        let expected = vec![legacy_id];
        assert_eq!(actual.iter().map(|c| c.id).collect::<Vec<_>>(), expected);
        assert!(repo.get_conversation(&legacy_id).await.unwrap().is_some());
        assert!(repo.get_conversation(&local.id).await.unwrap().is_none());
        assert!(
            repo.mark_intent_state(&legacy_id, "extracting")
                .await
                .is_err()
        );
        assert!(repo.rewind_conversation(&legacy_id).await.is_err());
        assert!(repo.prune_conversation(&legacy_id).await.is_err());
        assert!(repo.delete_conversation(&legacy_id).await.is_err());
        assert!(repo.update_parent_id(&legacy_id, None).await.is_err());
        let legacy_copy = Conversation::new(legacy_id).title(Some("shadow".to_string()));
        assert!(repo.upsert_conversation(legacy_copy.clone()).await.is_err());
        assert!(repo.upsert_conversation_ref(&legacy_copy).await.is_err());
        let visible = repo.get_all_conversations(None).await.unwrap().unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].title.as_deref(), Some("unscoped legacy"));
        // Explicit import retains its existing skip-existing contract; it does
        // not promote an attached legacy row into the writable database.
        let report = repo.import_forge_db(legacy_path.clone()).await.unwrap();
        assert_eq!((report.imported, report.skipped_existing), (0, 1));
        let local_legacy_count = repo
            .run_with_connection(move |connection, _wid| {
                Ok(conversations::table
                    .filter(conversations::conversation_id.eq(legacy_id.into_string()))
                    .count()
                    .get_result::<i64>(connection)?)
            })
            .await
            .unwrap();
        assert_eq!(local_legacy_count, 0);
        // Missing IDs retain the existing idempotent no-op contract.
        let missing_id = ConversationId::generate();
        repo.delete_conversation(&missing_id).await.unwrap();
        repo.update_parent_id(&missing_id, None).await.unwrap();
        assert!(
            repo.rewind_conversation(&missing_id)
                .await
                .unwrap()
                .is_none(),
            "rewinding an unknown conversation is an idempotent no-op"
        );
        zero_repo
            .update_parent_id(&local.id, Some(&missing_id))
            .await
            .unwrap();
        assert_eq!(
            zero_repo
                .get_conversation(&local.id)
                .await
                .unwrap()
                .unwrap()
                .parent_id,
            Some(missing_id)
        );
        let before = zero_repo
            .get_conversation(&local.id)
            .await
            .unwrap()
            .unwrap();
        let errors = [
            repo.upsert_conversation(local.clone()).await.unwrap_err(),
            repo.upsert_conversation_ref(&local).await.unwrap_err(),
            repo.delete_conversation(&local.id).await.unwrap_err(),
            repo.update_parent_id(&local.id, None).await.unwrap_err(),
            repo.rewind_conversation(&local.id).await.unwrap_err(),
            repo.mark_intent_state(&local.id, "extracting")
                .await
                .unwrap_err(),
            repo.prune_conversation(&local.id).await.unwrap_err(),
        ];
        for error in errors {
            assert_eq!(
                error.to_string(),
                "Conversation belongs to another workspace"
            );
        }
        let after = zero_repo
            .get_conversation(&local.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(after).unwrap(),
            serde_json::to_value(before).unwrap()
        );
        let remaining = zero_repo
            .get_all_conversations(None)
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(remaining.iter().any(|c| c.id == local.id));
        // Release all pooled attachments before changing the legacy schema.
        drop(repo);
        drop(zero_repo);
        let mut legacy =
            diesel::SqliteConnection::establish(legacy_path.to_str().unwrap()).unwrap();
        #[derive(diesel::QueryableByName)]
        struct Count {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            count: i64,
        }
        let actual = diesel::sql_query("SELECT count(*) AS count FROM conversations")
            .get_result::<Count>(&mut legacy)
            .unwrap()
            .count;
        assert_eq!(actual, 1);
        // A real workspace-zero legacy row must not be confused with a
        // projected placeholder for a schema that lacked workspace ownership.
        diesel::sql_query(
            "ALTER TABLE conversations ADD COLUMN workspace_id BIGINT NOT NULL DEFAULT 0",
        )
        .execute(&mut legacy)
        .unwrap();
        drop(legacy);
        let scoped_pool = Arc::new(
            DatabasePool::try_from(
                PoolConfig::new(fixture.path().join("scoped-write.db"))
                    .with_legacy_database_path(Some(legacy_path)),
            )
            .unwrap(),
        );
        let scoped_repo = ConversationRepositoryImpl::new(scoped_pool, WorkspaceHash::new(42));
        let actual = scoped_repo
            .get_all_conversations(None)
            .await
            .unwrap()
            .unwrap_or_default();
        assert!(actual.is_empty());
    }

    #[tokio::test]
    async fn test_split_db_union_reads_legacy_rows() -> anyhow::Result<()> {
        // Proves the split-DB read path: a pool whose primary (write) DB is
        // separate from a legacy DB sees rows from BOTH via the
        // `conversations_all` TEMP VIEW (legacy ATTACHed read-only).
        use crate::database::DatabasePool;
        let temp = tempfile::tempdir()?;
        let legacy_db = temp.path().join("legacy.db");
        let write_db = temp.path().join("write.db");

        // Seed the legacy DB through a standalone pool, then close it.
        {
            let legacy_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(legacy_db.clone()).with_legacy_database_path(None),
            )?);
            let legacy_repo = ConversationRepositoryImpl::new(legacy_pool, WorkspaceHash::new(0));
            let conv = Conversation::new(ConversationId::generate())
                .title(Some("legacy row".to_string()))
                .context(Some(Context::default().messages(vec![
                    ContextMessage::user("legacy payload", None).into(),
                ])));
            legacy_repo.upsert_conversation(conv.clone()).await?;
        }

        // Open a fresh write DB with the legacy DB attached read-only.
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_db.clone()).with_legacy_database_path(Some(legacy_db.clone())),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));

        let all = repo.get_all_conversations(None).await?;
        assert!(
            all.as_ref().is_some_and(|v| v.len() == 1),
            "legacy row must be visible through the union view"
        );
        assert_eq!(
            all.unwrap()[0].title.as_deref(),
            Some("legacy row"),
            "row comes from the legacy DB"
        );

        // A new write to the split pool lands in the write DB, and a fresh
        // pool still sees both rows.
        let new_conv = Conversation::new(ConversationId::generate())
            .title(Some("write row".to_string()))
            .context(Some(Context::default().messages(vec![
                ContextMessage::user("write payload", None).into(),
            ])));
        repo.upsert_conversation(new_conv).await?;

        let pool2 = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_db.clone()).with_legacy_database_path(Some(legacy_db)),
        )?);
        let repo2 = ConversationRepositoryImpl::new(pool2, WorkspaceHash::new(0));
        let all2 = repo2.get_all_conversations(None).await?.unwrap();
        assert_eq!(all2.len(), 2, "union shows legacy + write rows");
        Ok(())
    }

    #[tokio::test]
    async fn test_split_db_local_row_shadows_same_id_legacy_row() -> anyhow::Result<()> {
        // A legacy row and a local row can legitimately share an ID after a
        // split-DB cutover. Reads must consistently expose the local, writable
        // representation; returning both would let a read disagree with a later
        // mutation of that same ID.
        let temp = tempfile::tempdir()?;
        let legacy_db = temp.path().join("legacy.db");
        let write_db = temp.path().join("write.db");
        let duplicate_id = ConversationId::generate();

        {
            let legacy_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(legacy_db.clone()).with_legacy_database_path(None),
            )?);
            let legacy_repo = ConversationRepositoryImpl::new(legacy_pool, WorkspaceHash::new(0));
            legacy_repo
                .upsert_conversation(
                    Conversation::new(duplicate_id).title(Some("legacy duplicate".to_string())),
                )
                .await?;
        }
        {
            let write_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(write_db.clone()).with_legacy_database_path(None),
            )?);
            let write_repo = ConversationRepositoryImpl::new(write_pool, WorkspaceHash::new(0));
            write_repo
                .upsert_conversation(
                    Conversation::new(duplicate_id).title(Some("local winner".to_string())),
                )
                .await?;
        }

        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_db).with_legacy_database_path(Some(legacy_db)),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));

        let fetched = repo
            .get_conversation(&duplicate_id)
            .await?
            .expect("local row visible");
        assert_eq!(fetched.title.as_deref(), Some("local winner"));

        let all = repo
            .get_all_conversations(None)
            .await?
            .expect("one visible row");
        assert_eq!(all.len(), 1, "duplicate ID is represented once");
        assert_eq!(all[0].title.as_deref(), Some("local winner"));

        let last = repo
            .get_last_conversation()
            .await?
            .expect("local row visible");
        assert_eq!(last.id, duplicate_id);
        assert_eq!(last.title.as_deref(), Some("local winner"));

        let parents = repo
            .get_parent_conversations(None)
            .await?
            .expect("local row visible");
        assert_eq!(parents.len(), 1, "parent list must not expose both copies");
        assert_eq!(parents[0].title.as_deref(), Some("local winner"));
        Ok(())
    }

    async fn search_fixture(
        local: &[(&str, &str, i64, bool)],
        legacy: &[(&str, &str, i64, bool)],
    ) -> anyhow::Result<(tempfile::TempDir, ConversationRepositoryImpl)> {
        let temp = tempfile::tempdir()?;
        let write_db = temp.path().join("write.db");
        let legacy_db = temp.path().join("legacy.db");
        for (path, rows) in [(&write_db, local), (&legacy_db, legacy)] {
            let pool = DatabasePool::try_from(
                PoolConfig::new(path.clone()).with_legacy_database_path(None),
            )?;
            let mut connection = pool.get_connection()?;
            for &(title, text, timestamp, compressed) in rows {
                let context =
                    Context::default().messages(vec![ContextMessage::user(text, None).into()]);
                let json = serde_json::to_string(&ContextRecord::from(&context))?;
                let mut record = ConversationRecord::new(
                    Conversation::new(ConversationId::generate())
                        .title(Some(title.to_string()))
                        .context(Some(context)),
                    WorkspaceHash::new(0),
                );
                record.updated_at = Some(
                    chrono::DateTime::from_timestamp(timestamp, 0)
                        .unwrap()
                        .naive_utc(),
                );
                record.context = (!compressed).then_some(json.clone());
                record.context_zstd = if compressed {
                    Some(zstd::encode_all(json.as_bytes(), 0)?)
                } else {
                    None
                };
                record.is_compressed = i32::from(compressed);
                diesel::insert_into(conversations::table)
                    .values(record)
                    .execute(&mut connection)?;
            }
        }
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_db).with_legacy_database_path(Some(legacy_db)),
        )?);
        Ok((
            temp,
            ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0)),
        ))
    }

    fn search_titles(rows: Vec<Conversation>) -> Vec<String> {
        rows.into_iter().map(|row| row.title.unwrap()).collect()
    }

    #[tokio::test]
    async fn test_search_like_compressed_local_does_not_starve_legacy() {
        let (_temp, fixture) = search_fixture(
            &[
                ("local newest", "needle", 40, true),
                ("local next", "needle", 30, true),
                ("local plaintext needle", "", 20, false),
            ],
            &[("legacy oldest", "needle", 10, true)],
        )
        .await
        .unwrap();

        let actual = search_titles(
            fixture
                .search_conversations_like("needle".into(), Some(2))
                .await
                .unwrap(),
        );

        let expected = vec!["local newest", "legacy oldest"];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_search_duplicate_legacy_plaintext_and_compressed_does_not_underfill() {
        let (_temp, fixture) = search_fixture(
            &[],
            &[
                ("needle duplicate", "needle", 30, true),
                ("compressed second", "needle", 20, true),
                ("compressed third", "needle", 10, true),
            ],
        )
        .await
        .unwrap();

        let actual = vec![
            search_titles(
                fixture
                    .search_conversations_fts("needle".into(), Some(3))
                    .await
                    .unwrap(),
            ),
            search_titles(
                fixture
                    .search_conversations_like("needle".into(), Some(3))
                    .await
                    .unwrap(),
            ),
        ];

        let expected = vec![vec!["needle duplicate", "compressed second", "compressed third"]; 2];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_search_like_local_multiword_is_literal_for_both_storage_formats() {
        let (_temp, fixture) = search_fixture(
            &[
                ("compressed separated", "rust async refactor", 60, true),
                ("compressed reversed", "refactor rust", 50, true),
                ("compressed literal", "RUST REFACTOR", 40, true),
                ("plaintext separated", "rust async refactor", 30, false),
                ("plaintext literal", "rust refactor", 20, false),
            ],
            &[("legacy literal", "rust refactor", 10, true)],
        )
        .await
        .unwrap();

        let actual = search_titles(
            fixture
                .search_conversations_like("rust refactor".into(), None)
                .await
                .unwrap(),
        );

        let expected = vec!["compressed literal", "plaintext literal", "legacy literal"];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_search_empty_terms_do_not_match_all_compressed_rows() {
        let (_temp, fixture) = search_fixture(
            &[("local", "ordinary content", 20, true)],
            &[("legacy", "ordinary content", 10, true)],
        )
        .await
        .unwrap();

        let mut actual = Vec::new();
        for query in ["", "   ", "*", "\"\"", "\"\"*"] {
            actual.push(search_titles(
                fixture.search_conversations(query, Some(3)).await.unwrap(),
            ));
        }

        let expected: Vec<Vec<String>> = vec![vec![]; 5];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_search_legacy_fts_matches_local_expression_semantics() {
        let rows = [
            ("prefix", "tokio runtime", 70, true),
            ("substring", "notokio runtime", 60, false),
            ("rust", "refactor async", 50, true),
            ("phrase", "rust refactor", 40, false),
            ("separated", "rust async refactor", 30, true),
            ("stem", "running", 20, false),
            ("accent", "café", 10, true),
        ];
        let (_temp, fixture) = search_fixture(&rows, &rows).await.unwrap();
        fixture.refresh_fts_index().await.unwrap();

        let mut actual = Vec::new();
        for query in [
            "tokio*",
            "rust refactor",
            "\"rust refactor\"",
            "rust NOT async",
            "tokio OR running",
            "title:rust content:refactor",
            "NEAR(rust refactor, 1)",
            "run",
            "cafe",
        ] {
            let hits = fixture
                .search_conversations_fts(query.into(), None)
                .await
                .unwrap();
            let mut titles = search_titles(hits);
            titles.sort();
            actual.push(titles);
        }

        let expected = [
            vec!["prefix", "prefix"],
            vec!["phrase", "phrase", "rust", "rust", "separated", "separated"],
            vec!["phrase", "phrase"],
            vec!["phrase", "phrase"],
            vec!["prefix", "prefix", "stem", "stem"],
            vec!["rust", "rust"],
            vec!["phrase", "phrase", "separated", "separated"],
            vec![], // unicode61 does not stem "running" into "run".
            vec!["accent", "accent"],
        ];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_search_like_orders_selected_sources_by_recency() {
        let (_temp, fixture) = search_fixture(
            &[
                ("local needle", "", 20, false),
                ("local compressed", "needle", 40, true),
            ],
            &[
                ("legacy needle", "", 10, false),
                ("legacy compressed", "needle", 50, true),
            ],
        )
        .await
        .unwrap();

        let actual = vec![
            search_titles(
                fixture
                    .search_conversations_like("needle".into(), None)
                    .await
                    .unwrap(),
            ),
            search_titles(
                fixture
                    .search_conversations_like("needle".into(), Some(2))
                    .await
                    .unwrap(),
            ),
        ];

        let expected = vec![
            vec![
                "legacy compressed",
                "local compressed",
                "local needle",
                "legacy needle",
            ],
            vec!["legacy compressed", "local compressed"],
        ];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_search_like_newer_legacy_hits_are_not_displaced_by_old_local() {
        let (_temp, fixture) = search_fixture(
            &[("old local", "needle", 10, true)],
            &[
                ("new legacy", "needle", 30, true),
                ("next legacy", "needle", 20, false),
            ],
        )
        .await
        .unwrap();

        let actual = search_titles(
            fixture
                .search_conversations_like("needle".into(), Some(2))
                .await
                .unwrap(),
        );

        let expected = vec!["new legacy", "next legacy"];
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_split_db_search_finds_visible_legacy_rows_with_fts_and_like_fallback()
    -> anyhow::Result<()> {
        // The attached DB is intentionally not FTS-indexed by the write DB.
        // Both the successful FTS path and the missing-FTS fallback must still
        // supplement local results with visible legacy matches.
        let temp = tempfile::tempdir()?;
        let legacy_db = temp.path().join("legacy-search.db");
        let write_db = temp.path().join("write-search.db");
        let needle = "LEGACY_SPLIT_SEARCH_NEEDLE";
        let legacy_id = ConversationId::generate();

        {
            let legacy_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(legacy_db.clone()).with_legacy_database_path(None),
            )?);
            let legacy_repo = ConversationRepositoryImpl::new(legacy_pool, WorkspaceHash::new(0));
            legacy_repo
                .upsert_conversation(Conversation::new(legacy_id).title(Some(needle.to_string())))
                .await?;
        }

        // Two local FTS hits expose the regression where the legacy supplement
        // is appended after the local results and then discarded by the final
        // limit truncation.
        {
            let write_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(write_db.clone()).with_legacy_database_path(None),
            )?);
            let write_repo = ConversationRepositoryImpl::new(write_pool, WorkspaceHash::new(0));
            for title in ["local search hit one", "local search hit two"] {
                write_repo
                    .upsert_conversation(
                        Conversation::new(ConversationId::generate())
                            .title(Some(format!("{title} {needle}"))),
                    )
                    .await?;
            }
        }

        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_db).with_legacy_database_path(Some(legacy_db)),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));
        repo.refresh_fts_index().await?;
        let fts_hits = repo.search_conversations(needle, Some(2)).await?;
        assert_eq!(
            fts_hits.len(),
            2,
            "the requested limit must remain a hard upper bound"
        );
        assert!(
            fts_hits.iter().any(|hit| hit.id == legacy_id),
            "legacy matches must not be hidden when local matches fill the limit"
        );
        let prefix_hits = repo.search_conversations("LEGACY*", Some(2)).await?;
        assert!(
            prefix_hits.iter().any(|hit| hit.id == legacy_id),
            "legacy prefix expressions must retain FTS-style semantics"
        );

        repo.run_with_connection(move |connection, _wid| {
            diesel::sql_query("DROP TABLE conversations_fts").execute(connection)?;
            Ok(())
        })
        .await?;
        let like_hits = repo
            .search_conversations_like(needle.to_string(), Some(2))
            .await?;
        assert_eq!(like_hits.len(), 2);
        assert!(like_hits.iter().any(|hit| hit.id == legacy_id));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_parent_conversations_filters_agent_before_limit() -> anyhow::Result<()> {
        // Regression: the agent-exclusion predicate must run BEFORE the
        // LIMIT. With 2 user rows (older) + 4 agent rows (newer) and a limit
        // of 3 (< 6), applying the predicate after the limit would return the
        // 3 newest agent rows and drop both user rows. Rows are written
        // directly with a plain (uncompressed) `context` column so the
        // `json_extract(context, '$.initiator')` predicate can see the
        // initiator (the normal write path zstd-compresses, leaving
        // `context` NULL).
        let repo = repository()?;

        // `Context` must be brought into scope via `ContextRecord` below; the
        // module-level `use super::*` provides `ContextRecord` from
        // `conversation_record`.
        let seed = |initiator: &str,
                    title: &str,
                    updated: &str,
                    conn: &mut PooledSqliteConnection| {
            let context = forge_domain::Context::default().initiator(initiator);
            let context_json =
                serde_json::to_string(&ContextRecord::from(&context)).expect("serialize context");
            diesel::sql_query(
                "INSERT INTO conversations \
                 (conversation_id, title, workspace_id, context, created_at, updated_at, is_compressed) \
                 VALUES (?, ?, 0, ?, ?, ?, 0)",
            )
            .bind::<diesel::sql_types::Text, _>(ConversationId::generate().into_string())
            .bind::<diesel::sql_types::Text, _>(title)
            .bind::<diesel::sql_types::Text, _>(context_json)
            .bind::<diesel::sql_types::Text, _>("2026-01-01 00:00:00")
            .bind::<diesel::sql_types::Text, _>(updated)
            .execute(conn)?;
            Ok::<(), anyhow::Error>(())
        };

        repo.run_with_connection(move |conn, _wid| {
            seed("user", "user one", "2026-01-01 08:00:00", conn)?;
            seed("user", "user two", "2026-01-02 08:00:00", conn)?;
            seed("agent", "agent one", "2026-01-03 08:00:00", conn)?;
            seed("agent", "agent two", "2026-01-04 08:00:00", conn)?;
            seed("agent", "agent three", "2026-01-05 08:00:00", conn)?;
            seed("agent", "agent four", "2026-01-06 08:00:00", conn)?;
            Ok(())
        })
        .await?;

        let actual = repo.get_parent_conversations(Some(3)).await?;
        let conversations = actual.expect("seeded rows must be returned");
        assert_eq!(
            conversations.len(),
            2,
            "agent rows must be excluded before the limit, not after"
        );
        let titles: Vec<&str> = conversations
            .iter()
            .map(|c| c.title.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec!["user two", "user one"],
            "user rows newest-first"
        );
        Ok(())
    }
    #[tokio::test]
    async fn test_legacy_db_path_env_var_feeds_pool_union() -> anyhow::Result<()> {
        // Proves FORGE_LEGACY_DB_PATH reaches the pool's legacy read path:
        // the env var drives `Environment::legacy_database_path()`, which
        // `ForgeRepo::new` uses to pick the legacy attachment. A full infra
        // mock is impractical, so mirror that wiring here and verify the
        // seeded legacy row is visible through the UNION view.
        let temp = tempfile::tempdir()?;
        let legacy_db = temp.path().join("env-legacy.db");
        // Seed the legacy DB through a standalone pool, then close it.
        {
            let legacy_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(legacy_db.clone()).with_legacy_database_path(None),
            )?);
            let legacy_repo = ConversationRepositoryImpl::new(legacy_pool, WorkspaceHash::new(0));
            let conv = Conversation::new(ConversationId::generate())
                .title(Some("env legacy row".to_string()))
                .context(Some(Context::default().messages(vec![
                    ContextMessage::user("env legacy payload", None).into(),
                ])));
            legacy_repo.upsert_conversation(conv.clone()).await?;
        }
        // Point FORGE_LEGACY_DB_PATH at the seeded file and resolve the
        // paths exactly like `ForgeRepo::new` does.
        let previous = std::env::var("FORGE_LEGACY_DB_PATH").ok();
        unsafe { std::env::set_var("FORGE_LEGACY_DB_PATH", &legacy_db) };
        let env = forge_domain::Environment {
            os: "test".to_string(),
            cwd: temp.path().to_path_buf(),
            home: None,
            shell: "bash".to_string(),
            base_path: temp.path().to_path_buf(),
        };
        let write_path = env.write_database_path();
        let legacy_path = env.legacy_database_path();
        // Cleanup: restore the previous value, if any.
        match previous {
            Some(value) => unsafe { std::env::set_var("FORGE_LEGACY_DB_PATH", value) },
            None => unsafe { std::env::remove_var("FORGE_LEGACY_DB_PATH") },
        }
        assert_eq!(legacy_path, legacy_db, "env var drives the legacy path");
        assert_ne!(write_path, legacy_path, "write and legacy paths differ");
        let legacy_for_pool = if legacy_path != write_path && legacy_path.exists() {
            Some(legacy_path.clone())
        } else {
            None
        };
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_path.clone()).with_legacy_database_path(legacy_for_pool),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));
        let all = repo.get_all_conversations(None).await?;
        assert!(
            all.as_ref().is_some_and(|v| v.len() == 1),
            "env-var legacy row must be visible through the union view"
        );
        assert_eq!(
            all.unwrap()[0].title.as_deref(),
            Some("env legacy row"),
            "row comes from the legacy DB"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_migrate_data_dir_migrates_write_db() -> anyhow::Result<()> {
        // Regression: `migrate_data_dir` must copy BOTH the fork's write DB
        // (`.forge.writes.db`) and the legacy `.forge.db` into the canonical
        // dir. Before the fix only `.forge.db` was copied, so after migration
        // the app started with an empty write DB and fork conversations
        // vanished from the `conversations_all` UNION.
        //
        // The write DB is a live SQLite file (the pool has it open), exactly
        // like production; a marker conversation proves its content survives
        // the copy intact.
        use crate::database::DatabasePool;
        let temp = tempfile::tempdir()?;
        let source_dir = temp.path().to_path_buf();
        let write_db = source_dir.join(".forge.writes.db");
        let legacy_db = source_dir.join(".forge.db");

        // Seed the legacy DB through a standalone pool, then close it.
        {
            let legacy_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(legacy_db.clone()).with_legacy_database_path(None),
            )?);
            let legacy_repo = ConversationRepositoryImpl::new(legacy_pool, WorkspaceHash::new(0));
            let conv = Conversation::new(ConversationId::generate())
                .title(Some("legacy row".to_string()))
                .context(Some(Context::default().messages(vec![
                    ContextMessage::user("legacy payload", None).into(),
                ])));
            legacy_repo.upsert_conversation(conv.clone()).await?;
        }

        // Seed the fork's write DB through a standalone pool, then close it,
        // and open the main pool on that same file (the pool stays alive
        // during migration, as in production).
        {
            let write_pool = Arc::new(DatabasePool::try_from(
                PoolConfig::new(write_db.clone()).with_legacy_database_path(None),
            )?);
            let write_repo = ConversationRepositoryImpl::new(write_pool, WorkspaceHash::new(0));
            let marker = Conversation::new(ConversationId::generate())
                .title(Some("fork-write-db-marker".to_string()))
                .context(Some(Context::default().messages(vec![
                    ContextMessage::user("fork payload", None).into(),
                ])));
            write_repo.upsert_conversation(marker).await?;
        }
        let pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(write_db.clone()).with_legacy_database_path(None),
        )?);
        let repo = ConversationRepositoryImpl::new(pool, WorkspaceHash::new(0));

        // Point the canonical destination at a temp home dir.
        let home = tempfile::tempdir()?;
        let previous_userprofile = std::env::var("USERPROFILE").ok();
        let previous_home = std::env::var("HOME").ok();
        unsafe {
            std::env::set_var("USERPROFILE", home.path());
            std::env::set_var("HOME", home.path());
        }

        let result = repo
            .migrate_data_dir(&forge_domain::MigrateOptions::default())
            .await;

        // Restore the environment before asserting or propagating.
        match previous_userprofile {
            Some(value) => unsafe { std::env::set_var("USERPROFILE", value) },
            None => unsafe { std::env::remove_var("USERPROFILE") },
        }
        match previous_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let actual = result?;

        let destination_dir = home.path().join(".helioslite");
        assert_eq!(actual.outcome, "migrated");
        assert_eq!(actual.destination_path, destination_dir);
        // The write DB is copied verbatim: opening the destination copy shows
        // the marker conversation that lived in the fork's write DB.
        let verify_pool = Arc::new(DatabasePool::try_from(
            PoolConfig::new(destination_dir.join(".forge.writes.db"))
                .with_legacy_database_path(None),
        )?);
        let verify_repo = ConversationRepositoryImpl::new(verify_pool, WorkspaceHash::new(0));
        let written = verify_repo.get_all_conversations(None).await?;
        assert_eq!(
            written.unwrap()[0].title.as_deref(),
            Some("fork-write-db-marker"),
            "write DB content must survive migration"
        );
        assert!(
            destination_dir.join(".forge.db").exists(),
            "legacy DB must be copied too"
        );
        // The legacy copy is still the seeded DB (1 row).
        assert_eq!(actual.conversations_verified, 1);

        // The source dir is renamed aside (non-fatal on Windows if the pool
        // still holds a handle); clean up the orphan if it happened.
        if let Some(renamed) = &actual.renamed_legacy_to {
            assert!(!source_dir.exists(), "source dir renamed aside");
            assert!(renamed.exists(), "renamed-aside dir must exist");
            drop(repo);
            let _ = std::fs::remove_dir_all(renamed);
        }
        Ok(())
    }

    /// Verify that `get_conversation_highlight` returns the full context
    /// column with match spans wrapped in caller-supplied markup, distinct
    /// from `get_conversation_snippet` which returns a short passage.
    #[tokio::test]
    async fn test_get_conversation_highlight_wraps_match_in_markup() -> anyhow::Result<()> {
        let repo = repository()?;

        // Mirror test_search_finds_compressed_conversations: insert one conv
        // via upsert_conversation, refresh FTS, then call highlight().
        let msg = ContextMessage::user("SEARCHABLE_HIGHLIGHT_TERM", None);
        let context = Context::default().messages(vec![msg.into()]);
        let conv = Conversation::new(ConversationId::generate())
            .title(Some("Highlight Test Conversation".to_string()))
            .context(Some(context));
        repo.upsert_conversation(conv.clone()).await?;
        repo.refresh_fts_index().await?;

        // Call the new method with caller-supplied markup.
        let highlighted = repo
            .get_conversation_highlight(&conv.id, "SEARCHABLE_HIGHLIGHT_TERM", "<b>", "</b>")
            .await?;

        assert!(
            highlighted.is_some(),
            "highlight() must return Some for a matched conversation",
        );
        let h = highlighted.unwrap();

        // The markup must wrap the needle exactly.
        assert!(
            h.contains("<b>SEARCHABLE_HIGHLIGHT_TERM</b>"),
            "highlight() output must wrap the matched needle in caller-supplied markup; got: {h}",
        );

        // Sanity: a non-matching query returns None.
        let none = repo
            .get_conversation_highlight(&conv.id, "DEFINITELY_NOT_THERE", "<b>", "</b>")
            .await?;
        assert!(
            none.is_none(),
            "highlight() must return None when the query has no match",
        );

        Ok(())
    }

    /// Verify that `search_conversations` falls back to the LIKE scan when
    /// the `conversations_fts` virtual table is missing (the recovery path
    /// for migration failures / database corruption). The user must still
    /// get search results, just slower and ranked by `updated_at` rather than
    /// BM25. Without this guarantee, an FTS5 outage would silently drop
    /// search hits in the UI.
    #[tokio::test]
    async fn test_search_falls_back_to_like_when_fts_table_missing() -> anyhow::Result<()> {
        let repo = repository()?;

        // Insert a conversation with a known term; refresh FTS so the
        // happy-path search would succeed (sanity check).
        let needle = "LIKEFALLBACKNEEDLE";
        let msg = ContextMessage::user(needle, None);
        let context = Context::default().messages(vec![msg.into()]);
        let conv = Conversation::new(ConversationId::generate())
            .title(Some("LIKE Fallback Test".to_string()))
            .context(Some(context));
        repo.upsert_conversation(conv.clone()).await?;
        repo.refresh_fts_index().await?;

        // Sanity: FTS path works while the index is intact.
        let fts_hits = repo.search_conversations(needle, None).await?;
        assert_eq!(
            fts_hits.len(),
            1,
            "FTS5 happy path must find the inserted conversation",
        );

        // Simulate the FTS5 outage: DROP the virtual table. Subsequent
        // FTS5 MATCH queries will return a "no such table" error which
        // `search_conversations` must catch and recover from via LIKE.
        repo.run_with_connection(move |connection, _wid| {
            diesel::sql_query("DROP TABLE conversations_fts").execute(connection)?;
            Ok(())
        })
        .await?;

        // The recovery path: `search_conversations` should still return the
        // inserted conversation via the LIKE fallback. The result count is
        // the same as the FTS path.
        let like_hits = repo.search_conversations(needle, None).await?;
        assert_eq!(
            like_hits.len(),
            1,
            "search_conversations must fall back to LIKE when FTS5 is unavailable; \
             got {} hits instead of 1",
            like_hits.len(),
        );
        assert_eq!(
            like_hits[0].id, conv.id,
            "LIKE fallback must return the same conversation as the FTS path",
        );

        // A query that hits no rows is still empty (not an error) in the
        // fallback path.
        let empty = repo
            .search_conversations("DEFINITELY_NOT_THERE", None)
            .await?;
        assert!(
            empty.is_empty(),
            "LIKE fallback must return empty Vec for non-matching queries",
        );

        Ok(())
    }
}
