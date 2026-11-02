// SPDX-License-Identifier: Apache-2.0
//! Briefs as files: `backstage/briefs/<space>-<space-id6>.md` (spec
//! section 3), one per space that has a Brief.
//!
//! The file's `id` is the space id, since a space has one Brief. The summary,
//! the `## Active` and `## Backlog` lists in their order, each item's id,
//! added time, version and gate, the Brief's version and its last handoff
//! are kept. When each item was last changed, and the Brief's past
//! versions, are history (`brief_history`, part 3).

use std::collections::HashMap;

use super::memories::{check, utc};
use super::pages::Paths;
use super::{ExportDirs, LeftOut, Manifest};
use crate::db::export_reads::ExportBriefItemRow;
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::format::brief::{self, BriefFile, BriefItem};
use crate::store::format::names::{collision_stem, file_stem};

pub async fn export_briefs(
    db: &MemoryDB,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<usize, WenlanError> {
    let (briefs, items) = db.export_briefs().await?;
    let mut by_space: HashMap<&str, Vec<&ExportBriefItemRow>> = HashMap::new();
    for item in &items {
        by_space
            .entry(item.space_id.as_str())
            .or_default()
            .push(item);
    }
    let mut paths = Paths::default();
    let mut written = 0;
    for row in &briefs {
        let space = row.space_name.clone().unwrap_or_else(|| {
            manifest.left_out.push(LeftOut {
                id: row.space_id.clone(),
                field: "space".to_string(),
                why: "the Brief's space has no row; its id stands in for the name".to_string(),
            });
            row.space_id.clone()
        });
        let mut file = BriefFile {
            id: row.space_id.clone(),
            space: space.clone(),
            version: row.version,
            other: serde_yaml::Mapping::new(),
            summary: row.summary.clone(),
            active: Vec::new(),
            backlog: Vec::new(),
        };
        if let Some(at) = row.last_handoff_at {
            file.other.insert("last_handoff".into(), utc(at).into());
        }
        file.other
            .insert("updated".into(), utc(row.updated_at).into());
        for item in by_space.remove(row.space_id.as_str()).unwrap_or_default() {
            let line = BriefItem {
                id: item.id.clone(),
                text: item.text.clone(),
                added: item.added_at,
                version: item.version,
                gate: item.gate.clone(),
            };
            match item.state.as_str() {
                "active" => file.active.push(line),
                _ => file.backlog.push(line),
            }
        }
        let stem = collision_stem(&file_stem(&space), &row.space_id);
        let path = paths.claim("briefs", &stem, &row.space_id);
        let text = brief::render(&file);
        check(
            manifest,
            &row.space_id,
            brief::parse(&text).ok() == Some(file),
        );
        manifest.files.push(dirs.write(
            &format!("backstage/{path}"),
            &row.space_id,
            "brief",
            &text,
        )?);
        written += 1;
    }
    let mut orphans: Vec<_> = by_space.into_values().flatten().collect();
    orphans.sort_by(|a, b| a.id.cmp(&b.id));
    for item in orphans {
        manifest.left_out.push(LeftOut {
            id: item.id.clone(),
            field: "brief_item".to_string(),
            why: format!("an item of space {} that has no Brief", item.space_id),
        });
    }
    Ok(written)
}
