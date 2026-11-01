// SPDX-License-Identifier: Apache-2.0
//! Pages, entities and archived pages as files.
//!
//! - A page keeps the folder it is in today (`placement`, from the pages
//!   folder's own record); a page that has no file yet goes at the top.
//! - Its body is `pages.content` byte for byte. Today's `[x](concept_…)`
//!   links stay as written; turning them into `[[stem]]` is a later edit
//!   with its own history entry, not part of a lossless copy.
//! - The sources block lists one line per citation marker (`[N]` in the
//!   body) and one `basis:` line per page this page relies on (state `ok`
//!   or `pending`), at the version that was confirmed. Every basis row with
//!   its state and confirmation is also in `records/page_basis.json`. Review
//!   state, version numbers and the rest go to history (spec section 4).
//! - The page's `summary`, which the person can edit, is a frontmatter key.
//! - An entity's shadow page becomes `backstage/entities/<name>-<id6>.md`,
//!   its description the body, with `status` (when not active, so an
//!   archived entity stays archived), `created`, `modified`, `first_seen`
//!   and `last_seen`.
//! - An archived page goes to `backstage/archive/`, keeping its folder.

use std::collections::{BTreeMap, HashMap, HashSet};

use wenlan_types::pages::PageCitation;

use super::memories::{check, utc};
use super::{ExportDirs, LeftOut, Manifest};
use crate::db::export_reads::ExportPageRow;
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::store::format::entity::{self, EntityFile};
use crate::store::format::names::{collision_stem, file_stem, same_file_name};
use crate::store::format::page::{self, Basis, Citation, Locator, PageFile, PageKind};

/// Basis states that still point at a page; the rest are only in records.
const LIVE_BASIS_STATES: &[&str] = &["ok", "pending"];

/// What one export of pages wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PagesExported {
    pub pages: usize,
    pub archived: usize,
    pub entities: usize,
    /// Each entity id's file stem, for `[[stem]]` links to it.
    pub entity_stems: HashMap<String, String>,
    /// Each page id's file, relative to the export root.
    pub page_paths: HashMap<String, String>,
    /// Every page row's id, entities included.
    pub row_ids: HashSet<String>,
}

