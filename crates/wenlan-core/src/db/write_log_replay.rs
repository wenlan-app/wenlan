// SPDX-License-Identifier: Apache-2.0
//! Applies write log lines ([`crate::store::write_log`]) to a database: after
//! a restore, the lines the file copy does not hold yet; at start, the last
//! transaction when the database lost it (the process stopped between the
//! log line and the database commit).
//!
//! The log holds every row a write changed, the rows its triggers and
//! foreign key actions changed included, so a replay writes the rows as
//! logged with the triggers set aside and foreign keys off. The search
//! indexes the triggers keep are rebuilt after. Each change is applied by
//! its key, so applying a line twice, or lines the database already holds,
//! ends in the same rows.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use libsql::{Connection, Value};

use super::table_classes::{TableClass, TABLE_CLASSES};
use super::MemoryDB;
use crate::error::WenlanError;
use crate::store::write_log::{self, Change, LogPosition, RowOp, Shape, Tx, WriteLog};

/// What a replay did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replayed {
    pub txs: usize,
    pub changes: usize,
    pub problems: Vec<String>,
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn fail(context: &'static str) -> impl Fn(libsql::Error) -> WenlanError {
    move |error| WenlanError::VectorDb(format!("write log {context}: {error}"))
}

/// A table's columns as `PRAGMA table_xinfo` lists them: name, declared
/// type, primary key position, hidden.
async fn table_columns(
    conn: &Connection,
    table: &str,
) -> Result<Vec<(String, String, i64, i64)>, WenlanError> {
    let mut rows = conn
        .query(&format!("PRAGMA table_xinfo({})", quote(table)), ())
        .await
        .map_err(fail("read columns"))?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().await.map_err(fail("read columns"))? {
        let name: String = row.get(1).map_err(fail("read columns"))?;
        let kind: String = row
            .get::<Option<String>>(2)
            .ok()
            .flatten()
            .unwrap_or_default();
        let pk: i64 = row.get(5).map_err(fail("read columns"))?;
        let hidden: i64 = row.get(6).map_err(fail("read columns"))?;
        columns.push((name, kind, pk, hidden));
    }
    Ok(columns)
}

fn is_vector(name: &str, kind: &str) -> bool {
    let kind = kind.to_ascii_uppercase();
    name == "embedding" || kind.contains("F32_BLOB") || kind.contains("FLOAT32")
}

/// The shapes of the tables the log holds, as the database has them now.
async fn shapes(conn: &Connection) -> Result<HashMap<String, Shape>, WenlanError> {
    let mut shapes = HashMap::new();
    for (table, class) in TABLE_CLASSES {
        if !matches!(class, TableClass::File | TableClass::History) {
            continue;
        }
        let columns = table_columns(conn, table).await?;
        if columns.is_empty() {
            continue;
        }
        let mut key: Vec<(i64, usize)> = columns
            .iter()
            .enumerate()
            .filter(|(_, (_, _, pk, _))| *pk > 0)
            .map(|(i, (_, _, pk, _))| (*pk, i))
            .collect();
        key.sort();
        shapes.insert(
            table.to_string(),
            Shape {
                columns: columns.iter().map(|(name, ..)| name.clone()).collect(),
                skip: columns
                    .iter()
                    .map(|(name, kind, _, hidden)| *hidden != 0 || is_vector(name, kind))
                    .collect(),
                key: key.into_iter().map(|(_, i)| i).collect(),
            },
        );
    }
    Ok(shapes)
}

struct Columns {
    /// Every column, hidden ones included, in order (for `#n` names).
    all: Vec<String>,
    writable: BTreeSet<String>,
    has_key: bool,
}

/// Column names as this database has them: `#n` (a line logged by
/// position) is the n-th column; columns it does not have are dropped.
fn resolve(pairs: &[(String, Value)], columns: &Columns) -> Vec<(String, Value)> {
    pairs
        .iter()
        .filter_map(|(name, value)| {
            let name = match name.strip_prefix('#').and_then(|n| n.parse::<usize>().ok()) {
                Some(i) => columns.all.get(i)?.clone(),
                None => name.clone(),
            };
            columns
                .writable
                .contains(&name)
                .then(|| (name, value.clone()))
        })
        .collect()
}

fn matches_sql(pairs: &[(String, Value)], first: usize) -> String {
    pairs
        .iter()
        .enumerate()
        .map(|(i, (name, _))| format!("{} IS ?{}", quote(name), first + i))
        .collect::<Vec<_>>()
        .join(" AND ")
}

