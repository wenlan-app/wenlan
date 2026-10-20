// SPDX-License-Identifier: Apache-2.0
//! A note can rely on lines of code: the filesystem side.
//!
//! [`crate::code_anchor`] is the pure fingerprint; `db/code_basis.rs` stores
//! the rows. This module is the part between them that touches the disk:
//!
//! - [`resolve_code_path`] decides whether `repo_root` + `path` is a place the
//!   daemon may look (absolute root, relative `/`-separated path that cannot
//!   climb out of it),
//! - [`read_code_file`] reads one file for a check (UTF-8 lossy, 2 MB cap,
//!   symlinks may not leave the repository),
//! - [`prepare_code_basis`] turns what an agent wrote into `(input, anchor)`
//!   pairs before the write, so a file that cannot be read fails the request
//!   instead of landing as a row that was never true,
//! - [`recheck_code_basis`] and [`confirm_code_basis`] re-read files and move
//!   rows between `ok`, `moved`, `pending` and `unavailable`.
//!
//! Nothing here knows about HTTP. Blocking file reads run on the blocking
//! pool, and no database lock is held while a file is being read.

use crate::code_anchor::{self, AnchorError, CodeAnchor, LineSpan, Relocation};
use crate::db::{CodeBasisRow, CodeBasisVerdict, MemoryDB};
use crate::error::WenlanError;
use crate::read_scope::ReadScope;
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use wenlan_types::{CodeBasisInput, CodeBasisLocator, CodeBasisRecheckEntry, CodeBasisRef};

/// Largest file a code basis may point into. A bigger file is `unavailable`
/// (`too_large`) on a check and refused on a write.
pub const MAX_CODE_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Most code bases one write may carry.
pub const MAX_CODE_BASIS_PER_WRITE: usize = 100;

/// Rows whose files are read in one blocking task.
pub const RECHECK_BATCH: usize = 500;

/// `page_code_basis.state` values.
pub const CODE_BASIS_STATE_OK: &str = "ok";
pub const CODE_BASIS_STATE_MOVED: &str = "moved";
pub const CODE_BASIS_STATE_PENDING: &str = "pending";
pub const CODE_BASIS_STATE_UNAVAILABLE: &str = "unavailable";

/// Typed refusals for the code-basis operations. The server maps each kind to
/// a status: `Invalid` 400, `NotFound` 404, `Conflict` 409.
#[derive(Debug, thiserror::Error)]
pub enum CodeBasisError {
    /// The request names something that can never be a code basis (a path that
    /// climbs out of the repository, a span past the end of the file, a file
    /// that cannot be read at write time).
    #[error("{0}")]
    Invalid(String),
    /// The note has no such code basis.
    #[error("this note does not rely on those lines")]
    NotFound,
    /// The code basis exists but cannot be settled right now (the file is
    /// unreadable, the lines cannot be found, the row changed under the call).
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] WenlanError),
}

impl From<libsql::Error> for CodeBasisError {
    fn from(error: libsql::Error) -> Self {
        CodeBasisError::Db(WenlanError::VectorDb(format!("page_code_basis: {error}")))
    }
}

impl From<serde_json::Error> for CodeBasisError {
    fn from(error: serde_json::Error) -> Self {
        CodeBasisError::Db(WenlanError::Json(error))
    }
}

impl From<CodeBasisError> for WenlanError {
    fn from(error: CodeBasisError) -> Self {
        match error {
            CodeBasisError::Db(inner) => inner,
            CodeBasisError::NotFound => WenlanError::NotFound(error.to_string()),
            CodeBasisError::Conflict(message) => WenlanError::Conflict(message),
            CodeBasisError::Invalid(message) => WenlanError::Validation(message),
        }
    }
}

/// Why a file could not be read for a check. The word is what a row stores as
/// `unavailable_reason` and what a reader sees as `reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeFileProblem {
    RepoMissing,
    FileMissing,
    OutsideRepo,
    Unreadable,
    TooLarge,
}

impl CodeFileProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepoMissing => "repo_missing",
            Self::FileMissing => "file_missing",
            Self::OutsideRepo => "outside_repo",
            Self::Unreadable => "unreadable",
            Self::TooLarge => "too_large",
        }
    }
}

