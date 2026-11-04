// SPDX-License-Identifier: Apache-2.0
//! Writes for the rebuild (`store::rebuild`): rows read back from a lossless
//! export go into a fresh database as they were stored. Only for a database
//! the rebuild just created; never the person's.

use super::table_classes::TABLE_CLASSES;
use super::MemoryDB;
use crate::error::WenlanError;

/// How a foreign key problem starts: `foreign key: <table> -> <parent>
/// (row <rowid>)`, a row of `table` naming a `parent` row that is not there.
pub(crate) const FOREIGN_KEY: &str = "foreign key: ";

/// Tables whose triggers are set aside while the rebuild writes them. The
/// `edges` triggers check each new edge against its endpoints as they are
/// now (the space fence refuses an edge whose page moved to another space
/// or is gone) and mark grouping work; a copy goes back as it was stored,
/// and one stale edge must not stop a restore.
const WRITTEN_WITHOUT_TRIGGERS: &[&str] = &["edges"];

impl MemoryDB {
    /// After a restore: works out each page's links to pages not written
    /// yet (`page_links`, which is not in the files) again from its text.
    /// A link that has a target is an edge, and the edges come back from
    /// the files as stored: resolving them again by title could send a
    /// link to another page of the same name, so they are left alone.
    /// Returns how many pages were done; a page that fails is logged and
    /// skipped.
    pub async fn rebuild_page_links(&self) -> Result<usize, WenlanError> {
        let pages = {
            let conn = self.conn.lock().await;
            let fail = |e: libsql::Error| WenlanError::VectorDb(format!("rebuild page links: {e}"));
            let mut rows = conn
                .query("SELECT id, content FROM pages ORDER BY id", ())
                .await
                .map_err(fail)?;
            let mut pages = Vec::new();
            while let Some(row) = rows.next().await.map_err(fail)? {
                let id: String = row.get(0).map_err(fail)?;
                let content: Option<String> = row.get(1).map_err(fail)?;
                pages.push((id, content.unwrap_or_default()));
            }
            pages
        };
        let mut done = 0;
        for (id, content) in pages {
            match self.rebuild_unwritten_links(&id, &content).await {
                Ok(()) => done += 1,
                Err(error) => log::warn!("[restore] links of page {id} not worked out: {error}"),
            }
        }
        Ok(done)
    }

    /// One page's links that have no stored edge, written as `page_links`
    /// rows with no target, as a save left them: the next resolve pass
    /// gives each a target once its page exists.
    async fn rebuild_unwritten_links(
        &self,
        page_id: &str,
        content: &str,
    ) -> Result<(), WenlanError> {
        let labels = crate::synthesis::wikilinks::extract_wikilinks(content);
        if labels.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().await;
        let fail = |e: libsql::Error| WenlanError::VectorDb(format!("rebuild page links: {e}"));
        let mut stored = std::collections::HashSet::new();
        let mut rows = conn
            .query(
                "SELECT json_extract(payload, '$.label') FROM edges \
                 WHERE src_id = ?1 AND src_kind = 'page' AND edge_type = 'links' \
                   AND valid_until IS NULL AND json_valid(payload)",
                [page_id],
            )
            .await
            .map_err(fail)?;
        while let Some(row) = rows.next().await.map_err(fail)? {
            if let Some(label) = row.get::<Option<String>>(0).map_err(fail)? {
                stored.insert(Self::page_title_key(&label));
            }
        }
        drop(rows);
        conn.execute(
            "DELETE FROM page_links WHERE source_page_id = ?1",
            [page_id],
        )
        .await
        .map_err(fail)?;
        for label in labels {
            let key = Self::page_title_key(&label);
            if stored.contains(&key) {
                continue;
            }
            conn.execute(
                "INSERT OR IGNORE INTO page_links (source_page_id, target_page_id, label_key, label) \
                 VALUES (?1, NULL, ?2, ?3)",
                libsql::params![page_id, key, label],
            )
            .await
            .map_err(fail)?;
        }
        Ok(())
    }

