// SPDX-License-Identifier: Apache-2.0
//! Briefs read back into `briefs` and `brief_items` rows: the inverse of
//! `store::export::briefs`. When each item last changed is history; its
//! state seed sets it after this part.

use std::path::Path;

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::export::Manifest;
use crate::store::format::brief;

/// Writes every Brief file the manifest lists; returns `(briefs, items)`.
pub async fn rebuild_briefs(
    from: &Path,
    manifest: &Manifest,
    into: &MemoryDB,
    problems: &mut Vec<String>,
) -> Result<(usize, usize), WenlanError> {
    let mut briefs = Vec::new();
    let mut items = Vec::new();
    for file in manifest.files.iter().filter(|f| f.file_type == "brief") {
        let text = std::fs::read_to_string(from.join(&file.path))?;
        let mut parsed = match brief::parse(&text) {
            Ok(parsed) => parsed,
            Err(error) => {
                problems.push(format!("{}: {error}", file.path));
                continue;
            }
        };
        let mut time = |key: &str| {
            let text = super::pages::take_text(&mut parsed.other, key)?;
            let at = super::pages::seconds_of(&text);
            if at.is_none() {
                problems.push(format!("{}: {key} {text:?} is not a time", file.path));
            }
            at
        };
        let handoff = time("last_handoff");
        let updated = time("updated").unwrap_or(0);
        briefs.push(vec![
            libsql::Value::Text(parsed.id.clone()),
            libsql::Value::Text(parsed.summary.clone()),
            handoff.map_or(libsql::Value::Null, libsql::Value::Integer),
            libsql::Value::Integer(parsed.version),
            libsql::Value::Integer(updated),
        ]);
        for (state, list) in [("active", &parsed.active), ("backlog", &parsed.backlog)] {
            for item in list {
                items.push(vec![
                    libsql::Value::Text(item.id.clone()),
                    libsql::Value::Text(parsed.id.clone()),
                    libsql::Value::Text(item.text.clone()),
                    libsql::Value::Text(state.to_string()),
                    libsql::Value::Integer(item.added),
                    item.gate
                        .clone()
                        .map_or(libsql::Value::Null, libsql::Value::Text),
                    libsql::Value::Integer(item.version),
                    // History; NOT NULL until its state seed sets it.
                    libsql::Value::Integer(item.added),
                ]);
            }
        }
    }
    let names = |columns: &[&str]| columns.iter().map(|c| c.to_string()).collect::<Vec<_>>();
    let counts = (briefs.len(), items.len());
    into.rebuild_replace_table(
        "briefs",
        &names(&[
            "space_id",
            "last_session_summary",
            "last_handoff_at",
            "version",
            "updated_at",
        ]),
        &briefs,
    )
    .await?;
    into.rebuild_replace_table(
        "brief_items",
        &names(&[
            "id",
            "space_id",
            "text",
            "state",
            "added_at",
            "gate",
            "version",
            "updated_at",
        ]),
        &items,
    )
    .await?;
    Ok(counts)
}
