// SPDX-License-Identifier: Apache-2.0
//! Typed JSON values in `app_metadata`, read and written on the caller's
//! connection so they join the caller's transaction. Each caller passes its own
//! `sql` error mapper, which keeps its error text.
use super::commit_or_rollback;
use crate::WenlanError;
use serde::{de::DeserializeOwned, Serialize};

pub(super) type SqlError = fn(libsql::Error) -> WenlanError;

/// The raw stored string for `key`, if any.
pub(super) async fn load_raw(
    conn: &libsql::Connection,
    key: &str,
    sql: SqlError,
) -> Result<Option<String>, WenlanError> {
    let mut rows = conn
        .query("SELECT value FROM app_metadata WHERE key=?1", [key])
        .await
        .map_err(sql)?;
    rows.next()
        .await
        .map_err(sql)?
        .map(|row| row.get::<String>(0).map_err(sql))
        .transpose()
}

pub(super) async fn load<T: DeserializeOwned>(
    conn: &libsql::Connection,
    key: &str,
    sql: SqlError,
) -> Result<Option<T>, WenlanError> {
    load_raw(conn, key, sql)
        .await?
        .map(|raw| -> Result<T, WenlanError> { Ok(serde_json::from_str(&raw)?) })
        .transpose()
}

pub(super) async fn save(
    conn: &libsql::Connection,
    key: &str,
    value: &impl Serialize,
    sql: SqlError,
) -> Result<(), WenlanError> {
    conn.execute(
        "INSERT INTO app_metadata(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        libsql::params![key, serde_json::to_string(value)?],
    )
    .await
    .map_err(sql)?;
    Ok(())
}

/// Commit after `result` succeeded, or roll back and return its error.
pub(super) async fn finish<T>(
    conn: &libsql::Connection,
    result: Result<T, WenlanError>,
    sql: SqlError,
) -> Result<T, WenlanError> {
    match result {
        Ok(value) => {
            commit_or_rollback(conn).await.map_err(sql)?;
            Ok(value)
        }
        Err(error) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}