impl std::fmt::Display for CodeFileProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::RepoMissing => "the repository folder does not exist",
            Self::FileMissing => "the file does not exist",
            Self::OutsideRepo => "the path leads outside the repository",
            Self::Unreadable => "the file could not be read",
            Self::TooLarge => "the file is larger than 2 MB",
        })
    }
}

/// Check the shape of a repository-relative path: non-empty, `/` separated,
/// and made only of ordinary names (no empty, `.` or `..` segment, no
/// backslash, no drive prefix, not absolute).
fn validate_relative_path(path: &str) -> Result<(), CodeBasisError> {
    let bad = |why: &str| CodeBasisError::Invalid(format!("path {path:?} is not valid: {why}"));
    if path.is_empty() {
        return Err(bad("it is empty"));
    }
    if path.contains('\0') {
        return Err(bad("it contains a NUL byte"));
    }
    if path.contains('\\') {
        return Err(bad("separate folders with `/`"));
    }
    if path.starts_with('/') {
        return Err(bad("it must be relative to repo_root"));
    }
    for segment in path.split('/') {
        match segment {
            "" => return Err(bad("it has an empty segment")),
            "." | ".." => return Err(bad("`.` and `..` are not allowed")),
            _ => {}
        }
    }
    // The platform's own reading of the path must agree: on Windows this is
    // what rejects `C:foo` and `\\server` style prefixes.
    if !Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(bad("it must be made of plain folder and file names"));
    }
    Ok(())
}

/// Where `repo_root` + `path` points, if that is somewhere under `repo_root`.
///
/// Pure: nothing is read from the filesystem. `repo_root` must be absolute and
/// free of `.` and `..`; `path` must pass the shape check above; the joined
/// path, normalized by [`Path::components`], must still start with
/// `repo_root`. Symlinks are handled where the file is read
/// ([`read_code_file`]).
pub fn resolve_code_path(repo_root: &str, path: &str) -> Result<PathBuf, CodeBasisError> {
    let root = Path::new(repo_root);
    if repo_root.is_empty() || repo_root.contains('\0') || !root.is_absolute() {
        return Err(CodeBasisError::Invalid(
            "repo_root must be an absolute path".into(),
        ));
    }
    // `Path::components` silently drops an interior `.`, so look at the raw
    // segments instead of the parsed components.
    if repo_root
        .split(['/', '\\'])
        .any(|segment| segment == "." || segment == "..")
    {
        return Err(CodeBasisError::Invalid(
            "repo_root must not contain `.` or `..`".into(),
        ));
    }
    validate_relative_path(path)?;
    let normalized: PathBuf = root.join(path).components().collect();
    if !normalized.starts_with(root) {
        return Err(CodeBasisError::Invalid(format!(
            "path {path:?} leads outside repo_root"
        )));
    }
    Ok(normalized)
}