/// Writes every page row. `placement` maps a page id to its file's path
/// inside today's pages folder (forward slashes), for the pages that have
/// one.
pub async fn export_pages(
    db: &MemoryDB,
    placement: &HashMap<String, String>,
    dirs: &ExportDirs,
    manifest: &mut Manifest,
) -> Result<PagesExported, WenlanError> {
    let rows = db.export_page_rows().await?;
    let mut done = PagesExported {
        row_ids: rows.iter().map(|row| row.id.clone()).collect(),
        ..PagesExported::default()
    };

    let (entities, pages): (Vec<&ExportPageRow>, Vec<&ExportPageRow>) =
        rows.iter().partition(|row| is_entity(row));
    let mut visible = Paths::default();
    let mut archive = Paths::default();
    let mut paths: HashMap<&str, (bool, String)> = HashMap::new();
    // Pages that have a file keep its path; the rest are placed after them.
    let (placed, unplaced): (Vec<&ExportPageRow>, Vec<&ExportPageRow>) = pages
        .iter()
        .copied()
        .partition(|row| placement_of(placement, &row.id).is_some());
    for row in placed.iter().chain(unplaced.iter()) {
        let archived = row.status == "archived";
        let (folder, stem) = match placement_of(placement, &row.id) {
            Some((folder, stem)) => (folder, stem),
            None => (String::new(), file_stem(&row.title)),
        };
        let taken = if archived { &mut archive } else { &mut visible };
        let path = taken.claim(&folder, &stem, &row.id);
        paths.insert(&row.id, (archived, path));
    }
    let stems: HashMap<&str, String> = paths
        .iter()
        .map(|(id, (_, path))| (*id, stem_of(path)))
        .collect();

    let memory_ids: Vec<String> = pages
        .iter()
        .flat_map(|row| citations_of(row).unwrap_or_default())
        .filter(|citation| citation.source_kind == "memory")
        .map(|citation| citation.locator)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let memory_texts = db.export_memory_texts(&memory_ids).await?;

    for row in &pages {
        let (archived, path) = &paths[row.id.as_str()];
        let mut file = PageFile::new(&row.title, &row.content);
        file.id = Some(row.id.clone());
        // `pages.kind` cannot be trusted to read yet (drift_guard), so
        // every page is `type: page`; kind and creation kind go to history.
        file.kind = PageKind::Page;
        file.tags = row.space.iter().cloned().collect();
        file.created = Some(row.created_at.clone());
        file.modified = Some(row.last_modified.clone());
        if let Some(summary) = &row.summary {
            file.other.insert("summary".into(), summary.as_str().into());
        }
        entity_keys(&mut file.other, row);
        // Archived pages are told by their folder; any other status is kept.
        if !matches!(row.status.as_str(), "active" | "archived") {
            file.other
                .insert("status".into(), row.status.as_str().into());
        }
        file.citations = citation_lines(row, &memory_texts, &stems, manifest);
        // A basis whose page is gone is not a line; every basis row, state
        // and confirmation is in `records/page_basis.json`.
        for basis in db.list_page_basis(&row.id).await? {
            if !LIVE_BASIS_STATES.contains(&basis.state.as_str()) {
                continue;
            }
            let label = stems
                .get(basis.basis_page_id.as_str())
                .cloned()
                .unwrap_or_else(|| one_line(&basis.title));
            file.basis.push(Basis {
                label: format!("[[{label}]]"),
                locator: Locator::Wenlan {
                    id: basis.basis_page_id,
                    version: basis.confirmed_version,
                },
            });
        }
        let (relative, file_type) = if *archived {
            done.archived += 1;
            (format!("backstage/archive/{path}"), "archived_page")
        } else {
            done.pages += 1;
            (format!("visible/{path}"), "page")
        };
        let text = page::render(&file);
        check(manifest, &row.id, page::parse(&text, "").ok() == Some(file));
        done.page_paths.insert(row.id.clone(), relative.clone());
        manifest
            .files
            .push(dirs.write(&relative, &row.id, file_type, &text)?);
    }

    let mut entity_paths = Paths::default();
    for row in entities {
        unwritten(
            manifest,
            &row.id,
            &[
                ("summary", row.summary.is_some()),
                ("citations", citations_of(row).is_none_or(|c| !c.is_empty())),
            ],
        );
        let entity_id = match &row.entity_id {
            Some(id) => id.clone(),
            None => {
                manifest.left_out.push(LeftOut {
                    id: row.id.clone(),
                    field: "entity_id".to_string(),
                    why: "an entity page with no entity id; the page id stands in".to_string(),
                });
                row.id.clone()
            }
        };
        let mut file = EntityFile::new(&entity_id, &row.title);
        file.page = Some(row.id.clone());
        file.entity_type = row.entity_type.clone();
        file.aliases = aliases_of(row, manifest);
        file.confirmed = row.entity_confirmed;
        file.established_by = row.established_by.clone();
        file.space = row.space.clone();
        file.description = row.content.clone();
        // Wenlan's keys the entity file has no field for.
        if row.status != "active" {
            file.other
                .insert("status".into(), row.status.as_str().into());
        }
        file.other
            .insert("created".into(), row.created_at.as_str().into());
        file.other
            .insert("modified".into(), row.last_modified.as_str().into());
        if let Some(at) = row.entity_created_at {
            file.other.insert("first_seen".into(), utc(at).into());
        }
        if let Some(at) = row.entity_updated_at {
            file.other.insert("last_seen".into(), utc(at).into());
        }
        let stem = collision_stem(&file_stem(&row.title), &entity_id);
        let path = entity_paths.claim("", &stem, &entity_id);
        done.entity_stems.insert(entity_id.clone(), stem_of(&path));
        let text = entity::render(&file);
        check(
            manifest,
            &entity_id,
            entity::parse(&text).ok() == Some(file),
        );
        manifest.files.push(dirs.write(
            &format!("backstage/entities/{path}"),
            &entity_id,
            "entity",
            &text,
        )?);
        done.entities += 1;
    }
    Ok(done)
}