async fn apply(conn: &Connection, change: &Change, columns: &Columns) -> Result<(), WenlanError> {
    let table = quote(&change.table);
    let key = resolve(&change.key, columns);
    let row = resolve(&change.row, columns);
    let run = |sql: String, params: Vec<Value>| async move {
        conn.execute(&sql, params)
            .await
            .map_err(|error| WenlanError::VectorDb(format!("{sql}: {error}")))
    };
    match change.op {
        RowOp::Delete => {
            if key.is_empty() {
                return Ok(());
            }
            let params = key.iter().map(|(_, v)| v.clone()).collect();
            let sql = if columns.has_key {
                format!("DELETE FROM {table} WHERE {}", matches_sql(&key, 1))
            } else {
                format!(
                    "DELETE FROM {table} WHERE rowid = \
                     (SELECT rowid FROM {table} WHERE {} LIMIT 1)",
                    matches_sql(&key, 1)
                )
            };
            run(sql, params).await?;
        }
        RowOp::Insert | RowOp::Update => {
            if row.is_empty() {
                return Ok(());
            }
            if change.op == RowOp::Update && !key.is_empty() {
                let set = row
                    .iter()
                    .enumerate()
                    .map(|(i, (name, _))| format!("{} = ?{}", quote(name), i + 1))
                    .collect::<Vec<_>>()
                    .join(", ");
                let params = row
                    .iter()
                    .chain(key.iter())
                    .map(|(_, v)| v.clone())
                    .collect();
                let sql = format!(
                    "UPDATE {table} SET {set} WHERE {}",
                    matches_sql(&key, row.len() + 1)
                );
                if run(sql, params).await? > 0 {
                    return Ok(());
                }
            }
            let names = row
                .iter()
                .map(|(name, _)| quote(name))
                .collect::<Vec<_>>()
                .join(", ");
            let marks = (1..=row.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let values: Vec<Value> = row.iter().map(|(_, v)| v.clone()).collect();
            if columns.has_key {
                let set = row
                    .iter()
                    .map(|(name, _)| format!("{0} = excluded.{0}", quote(name)))
                    .collect::<Vec<_>>()
                    .join(", ");
                run(
                    format!(
                        "INSERT INTO {table} ({names}) VALUES ({marks}) \
                         ON CONFLICT DO UPDATE SET {set}"
                    ),
                    values,
                )
                .await?;
            } else {
                let params = values.iter().chain(values.iter()).cloned().collect();
                run(
                    format!(
                        "INSERT INTO {table} ({names}) SELECT {marks} WHERE NOT EXISTS \
                         (SELECT 1 FROM {table} WHERE {})",
                        matches_sql(&row, row.len() + 1)
                    ),
                    params,
                )
                .await?;
            }
        }
    }
    Ok(())
}

async fn columns_of(conn: &Connection, table: &str) -> Result<Option<Columns>, WenlanError> {
    let found = table_columns(conn, table).await?;
    if found.is_empty() {
        return Ok(None);
    }
    Ok(Some(Columns {
        all: found.iter().map(|(name, ..)| name.clone()).collect(),
        writable: found
            .iter()
            .filter(|(_, _, _, hidden)| *hidden == 0)
            .map(|(name, ..)| name.clone())
            .collect(),
        has_key: found.iter().any(|(_, _, pk, _)| *pk > 0),
    }))
}

/// Search indexes kept by triggers on a logged table.
const INDEXES: &[(&str, &str)] = &[("memories", "memories_fts"), ("pages", "pages_fts")];

/// Applies `txs` to `conn` in one transaction. A change that cannot be
/// applied is reported and the rest go on.
async fn replay_changes(conn: &Connection, txs: &[Tx]) -> Result<Replayed, WenlanError> {
    let mut replayed = Replayed::default();
    if txs.iter().all(|tx| tx.changes.is_empty()) {
        replayed.txs = txs.len();
        return Ok(replayed);
    }
    conn.execute("PRAGMA foreign_keys = OFF", ())
        .await
        .map_err(fail("foreign keys off"))?;
    let result = replay_in_transaction(conn, txs, &mut replayed).await;
    let back_on = conn
        .execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(fail("foreign keys on"));
    result?;
    back_on?;
    Ok(replayed)
}

async fn replay_in_transaction(
    conn: &Connection,
    txs: &[Tx],
    replayed: &mut Replayed,
) -> Result<(), WenlanError> {
    conn.execute("BEGIN IMMEDIATE", ())
        .await
        .map_err(fail("begin"))?;
    let result = async {
        let mut triggers = Vec::new();
        {
            let mut rows = conn
                .query(
                    "SELECT name, sql FROM sqlite_master \
                     WHERE type = 'trigger' AND sql IS NOT NULL ORDER BY name",
                    (),
                )
                .await
                .map_err(fail("read triggers"))?;
            while let Some(row) = rows.next().await.map_err(fail("read triggers"))? {
                let name: String = row.get(0).map_err(fail("read triggers"))?;
                let sql: String = row.get(1).map_err(fail("read triggers"))?;
                triggers.push((name, sql));
            }
        }
        for (name, _) in &triggers {
            conn.execute(&format!("DROP TRIGGER {}", quote(name)), ())
                .await
                .map_err(fail("set triggers aside"))?;
        }
        let mut known: HashMap<String, Option<Columns>> = HashMap::new();
        let mut touched = BTreeSet::new();
        for tx in txs {
            for change in &tx.changes {
                if !known.contains_key(&change.table) {
                    let columns = columns_of(conn, &change.table).await?;
                    known.insert(change.table.clone(), columns);
                }
                let Some(Some(columns)) = known.get(&change.table) else {
                    replayed
                        .problems
                        .push(format!("write log tx {}: no table {}", tx.tx, change.table));
                    continue;
                };
                match apply(conn, change, columns).await {
                    Ok(()) => {
                        replayed.changes += 1;
                        touched.insert(change.table.clone());
                    }
                    Err(error) => replayed
                        .problems
                        .push(format!("write log tx {}: {error}", tx.tx)),
                }
            }
            replayed.txs += 1;
        }
        for (_, sql) in &triggers {
            conn.execute(sql, ())
                .await
                .map_err(fail("put triggers back"))?;
        }
        for (table, index) in INDEXES {
            if touched.contains(*table) {
                conn.execute(
                    &format!("INSERT INTO {index}({index}) VALUES ('rebuild')"),
                    (),
                )
                .await
                .map_err(fail("rebuild search index"))?;
            }
        }
        Ok::<(), WenlanError>(())
    }
    .await;
    match result {
        Ok(()) => conn
            .execute("COMMIT", ())
            .await
            .map(|_| ())
            .map_err(fail("commit")),
        Err(error) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(error)
        }
    }
}

