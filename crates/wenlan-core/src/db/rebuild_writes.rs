// SPDX-License-Identifier: Apache-2.0
//! Writes for the rebuild (`store::rebuild`): rows read back from a lossless
//! export go into a fresh database as they were stored. Only for a database
//! the rebuild just created; never the person's.

use super::table_classes::TABLE_CLASSES;
use super::MemoryDB;
use crate::error::WenlanError;

impl MemoryDB {
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
}
