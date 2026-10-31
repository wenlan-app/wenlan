// SPDX-License-Identifier: Apache-2.0
//! The memory palace layout file (UX-GRAPH-1..3).
//!
//! `<data root>/layout/palace.json` holds where each point sits, the person's
//! lines, circles and notes, and which AI-added items were accepted. Every
//! write is checked against the version it was based on, so two windows can
//! never overwrite each other silently, and the version before each write is
//! kept under `layout/palace.versions/` so a relayout (or any write) can be
//! reverted. Nothing here moves a point: the app sends positions, and a
//! relayout is a write the person asked for.
//!
//! The rules for AI writes live here, not in the app: an AI tool's new points,
//! lines, circles and notes are always suggestions (`added_by: ai`,
//! `accepted: false`, drawn dashed), and only the person can accept one.

use crate::error::WenlanError;
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use wenlan_types::palace::{
    PalaceAuthor, PalaceLayout, PalaceLayoutFile, PalaceLayoutResponse, PalaceWriteReason,
    PALACE_FORMAT,
};

/// Earlier versions kept for revert.
pub const KEPT_VERSIONS: usize = 20;
/// Size ceilings for one layout.
pub const MAX_ITEMS: usize = 50_000;
pub const MAX_ID_CHARS: usize = 256;
pub const MAX_TEXT_CHARS: usize = 10_000;

/// Who is writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PalaceWriter {
    Person,
    Ai { agent: String },
}

/// Why a write was refused.
#[derive(Debug)]
pub enum PalaceError {
    /// `expected_version` is not the stored version.
    VersionConflict {
        current: u64,
    },
    /// No such earlier version to revert to.
    NoSuchVersion(u64),
    Invalid(String),
    Io(WenlanError),
}

impl From<std::io::Error> for PalaceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(WenlanError::Io(error))
    }
}

impl From<serde_json::Error> for PalaceError {
    fn from(error: serde_json::Error) -> Self {
        Self::Io(WenlanError::Json(error))
    }
}

/// The folder that holds `palace.json`: `<data root>/layout`.
pub fn default_layout_dir() -> PathBuf {
    crate::config::config_root().join("layout")
}

/// One palace file store. The daemon holds one; all writes go through it.
#[derive(Debug, Clone)]
pub struct PalaceStore {
    dir: PathBuf,
}

/// Writes in this process run one at a time, so the version check and the
/// rename are one step.
static WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

