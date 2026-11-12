use std::path::Path;

use diesel::prelude::*;
use forge_domain::{ForgeImportOptions, ForgeImportReport, WorkspaceHash};

use super::{DecodedSourceRow, import_rows};

pub(super) fn scan(
    src: &mut diesel::SqliteConnection,
    destination: &mut diesel::SqliteConnection,
    _source: &Path,
    wid: WorkspaceHash,
    options: &ForgeImportOptions,
    require_valid_context: bool,
    source_byte_limit: i64,
) -> anyhow::Result<ForgeImportReport> {
    anyhow::ensure!(
        options.dry_run,
        "unselected import is disabled; select IDs explicitly"
    );
    src.transaction::<_, anyhow::Error, _>(|src| {
        #[derive(diesel::QueryableByName)]
        struct ScanLimit {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            oversized: i64,
        }
        let limit: ScanLimit = diesel::sql_query(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE \
             COALESCE(length(CAST(conversation_id AS BLOB)), 0) + \
             COALESCE(length(CAST(title AS BLOB)), 0) + \
             COALESCE(length(CAST(context AS BLOB)), 0) + \
             COALESCE(length(CAST(created_at AS BLOB)), 0) + \
             COALESCE(length(CAST(updated_at AS BLOB)), 0) + \
             COALESCE(length(CAST(metrics AS BLOB)), 0) > ?) AS oversized",
        )
        .bind::<diesel::sql_types::BigInt, _>(source_byte_limit)
        .get_result(src)?;
        anyhow::ensure!(
            limit.oversized == 0,
            "source contains a conversation exceeding the 8 MiB preview limit or its conservative encoding bound"
        );
        #[cfg(test)]
        if let Some(id) = {
            let mut hook = preview_snapshot_hook().lock().unwrap();
            if hook.as_ref().is_some_and(|(path, _)| path == _source) {
                hook.take().map(|(_, id)| id)
            } else {
                None
            }
        } {
            use diesel::sql_types::Text;
            let mut writer =
                diesel::sqlite::SqliteConnection::establish(&_source.to_string_lossy())?;
            diesel::sql_query(
                "UPDATE conversations SET context = CAST(zeroblob(8388609) AS TEXT) \
                 WHERE conversation_id = ?",
            )
            .bind::<Text, _>(id)
            .execute(&mut writer)?;
        }
        let query = diesel::sql_query(
            "SELECT conversation_id, title, context, created_at, updated_at, metrics \
             FROM conversations",
        );
        let mut rows = query.load_iter::<DecodedSourceRow, _>(src)?;
        let mut report = ForgeImportReport { dry_run: options.dry_run, ..Default::default() };
        destination.transaction::<_, anyhow::Error, _>(|conn| {
            for row in &mut rows {
                let row = row?;
                report.source_total += 1;
                import_rows(
                    conn,
                    wid,
                    std::slice::from_ref(&row),
                    options,
                    require_valid_context,
                    None,
                    &mut report,
                )?;
            }
            Ok(())
        })?;
        Ok(report)
    })
}

#[cfg(test)]
fn preview_snapshot_hook() -> &'static std::sync::Mutex<Option<(std::path::PathBuf, String)>> {
    static HOOK: std::sync::Mutex<Option<(std::path::PathBuf, String)>> =
        std::sync::Mutex::new(None);
    &HOOK
}

#[cfg(test)]
mod tests {
    use super::super::tests::{repository, seed_official_source_db};
    use super::preview_snapshot_hook;
    use crate::database::schema::conversations;
    use diesel::prelude::*;
    use forge_domain::{ConversationId, ConversationRepository};

    #[tokio::test]
    async fn test_import_preview_counts_large_history_without_writing() -> anyhow::Result<()> {
        use diesel::Connection;
        use diesel::sql_types::Text;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("large-source.db");
        seed_official_source_db(&source);
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        writer.transaction::<_, anyhow::Error, _>(|writer| {
            for _ in 0..512 {
                diesel::sql_query(
                    "INSERT INTO conversations (conversation_id, created_at) VALUES (?, ?)",
                )
                .bind::<Text, _>(ConversationId::generate().into_string())
                .bind::<Text, _>("2026-08-05 10:00:00")
                .execute(writer)?;
            }
            Ok(())
        })?;
        drop(writer);
        let source_before = std::fs::read(&source)?;
        let repo = repository()?;
        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };
        let actual = repo
            .import_forge_db_with_options(source.clone(), &options)
            .await?;