/// The blocking half of [`read_code_file`].
fn read_code_file_blocking(repo_root: &str, path: &str) -> Result<String, CodeFileProblem> {
    let resolved = resolve_code_path(repo_root, path).map_err(|_| CodeFileProblem::OutsideRepo)?;
    let root = match std::fs::canonicalize(repo_root) {
        Ok(root) if root.is_dir() => root,
        Ok(_) => return Err(CodeFileProblem::RepoMissing),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CodeFileProblem::RepoMissing)
        }
        Err(_) => return Err(CodeFileProblem::Unreadable),
    };
    let file = match std::fs::canonicalize(&resolved) {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            return Err(CodeFileProblem::FileMissing)
        }
        Err(_) => return Err(CodeFileProblem::Unreadable),
    };
    // A symlink inside the repository may not lead out of it.
    if !file.starts_with(&root) {
        return Err(CodeFileProblem::OutsideRepo);
    }
    let metadata = std::fs::metadata(&file).map_err(|_| CodeFileProblem::Unreadable)?;
    if !metadata.is_file() {
        return Err(CodeFileProblem::Unreadable);
    }
    if metadata.len() > MAX_CODE_FILE_BYTES {
        return Err(CodeFileProblem::TooLarge);
    }
    let bytes = std::fs::read(&file).map_err(|_| CodeFileProblem::Unreadable)?;
    if bytes.len() as u64 > MAX_CODE_FILE_BYTES {
        return Err(CodeFileProblem::TooLarge);
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Read one file of a repository for a code-basis check, off the async
/// runtime's worker threads.
pub async fn read_code_file(repo_root: &str, path: &str) -> Result<String, CodeFileProblem> {
    let (repo_root, path) = (repo_root.to_string(), path.to_string());
    tokio::task::spawn_blocking(move || read_code_file_blocking(&repo_root, &path))
        .await
        .unwrap_or(Err(CodeFileProblem::Unreadable))
}

/// Read many files at once, one blocking task for the lot. Each distinct
/// `(repo_root, path)` is read once however many rows point at it.
async fn read_code_files(
    keys: Vec<(String, String)>,
) -> HashMap<(String, String), Result<String, CodeFileProblem>> {
    tokio::task::spawn_blocking(move || {
        keys.into_iter()
            .map(|key| {
                let read = read_code_file_blocking(&key.0, &key.1);
                (key, read)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Fingerprint what an agent says a note relies on, before the note is
/// written. A path that escapes the repository, a span that selects nothing or
/// reaches past the end of the file, and a file that cannot be read are all
/// refusals ([`CodeBasisError::Invalid`]); none becomes a quiet
/// `unavailable` row. Identical entries collapse into one.
pub async fn prepare_code_basis(
    inputs: &[CodeBasisInput],
) -> Result<Vec<(CodeBasisInput, CodeAnchor)>, CodeBasisError> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    if inputs.len() > MAX_CODE_BASIS_PER_WRITE {
        return Err(CodeBasisError::Invalid(format!(
            "a write may carry at most {MAX_CODE_BASIS_PER_WRITE} code_basis entries"
        )));
    }
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for input in inputs {
        resolve_code_path(&input.repo_root, &input.path)?;
        if input.line_start == 0 || input.line_end < input.line_start {
            return Err(CodeBasisError::Invalid(format!(
                "{}: {}",
                input.path,
                AnchorError::EmptySpan
            )));
        }
        if seen.insert(input.clone()) {
            unique.push(input.clone());
        }
    }
    let keys: Vec<(String, String)> = unique
        .iter()
        .map(|input| (input.repo_root.clone(), input.path.clone()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let files = read_code_files(keys).await;
    let mut prepared = Vec::with_capacity(unique.len());
    for input in unique {
        let text = match files.get(&(input.repo_root.clone(), input.path.clone())) {
            Some(Ok(text)) => text,
            Some(Err(problem)) => {
                return Err(CodeBasisError::Invalid(format!(
                    "cannot read {}: {problem} ({})",
                    input.path,
                    problem.as_str()
                )))
            }
            None => {
                return Err(CodeBasisError::Invalid(format!(
                    "cannot read {}",
                    input.path
                )))
            }
        };
        let span = LineSpan {
            start: input.line_start,
            end: input.line_end,
        };
        let anchor = code_anchor::anchor(text, span)
            .map_err(|error| CodeBasisError::Invalid(format!("{}: {error}", input.path)))?;
        prepared.push((input, anchor));
    }
    Ok(prepared)
}

/// What a check of one row decided, given the file as it reads now.
fn verdict_for(row: &CodeBasisRow, file: &Result<String, CodeFileProblem>) -> CodeBasisVerdict {
    let text = match file {
        Ok(text) => text,
        Err(problem) => return CodeBasisVerdict::Unavailable(problem.as_str()),
    };
    let hint = LineSpan {
        start: row.reference.line_start,
        end: row.reference.line_end,
    };
    match code_anchor::relocate(text, Some(hint), &row.anchor) {
        Relocation::Unchanged { .. } => CodeBasisVerdict::Unchanged,
        Relocation::Moved { span } => match code_anchor::anchor(text, span) {
            Ok(anchor) => CodeBasisVerdict::Moved { span, anchor },
            Err(_) => CodeBasisVerdict::Pending,
        },
        Relocation::Changed { .. } | Relocation::Unresolved => CodeBasisVerdict::Pending,
    }
}

/// The Space (by name) a read scope selects: `None` is every Space.
fn space_of(scope: &ReadScope) -> Option<String> {
    match scope {
        ReadScope::Global => None,
        ReadScope::Space(space) => Some(space.clone()),
        ReadScope::Uncategorized => Some(crate::db::UNFILED_SPACE_ID.to_string()),
    }
}

/// Re-read the files behind every code basis in `space` (`None`: every Space)
/// and move each row to the state its file now justifies. Returns how many
/// rows were checked. Needs no model; one read per distinct file.
pub async fn recheck_code_basis(db: &MemoryDB, space: Option<&str>) -> Result<usize, WenlanError> {
    let rows = db.list_code_basis_rows(space).await?;
    let checked = rows.len();
    // Files are read a batch at a time (each batch is one blocking task); the
    // verdicts are applied together, so a row can move into a place another
    // row of the note is leaving.
    let mut verdicts: Vec<(&CodeBasisRow, CodeBasisVerdict)> = Vec::with_capacity(rows.len());
    for batch in rows.chunks(RECHECK_BATCH) {
        let keys: Vec<(String, String)> = batch
            .iter()
            .map(|row| (row.reference.repo_root.clone(), row.reference.path.clone()))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let files = read_code_files(keys).await;
        let unreadable = Err(CodeFileProblem::Unreadable);
        for row in batch {
            let key = (row.reference.repo_root.clone(), row.reference.path.clone());
            verdicts.push((
                row,
                verdict_for(row, files.get(&key).unwrap_or(&unreadable)),
            ));
        }
    }
    db.apply_code_basis_verdicts(&verdicts, chrono::Utc::now().timestamp())
        .await?;
    Ok(checked)
}

/// [`recheck_code_basis`] for a read scope, then the notes that now have a
/// `pending` or `unavailable` code basis, grouped by note.
pub async fn recheck_code_basis_in_scope(
    db: &MemoryDB,
    scope: &ReadScope,
) -> Result<Vec<CodeBasisRecheckEntry>, WenlanError> {
    let space = space_of(scope);
    recheck_code_basis(db, space.as_deref()).await?;
    let mut entries: Vec<CodeBasisRecheckEntry> = Vec::new();
    for (page_id, title, reference) in db
        .list_code_basis_needing_attention(space.as_deref())
        .await?
    {
        match entries.last_mut() {
            Some(entry) if entry.page_id == page_id => entry.pending.push(reference),
            _ => entries.push(CodeBasisRecheckEntry {
                page_id,
                title,
                pending: vec![reference],
            }),
        }
    }
    entries.sort_by_key(|entry| (entry.title.to_lowercase(), entry.page_id.clone()));
    Ok(entries)
}

/// For the Brief: recheck the Space and count the notes that rely on code that
/// changed or went missing. `None` when no note in the Space has a code basis,
/// in which case nothing is read.
pub async fn brief_code_basis_pending(
    db: &MemoryDB,
    space: &str,
) -> Result<Option<u32>, WenlanError> {
    if !db.space_has_code_basis(space).await? {
        return Ok(None);
    }
    recheck_code_basis(db, Some(space)).await?;
    Ok(Some(db.count_pages_with_code_basis_pending(space).await?))
}

/// Settle one code basis: "I have accounted for the code as it reads now."
///
/// The file is read again, the lines are found where they are now (a changed
/// block is found between its unchanged neighbours; failing that, the span the
/// row already has is taken as meant), the anchor is recomputed from the
/// current text, and the row returns to `ok`.
pub async fn confirm_code_basis(
    db: &MemoryDB,
    page_id: &str,
    locator: &CodeBasisLocator,
    by: &str,
) -> Result<CodeBasisRef, CodeBasisError> {
    let row = db
        .get_code_basis_row(page_id, locator)
        .await?
        .ok_or(CodeBasisError::NotFound)?;
    let text = read_code_file(&locator.repo_root, &locator.path)
        .await
        .map_err(|problem| {
            CodeBasisError::Conflict(format!(
                "cannot read {}: {problem} ({})",
                locator.path,
                problem.as_str()
            ))
        })?;
    let stored = LineSpan {
        start: locator.line_start,
        end: locator.line_end,
    };
    let span = match code_anchor::relocate(&text, Some(stored), &row.anchor) {
        Relocation::Unchanged { span }
        | Relocation::Moved { span }
        | Relocation::Changed { span } => span,
        Relocation::Unresolved => stored,
    };
    let anchor = code_anchor::anchor(&text, span).map_err(|error| {
        CodeBasisError::Conflict(format!(
            "{}: the lines can no longer be located ({error}); write the note again with the \
             lines it relies on now",
            locator.path
        ))
    })?;
    db.confirm_code_basis(
        page_id,
        locator,
        &row.anchor_json,
        span,
        &anchor,
        by,
        chrono::Utc::now().timestamp(),
    )
    .await
}