impl PalaceStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn file_path(&self) -> PathBuf {
        self.dir.join("palace.json")
    }

    fn versions_dir(&self) -> PathBuf {
        self.dir.join("palace.versions")
    }

    fn version_path(&self, version: u64) -> PathBuf {
        self.versions_dir().join(format!("{version}.json"))
    }

    fn read_file(path: &Path) -> Result<Option<PalaceLayoutFile>, PalaceError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// The stored layout; version 0 and an empty layout when there is none.
    pub fn read(&self) -> Result<PalaceLayoutResponse, PalaceError> {
        let earlier_versions = self.earlier_versions()?;
        Ok(match Self::read_file(&self.file_path())? {
            Some(file) => PalaceLayoutResponse {
                version: file.version,
                updated_at: Some(file.updated_at),
                reason: Some(file.reason),
                layout: file.layout,
                earlier_versions,
            },
            None => PalaceLayoutResponse {
                earlier_versions,
                ..Default::default()
            },
        })
    }

    /// Kept earlier versions, newest first.
    pub fn earlier_versions(&self) -> Result<Vec<u64>, PalaceError> {
        let entries = match std::fs::read_dir(self.versions_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut versions: Vec<u64> = entries
            .flatten()
            .filter_map(|entry| {
                entry
                    .file_name()
                    .to_str()?
                    .strip_suffix(".json")?
                    .parse()
                    .ok()
            })
            .collect();
        versions.sort_unstable_by(|a, b| b.cmp(a));
        Ok(versions)
    }

    /// Replace the layout. `expected_version` must be the stored version (0
    /// before the first write).
    pub async fn write(
        &self,
        expected_version: u64,
        layout: PalaceLayout,
        reason: PalaceWriteReason,
        writer: &PalaceWriter,
    ) -> Result<PalaceLayoutResponse, PalaceError> {
        let _guard = WRITE_LOCK.lock().await;
        let current = Self::read_file(&self.file_path())?;
        let current_version = current.as_ref().map_or(0, |file| file.version);
        if expected_version != current_version {
            return Err(PalaceError::VersionConflict {
                current: current_version,
            });
        }
        let empty = PalaceLayout::default();
        let previous = current.as_ref().map_or(&empty, |file| &file.layout);
        let layout = match writer {
            PalaceWriter::Person => layout,
            PalaceWriter::Ai { .. } => {
                if reason == PalaceWriteReason::Relayout {
                    return Err(PalaceError::Invalid(
                        "only the person can ask for a relayout".into(),
                    ));
                }
                as_ai_suggestions(previous, layout)
            }
        };
        validate(&layout)?;
        self.commit(current, layout, reason, writer)
    }

    /// Write an earlier version again as the newest one.
    pub async fn revert(
        &self,
        expected_version: u64,
        to_version: Option<u64>,
    ) -> Result<PalaceLayoutResponse, PalaceError> {
        let _guard = WRITE_LOCK.lock().await;
        let current = Self::read_file(&self.file_path())?;
        let current_version = current.as_ref().map_or(0, |file| file.version);
        if expected_version != current_version {
            return Err(PalaceError::VersionConflict {
                current: current_version,
            });
        }
        let target = match to_version {
            Some(version) => version,
            None => *self
                .earlier_versions()?
                .first()
                .ok_or(PalaceError::NoSuchVersion(
                    current_version.saturating_sub(1),
                ))?,
        };
        let earlier = Self::read_file(&self.version_path(target))?
            .ok_or(PalaceError::NoSuchVersion(target))?;
        self.commit(
            current,
            earlier.layout,
            PalaceWriteReason::Revert,
            &PalaceWriter::Person,
        )
    }

    fn commit(
        &self,
        current: Option<PalaceLayoutFile>,
        layout: PalaceLayout,
        reason: PalaceWriteReason,
        writer: &PalaceWriter,
    ) -> Result<PalaceLayoutResponse, PalaceError> {
        std::fs::create_dir_all(&self.dir)?;
        let version = current.as_ref().map_or(0, |file| file.version) + 1;
        if let Some(previous) = &current {
            std::fs::create_dir_all(self.versions_dir())?;
            write_atomic(
                &self.version_path(previous.version),
                &serde_json::to_vec_pretty(previous)?,
            )?;
        }
        let (updated_by, agent) = match writer {
            PalaceWriter::Person => (PalaceAuthor::Person, None),
            PalaceWriter::Ai { agent } => (PalaceAuthor::Ai, Some(agent.clone())),
        };
        let file = PalaceLayoutFile {
            format: PALACE_FORMAT,
            version,
            updated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            updated_by,
            agent,
            reason,
            layout,
        };
        write_atomic(&self.file_path(), &serde_json::to_vec_pretty(&file)?)?;
        self.prune_versions()?;
        self.read()
    }

    fn prune_versions(&self) -> Result<(), PalaceError> {
        for version in self.earlier_versions()?.into_iter().skip(KEPT_VERSIONS) {
            let _ = std::fs::remove_file(self.version_path(version));
        }
        Ok(())
    }
}

/// Write to a temporary file beside `path`, then move it into place, so a
/// crash leaves the old file or the new one, never half of one.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), PalaceError> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| PalaceError::from(error.error))?;
    Ok(())
}