/// Lists each column that holds a value the file for this kind of row has
/// no place for: entity columns on a page, page columns on an entity.
fn unwritten(manifest: &mut Manifest, id: &str, columns: &[(&str, bool)]) {
    for (column, set) in columns {
        if *set {
            manifest.left_out.push(LeftOut {
                id: id.to_string(),
                field: column.to_string(),
                why: "a value this kind of file has no place for".to_string(),
            });
        }
    }
}

/// The entity a page is about (a distilled page keeps its cluster's
/// entity), and any other entity column a page row carries, as stored, so
/// the rebuild restores them exactly.
fn entity_keys(other: &mut serde_yaml::Mapping, row: &ExportPageRow) {
    let text = [
        ("entity_id", &row.entity_id),
        ("entity_type", &row.entity_type),
        ("aliases", &row.aliases),
        ("established_by", &row.established_by),
    ];
    for (key, value) in text {
        if let Some(value) = value {
            other.insert(key.into(), value.as_str().into());
        }
    }
    if row.entity_confirmed {
        other.insert("entity_confirmed".into(), true.into());
    }
    for (key, value) in [
        ("entity_created_at", row.entity_created_at),
        ("entity_updated_at", row.entity_updated_at),
    ] {
        if let Some(value) = value {
            other.insert(key.into(), value.into());
        }
    }
}

/// A shadow page behind an entity.
fn is_entity(row: &ExportPageRow) -> bool {
    row.kind == "entity" || row.creation_kind == "entity"
}

/// `(folder, stem)` of a page's file today, when its recorded path is one
/// the export can keep: a relative `.md` path of plain names. A path under
/// today's in-folder `archive/` keeps the rest of its path.
fn placement_of(placement: &HashMap<String, String>, id: &str) -> Option<(String, String)> {
    let path = placement.get(id)?;
    let path = path.strip_prefix("archive/").unwrap_or(path);
    let name = path.strip_suffix(".md")?;
    let (folder, stem) = match name.rsplit_once('/') {
        Some((folder, stem)) => (folder.to_string(), stem.to_string()),
        None => (String::new(), name.to_string()),
    };
    let plain = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.contains('\\')
            && !part.starts_with('.')
    };
    let fine = plain(&stem)
        && (folder.is_empty() || folder.split('/').all(plain))
        && file_stem(&stem) == stem;
    fine.then_some((folder, stem))
}

/// Paths given out so far, so no two files share a name the way a
/// case-insensitive file system compares them.
#[derive(Default)]
pub(super) struct Paths {
    taken: HashMap<String, Vec<String>>,
}

impl Paths {
    /// `folder/stem.md`, or `folder/stem-<id6>.md` when that name is taken,
    /// then `-2`, `-3`, … until a name is free.
    pub(super) fn claim(&mut self, folder: &str, stem: &str, id: &str) -> String {
        self.claim_as(folder, stem, id, "md")
    }

    /// [`Self::claim`] with another extension.
    pub(super) fn claim_as(
        &mut self,
        folder: &str,
        stem: &str,
        id: &str,
        extension: &str,
    ) -> String {
        let key = folder.to_lowercase();
        let names = self.taken.entry(key).or_default();
        let clash = |stem: &str| names.iter().any(|name| same_file_name(name, stem));
        let mut chosen = stem.to_string();
        if clash(&chosen) {
            let base = collision_stem(stem, id);
            chosen = base.clone();
            let mut n = 2;
            while clash(&chosen) {
                chosen = collision_stem(&base, &format!("{n}"));
                n += 1;
            }
        }
        names.push(chosen.clone());
        if folder.is_empty() {
            format!("{chosen}.{extension}")
        } else {
            format!("{folder}/{chosen}.{extension}")
        }
    }
}