    /// Replaces every row of `table` with `rows`, each holding a value for
    /// each of `columns`, in one transaction. `table` must be one of the
    /// classified tables; column names are quoted.
    pub(crate) async fn rebuild_replace_table(
        &self,
        table: &str,
        columns: &[String],
        rows: &[Vec<libsql::Value>],
    ) -> Result<(), WenlanError> {
        if !TABLE_CLASSES.iter().any(|(name, _)| *name == table) {
            return Err(WenlanError::Validation(format!(
                "rebuild cannot write table {table:?}"
            )));
        }
        let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        let names = columns
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ");
        let marks = (1..=columns.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let insert = format!("INSERT INTO {} ({names}) VALUES ({marks})", quote(table));
        let conn = self.conn.lock().await;
        let fail = |e: libsql::Error| WenlanError::VectorDb(format!("rebuild {table}: {e}"));
        conn.execute("BEGIN IMMEDIATE", ()).await.map_err(fail)?;
        let result = async {
            let mut triggers = Vec::new();
            if WRITTEN_WITHOUT_TRIGGERS.contains(&table) {
                let mut found = conn
                    .query(
                        "SELECT name, sql FROM sqlite_master \
                         WHERE type = 'trigger' AND tbl_name = ?1 ORDER BY name",
                        [table],
                    )
                    .await
                    .map_err(fail)?;
                while let Some(row) = found.next().await.map_err(fail)? {
                    let name: String = row.get(0).map_err(fail)?;
                    let sql: String = row.get(1).map_err(fail)?;
                    triggers.push((name, sql));
                }
                drop(found);
                for (name, _) in &triggers {
                    conn.execute(&format!("DROP TRIGGER {}", quote(name)), ())
                        .await
                        .map_err(fail)?;
                }
            }
            conn.execute(&format!("DELETE FROM {}", quote(table)), ())
                .await
                .map_err(fail)?;
            for row in rows {
                if row.len() != columns.len() {
                    return Err(WenlanError::Validation(format!(
                        "rebuild {table}: a row has {} values for {} columns",
                        row.len(),
                        columns.len()
                    )));
                }
                conn.execute(&insert, row.clone()).await.map_err(fail)?;
            }
            for (_, sql) in &triggers {
                conn.execute(sql, ()).await.map_err(fail)?;
            }
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                conn.execute("COMMIT", ()).await.map_err(fail)?;
                Ok(())
            }
            Err(error) => {
                let _ = conn.execute("ROLLBACK", ()).await;
                Err(error)
            }
        }
    }

    /// Sets `set` on the rows of `table` whose `key` columns equal the given
    /// values; returns how many rows changed.
    pub(crate) async fn rebuild_update_rows(
        &self,
        table: &str,
        key: &[(String, libsql::Value)],
        set: &[(String, libsql::Value)],
    ) -> Result<u64, WenlanError> {
        if !TABLE_CLASSES.iter().any(|(name, _)| *name == table) {
            return Err(WenlanError::Validation(format!(
                "rebuild cannot write table {table:?}"
            )));
        }
        if set.is_empty() || key.is_empty() {
            return Ok(0);
        }
        let quote = |name: &str| format!("\"{}\"", name.replace('"', "\"\""));
        let assignments = set
            .iter()
            .enumerate()
            .map(|(i, (column, _))| format!("{} = ?{}", quote(column), i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let matches = key
            .iter()
            .enumerate()
            .map(|(i, (column, _))| format!("{} IS ?{}", quote(column), set.len() + i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let params: Vec<libsql::Value> = set
            .iter()
            .chain(key.iter())
            .map(|(_, value)| value.clone())
            .collect();
        let conn = self.conn.lock().await;
        conn.execute(
            &format!("UPDATE {} SET {assignments} WHERE {matches}", quote(table)),
            params,
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("rebuild update {table}: {e}")))
    }

    /// Whether the database has no page and no memory: the rebuild writes
    /// only into a database it may wipe.
    pub(crate) async fn rebuild_target_is_empty(&self) -> Result<bool, WenlanError> {
        let conn = self.conn.lock().await;
        let fail = |e: libsql::Error| WenlanError::VectorDb(format!("rebuild check target: {e}"));
        let mut rows = conn
            .query(
                "SELECT (SELECT COUNT(*) FROM pages) + (SELECT COUNT(*) FROM memories)",
                (),
            )
            .await
            .map_err(fail)?;
        let count: i64 = match rows.next().await.map_err(fail)? {
            Some(row) => row.get(0).map_err(fail)?,
            None => 0,
        };
        Ok(count == 0)
    }

    /// Turns foreign key enforcement on or off for this database. The
    /// rebuild writes tables one at a time, so a row can name one that is
    /// written later; [`Self::rebuild_foreign_key_problems`] checks them all
    /// at the end.
    pub(crate) async fn rebuild_foreign_keys(&self, on: bool) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        conn.execute(
            if on {
                "PRAGMA foreign_keys = ON"
            } else {
                "PRAGMA foreign_keys = OFF"
            },
            (),
        )
        .await
        .map_err(|e| WenlanError::VectorDb(format!("rebuild foreign keys: {e}")))?;
        Ok(())
    }

    /// Every row whose foreign key names a row that is not there, as
    /// `table rowid → parent`, from `PRAGMA foreign_key_check`.
    pub(crate) async fn rebuild_foreign_key_problems(&self) -> Result<Vec<String>, WenlanError> {
        let conn = self.conn.lock().await;
        let fail =
            |e: libsql::Error| WenlanError::VectorDb(format!("rebuild foreign key check: {e}"));
        let mut rows = conn
            .query("PRAGMA foreign_key_check", ())
            .await
            .map_err(fail)?;
        let mut problems = Vec::new();
        while let Some(row) = rows.next().await.map_err(fail)? {
            let table: String = row.get(0).map_err(fail)?;
            let rowid: Option<i64> = row.get(1).ok();
            let parent: String = row.get(2).map_err(fail)?;
            problems.push(format!(
                "{FOREIGN_KEY}{table} -> {parent} (row {})",
                rowid.map_or_else(|| "?".to_string(), |id| id.to_string())
            ));
        }
        Ok(problems)
    }
}