/// An AI write: what it adds is a suggestion, it cannot accept anything, and
/// the person's own items keep their authorship.
fn as_ai_suggestions(previous: &PalaceLayout, mut layout: PalaceLayout) -> PalaceLayout {
    fn mark(
        before: Option<(Option<PalaceAuthor>, Option<bool>)>,
        added_by: &mut Option<PalaceAuthor>,
        accepted: &mut Option<bool>,
    ) {
        match before {
            // New: an AI suggestion.
            None => {
                *added_by = Some(PalaceAuthor::Ai);
                *accepted = Some(false);
            }
            // Existing: authorship and acceptance stay as they were.
            Some((was_by, was_accepted)) => {
                *added_by = was_by;
                *accepted = was_accepted;
            }
        }
    }
    let nodes: HashMap<&str, _> = previous
        .nodes
        .iter()
        .map(|(id, n)| (id.as_str(), (n.added_by, n.accepted)))
        .collect();
    for (id, node) in layout.nodes.iter_mut() {
        mark(
            nodes.get(id.as_str()).copied(),
            &mut node.added_by,
            &mut node.accepted,
        );
    }
    let lines: HashMap<&str, _> = previous
        .lines
        .iter()
        .map(|l| (l.id.as_str(), (l.added_by, l.accepted)))
        .collect();
    for line in layout.lines.iter_mut() {
        mark(
            lines.get(line.id.as_str()).copied(),
            &mut line.added_by,
            &mut line.accepted,
        );
    }
    let circles: HashMap<&str, _> = previous
        .circles
        .iter()
        .map(|c| (c.id.as_str(), (c.added_by, c.accepted)))
        .collect();
    for circle in layout.circles.iter_mut() {
        mark(
            circles.get(circle.id.as_str()).copied(),
            &mut circle.added_by,
            &mut circle.accepted,
        );
    }
    let notes: HashMap<&str, _> = previous
        .notes
        .iter()
        .map(|n| (n.id.as_str(), (n.added_by, n.accepted)))
        .collect();
    for note in layout.notes.iter_mut() {
        mark(
            notes.get(note.id.as_str()).copied(),
            &mut note.added_by,
            &mut note.accepted,
        );
    }
    // An AI tool does not delete what the person placed: keep every item the
    // person added (or accepted) that the AI write left out.
    let person_owned = |by: Option<PalaceAuthor>, accepted: Option<bool>| {
        by != Some(PalaceAuthor::Ai) || accepted == Some(true)
    };
    let mut kept_nodes: BTreeMap<String, _> = previous
        .nodes
        .iter()
        .filter(|(id, n)| {
            n.added_by.is_some()
                && person_owned(n.added_by, n.accepted)
                && !layout.nodes.contains_key(*id)
        })
        .map(|(id, n)| (id.clone(), n.clone()))
        .collect();
    layout.nodes.append(&mut kept_nodes);
    for line in &previous.lines {
        if person_owned(line.added_by, line.accepted)
            && !layout.lines.iter().any(|l| l.id == line.id)
        {
            layout.lines.push(line.clone());
        }
    }
    for circle in &previous.circles {
        if person_owned(circle.added_by, circle.accepted)
            && !layout.circles.iter().any(|c| c.id == circle.id)
        {
            layout.circles.push(circle.clone());
        }
    }
    for note in &previous.notes {
        if person_owned(note.added_by, note.accepted)
            && !layout.notes.iter().any(|n| n.id == note.id)
        {
            layout.notes.push(note.clone());
        }
    }
    layout.removed = previous.removed.clone();
    layout
}

fn validate(layout: &PalaceLayout) -> Result<(), PalaceError> {
    let invalid = |what: &str| Err(PalaceError::Invalid(what.to_string()));
    let items = layout.nodes.len()
        + layout.lines.len()
        + layout.circles.len()
        + layout.notes.len()
        + layout.removed.len();
    if items > MAX_ITEMS {
        return invalid("the layout has too many items");
    }
    let id_ok = |id: &str| !id.trim().is_empty() && id.chars().count() <= MAX_ID_CHARS;
    let text_ok = |text: &Option<String>| {
        text.as_ref()
            .is_none_or(|t| t.chars().count() <= MAX_TEXT_CHARS)
    };
    for (id, node) in &layout.nodes {
        if !id_ok(id) {
            return invalid("a point id is empty or too long");
        }
        if !node.x.is_finite() || !node.y.is_finite() {
            return invalid("a point's position is not a finite number");
        }
        if !text_ok(&node.label) {
            return invalid("a point's label is too long");
        }
    }
    let mut seen = std::collections::HashSet::new();
    for line in &layout.lines {
        if !id_ok(&line.id) || !id_ok(&line.from) || !id_ok(&line.to) {
            return invalid("a line id or end is empty or too long");
        }
        if !seen.insert(("line", line.id.as_str())) {
            return invalid("two lines share an id");
        }
        if !text_ok(&line.label) {
            return invalid("a line's label is too long");
        }
    }
    for circle in &layout.circles {
        if !id_ok(&circle.id) || circle.members.iter().any(|m| !id_ok(m)) {
            return invalid("a circle id or member is empty or too long");
        }
        if !seen.insert(("circle", circle.id.as_str())) {
            return invalid("two circles share an id");
        }
        if !text_ok(&circle.label) {
            return invalid("a circle's label is too long");
        }
    }
    for note in &layout.notes {
        if !id_ok(&note.id) || note.node_id.as_deref().is_some_and(|n| !id_ok(n)) {
            return invalid("a note id is empty or too long");
        }
        if !seen.insert(("note", note.id.as_str())) {
            return invalid("two notes share an id");
        }
        if !note.x.is_finite() || !note.y.is_finite() {
            return invalid("a note's position is not a finite number");
        }
        if note.text.chars().count() > MAX_TEXT_CHARS {
            return invalid("a note is too long");
        }
    }
    if layout.removed.iter().any(|id| !id_ok(id)) {
        return invalid("a removed point id is empty or too long");
    }
    Ok(())
}

#[cfg(test)]
#[path = "palace_test.rs"]
mod tests;