fn stem_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

/// `None` when the stored citations cannot be read.
fn citations_of(row: &ExportPageRow) -> Option<Vec<PageCitation>> {
    let raw = row.citations.as_deref().unwrap_or("").trim();
    if raw.is_empty() {
        return Some(Vec::new());
    }
    serde_json::from_str(raw).ok()
}

/// One line per marker, in marker order, from the first occurrence of each.
fn citation_lines(
    row: &ExportPageRow,
    memory_texts: &HashMap<String, String>,
    stems: &HashMap<&str, String>,
    manifest: &mut Manifest,
) -> Vec<Citation> {
    let Some(mut citations) = citations_of(row) else {
        manifest.left_out.push(LeftOut {
            id: row.id.clone(),
            field: "citations".to_string(),
            why: "the stored citations are not readable".to_string(),
        });
        return Vec::new();
    };
    citations.sort_by_key(|citation| citation.occurrence);
    let mut by_marker: BTreeMap<u32, &PageCitation> = BTreeMap::new();
    for citation in &citations {
        let first = by_marker.entry(citation.marker).or_insert(citation);
        if source_of(first) != source_of(citation) {
            manifest.left_out.push(LeftOut {
                id: row.id.clone(),
                field: format!("citations[occurrence {}]", citation.occurrence),
                why: format!(
                    "marker [{}] cites {} here and {} at its first occurrence",
                    citation.marker,
                    render_source(citation),
                    render_source(first)
                ),
            });
        }
    }
    by_marker
        .into_values()
        .map(|citation| Citation {
            marker: citation.marker,
            label: label_of(citation, memory_texts, stems),
            locator: locator_of(citation),
        })
        .collect()
}

fn source_of(citation: &PageCitation) -> (&str, &str, Option<i64>) {
    (
        citation.source_kind.as_str(),
        citation.locator.as_str(),
        citation.cited_version,
    )
}

fn render_source(citation: &PageCitation) -> String {
    locator_of(citation).render()
}

/// `wenlan:<id>@<version>` for a memory cited at a version, `wenlan:<id>`
/// for one cited before versions were recorded, `<kind>:<locator>` for
/// everything else, so the rebuild gets the kind back.
pub(crate) fn locator_of(citation: &PageCitation) -> Locator {
    match (citation.source_kind.as_str(), citation.cited_version) {
        ("memory", Some(version)) => Locator::Wenlan {
            id: citation.locator.clone(),
            version,
        },
        ("memory", None) => Locator::Other(format!("wenlan:{}", citation.locator)),
        (kind, None) => Locator::Other(format!("{kind}:{}", citation.locator)),
        (kind, Some(version)) => Locator::Other(format!("{kind}:{}@{version}", citation.locator)),
    }
}

/// What a reader sees: the start of the memory's text, the cited page's
/// name, or the locator.
fn label_of(
    citation: &PageCitation,
    memory_texts: &HashMap<String, String>,
    stems: &HashMap<&str, String>,
) -> String {
    if let Some(stem) = stems.get(citation.locator.as_str()) {
        return format!("[[{stem}]]");
    }
    if citation.source_kind == "memory" {
        if let Some(text) = memory_texts.get(&citation.locator) {
            let line = one_line(text);
            if !line.is_empty() {
                let mut short: String = line.chars().take(60).collect();
                if short.len() < line.len() {
                    short.push('…');
                }
                return short;
            }
        }
    }
    citation.locator.clone()
}

/// `text` on one line, so it fits a sources line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn aliases_of(row: &ExportPageRow, manifest: &mut Manifest) -> Vec<String> {
    let Some(raw) = row.aliases.as_deref().filter(|raw| !raw.trim().is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(aliases) => aliases,
        Err(_) => {
            manifest.left_out.push(LeftOut {
                id: row.id.clone(),
                field: "aliases".to_string(),
                why: format!("the stored aliases are not a list of names: {raw}"),
            });
            Vec::new()
        }
    }
}