/// Whether the database holds what `change` left: the row as logged, or,
/// for a delete, no row with its key.
async fn holds(conn: &Connection, change: &Change) -> Result<bool, WenlanError> {
    let Some(columns) = columns_of(conn, &change.table).await? else {
        return Ok(true);
    };
    let table = quote(&change.table);
    let (pairs, gone) = match change.op {
        RowOp::Delete => (resolve(&change.key, &columns), true),
        RowOp::Insert | RowOp::Update => (resolve(&change.row, &columns), false),
    };
    if pairs.is_empty() {
        return Ok(true);
    }
    let params: Vec<Value> = pairs.iter().map(|(_, v)| v.clone()).collect();
    let mut rows = conn
        .query(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM {table} WHERE {})",
                matches_sql(&pairs, 1)
            ),
            params,
        )
        .await
        .map_err(fail("check last write"))?;
    let found: i64 = match rows.next().await.map_err(fail("check last write"))? {
        Some(row) => row.get(0).map_err(fail("check last write"))?,
        None => 0,
    };
    Ok((found == 1) != gone)
}

/// At start, before anything else opens `db_file`: when the last
/// transaction in the log at `log_dir` is not in the database (the process
/// stopped after the log line and before the database commit), applies
/// it. Only a database at this build's schema that the same build wrote
/// is touched. Returns what it did, when it did something.
pub async fn settle(db_file: &Path, log_dir: &Path) -> Result<Option<Replayed>, WenlanError> {
    if !db_file.is_file() {
        return Ok(None);
    }
    let Some(last) = write_log::last_committed(log_dir)? else {
        return Ok(None);
    };
    if last.schema != i64::from(super::SCHEMA_VERSION) {
        return Ok(None);
    }
    let Some(change) = last.changes.last() else {
        return Ok(None);
    };
    let path = db_file.to_str().ok_or_else(|| {
        WenlanError::Validation(format!("not a UTF-8 path: {}", db_file.display()))
    })?;
    let database = libsql::Builder::new_local(path)
        .build()
        .await
        .map_err(fail("open"))?;
    let conn = database.connect().map_err(fail("connect"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(fail("busy timeout"))?;
    let version: i64 = {
        let mut rows = conn
            .query("PRAGMA user_version", ())
            .await
            .map_err(fail("schema"))?;
        match rows.next().await.map_err(fail("schema"))? {
            Some(row) => row.get(0).map_err(fail("schema"))?,
            None => 0,
        }
    };
    if version != last.schema || holds(&conn, change).await? {
        return Ok(None);
    }
    let replayed = replay_changes(&conn, std::slice::from_ref(&last)).await?;
    Ok(Some(replayed))
}

impl MemoryDB {
    /// Arms `log` with this database's tables: from now on every change to
    /// them is logged before it commits.
    pub async fn arm_write_log(&self, log: &WriteLog) -> Result<(), WenlanError> {
        let conn = self.conn.lock().await;
        let shapes = shapes(&conn).await?;
        log.arm(shapes, i64::from(super::SCHEMA_VERSION));
        Ok(())
    }

    /// The position of `log` with no write running: every line up to it is
    /// in this database.
    pub async fn write_log_position(&self, log: &WriteLog) -> LogPosition {
        let _writer = self.conn.lock().await;
        log.position()
    }

    /// Applies `txs` (see [`replay_changes`]). For a database a restore just built.
    pub async fn replay_write_log(&self, txs: &[Tx]) -> Result<Replayed, WenlanError> {
        let conn = self.conn.lock().await;
        replay_changes(&conn, txs).await
    }
}

#[cfg(test)]
#[path = "write_log_replay_test.rs"]
mod write_log_replay_test;