        assert_eq!(actual.source_total, 516);
        assert_eq!(actual.imported, 515);
        assert_eq!(actual.invalid_id, 1);
        assert_eq!(actual.errors, 0);
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        assert!(repo.get_conversation(&id).await?.is_none());
        let destination_rows = repo
            .run_with_connection(|conn, _| {
                Ok(conversations::table.count().get_result::<i64>(conn)?)
            })
            .await?;
        assert_eq!(destination_rows, 0);
        assert_eq!(std::fs::read(source)?, source_before);
        Ok(())
    }

    #[tokio::test]
    async fn test_import_preview_rejects_oversized_row_without_writing() -> anyhow::Result<()> {
        use diesel::Connection;
        use diesel::sql_types::Text;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("oversized-source.db");
        seed_official_source_db(&source);
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        let context = forge_domain::Context::default().messages(vec![
            forge_domain::ContextMessage::user("boundary", None).into(),
        ]);
        let context = serde_json::to_string(&super::super::ContextRecord::from(&context))?;
        let available = 8_388_608 - 36 - 19 - context.len();
        let mut title = "é".repeat(available / 2);
        if available % 2 != 0 {
            title.push('a');
        }
        diesel::sql_query(
            "UPDATE conversations SET title = ?, context = ?, updated_at = NULL \
             WHERE conversation_id = '11111111-1111-1111-1111-111111111111'",
        )
        .bind::<Text, _>(title)
        .bind::<Text, _>(context)
        .execute(&mut writer)?;
        let repo = repository()?;
        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };
        let at_limit = repo
            .import_forge_db_with_options(source.clone(), &options)
            .await?;
        assert_eq!(at_limit.source_total, 4);
        let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
        let selected = repo
            .import_forge_db_selected(source.clone(), &[id], false, false, None, &options)
            .await?;
        assert_eq!(selected.source_total, 1);
        diesel::sql_query(
            "UPDATE conversations SET title = title || 'a' \
             WHERE conversation_id = '11111111-1111-1111-1111-111111111111'",
        )
        .execute(&mut writer)?;
        drop(writer);
        let source_before = std::fs::read(&source)?;
        let error = repo
            .import_forge_db_with_options(source.clone(), &options)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("exceeding the 8 MiB preview limit")
        );
        assert!(
            repo.import_forge_db_selected(source.clone(), &[id], false, false, None, &options)
                .await
                .is_err()
        );
        let destination_rows = repo
            .run_with_connection(|conn, _| {
                Ok(conversations::table.count().get_result::<i64>(conn)?)
            })
            .await?;
        assert_eq!(destination_rows, 0);
        assert_eq!(std::fs::read(source)?, source_before);
        Ok(())
    }

    #[tokio::test]
    async fn test_import_preview_uses_one_snapshot_across_size_check() -> anyhow::Result<()> {
        use diesel::Connection;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("changing-wal.db");
        seed_official_source_db(&source);
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        diesel::sql_query("PRAGMA journal_mode=WAL").execute(&mut writer)?;
        *preview_snapshot_hook().lock().unwrap() = Some((
            source.clone(),
            "11111111-1111-1111-1111-111111111111".into(),
        ));
        let repo = repository()?;
        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };

        let actual = repo
            .import_forge_db_with_options(source.clone(), &options)
            .await?;
        assert_eq!(
            (actual.source_total, actual.imported, actual.invalid_id),
            (4, 3, 1)
        );
        assert!(preview_snapshot_hook().lock().unwrap().is_none());
        let error = repo
            .import_forge_db_with_options(source, &options)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exceeding the 8 MiB preview limit")
        );
        let destination_rows = repo
            .run_with_connection(|conn, _| {
                Ok(conversations::table.count().get_result::<i64>(conn)?)
            })
            .await?;
        assert_eq!(destination_rows, 0);
        drop(writer);
        Ok(())
    }

    #[tokio::test]
    async fn test_selected_preview_rejects_oversized_duplicate() -> anyhow::Result<()> {
        use diesel::Connection;
        use diesel::sql_types::Text;

        let temp = tempfile::tempdir()?;
        let source = temp.path().join("duplicate-source.db");
        let mut writer = diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
        diesel::sql_query(
            "CREATE TABLE conversations (conversation_id TEXT, title TEXT, context TEXT, \
             created_at TEXT, updated_at TEXT, metrics TEXT)",
        )
        .execute(&mut writer)?;
        let context = forge_domain::Context::default().messages(vec![
            forge_domain::ContextMessage::user("valid", None).into(),
        ]);
        let context = serde_json::to_string(&super::super::ContextRecord::from(&context))?;
        let id = "11111111-1111-1111-1111-111111111111";
        diesel::sql_query(
            "INSERT INTO conversations (conversation_id, context, created_at) VALUES (?, ?, ?)",
        )
        .bind::<Text, _>(id)
        .bind::<Text, _>(context)
        .bind::<Text, _>("2026-08-05 10:00:00")
        .execute(&mut writer)?;
        diesel::sql_query(
            "INSERT INTO conversations (conversation_id, context, created_at) \
             VALUES (?, CAST(zeroblob(8388609) AS TEXT), ?)",
        )
        .bind::<Text, _>(id)
        .bind::<Text, _>("2026-08-05 10:00:00")
        .execute(&mut writer)?;
        drop(writer);
        let source_before = std::fs::read(&source)?;
        let repo = repository()?;
        let id = ConversationId::parse(id)?;
        let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };

        let error = repo
            .import_forge_db_selected(source.clone(), &[id], false, false, None, &options)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("absent or duplicated"));
        let destination_rows = repo
            .run_with_connection(|conn, _| {
                Ok(conversations::table.count().get_result::<i64>(conn)?)
            })
            .await?;
        assert_eq!(destination_rows, 0);
        assert_eq!(std::fs::read(source)?, source_before);
        Ok(())
    }

    #[tokio::test]
    async fn test_utf16_preview_bounds_utf8_expansion() -> anyhow::Result<()> {
        use diesel::Connection;
        use diesel::sql_types::Text;

        let temp = tempfile::tempdir()?;
        for encoding in ["UTF-16le", "UTF-16be"] {
            let source = temp.path().join(format!("{encoding}-source.db"));
            let mut writer =
                diesel::sqlite::SqliteConnection::establish(&source.to_string_lossy())?;
            diesel::sql_query(format!("PRAGMA encoding = '{encoding}'")).execute(&mut writer)?;
            diesel::sql_query(
                "CREATE TABLE conversations (conversation_id TEXT, title TEXT, context TEXT, \
             created_at TEXT, updated_at TEXT, metrics TEXT)",
            )
            .execute(&mut writer)?;
            #[derive(diesel::QueryableByName)]
            struct Encoding {
                #[diesel(sql_type = Text)]
                encoding: String,
            }
            let actual: Encoding = diesel::sql_query("PRAGMA encoding").get_result(&mut writer)?;
            assert_eq!(actual.encoding, encoding);
            let context = forge_domain::Context::default().messages(vec![
                forge_domain::ContextMessage::user("valid", None).into(),
            ]);
            let context = serde_json::to_string(&super::super::ContextRecord::from(&context))?;
            let id = ConversationId::parse("11111111-1111-1111-1111-111111111111")?;
            diesel::sql_query(
                "INSERT INTO conversations (conversation_id, title, context, created_at) \
             VALUES (?, ?, ?, ?)",
            )
            .bind::<Text, _>(id.into_string())
            .bind::<Text, _>("small")
            .bind::<Text, _>(context)
            .bind::<Text, _>("2026-08-05 10:00:00")
            .execute(&mut writer)?;
            let repo = repository()?;
            let options = forge_domain::ForgeImportOptions { dry_run: true, verbose: false };
            assert_eq!(
                repo.import_forge_db_with_options(source.clone(), &options)
                    .await?
                    .source_total,
                1
            );
            assert_eq!(
                repo.import_forge_db_selected(source.clone(), &[id], false, false, None, &options)
                    .await?
                    .source_total,
                1
            );
            diesel::sql_query("UPDATE conversations SET title = ?")
                .bind::<Text, _>("ࠀ".repeat(2_800_000))
                .execute(&mut writer)?;
            drop(writer);
            let source_before = std::fs::read(&source)?;

            let error = repo
                .import_forge_db_with_options(source.clone(), &options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("conservative encoding bound"));
            let error = repo
                .import_forge_db_selected(source.clone(), &[id], false, false, None, &options)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("conservative encoding bound"));
            let destination_rows = repo
                .run_with_connection(|conn, _| {
                    Ok(conversations::table.count().get_result::<i64>(conn)?)
                })
                .await?;
            assert_eq!(destination_rows, 0);
            assert_eq!(std::fs::read(source)?, source_before);
        }
        Ok(())
    }
}
