// SPDX-License-Identifier: Apache-2.0
//! A code basis whose file is gone: follow a rename, or tell a deletion apart
//! from a file that is only missing for now.
//!
//! A check ([`crate::code_basis::recheck_code_basis`]) that cannot find a
//! row's file asks git, read only, what became of it:
//!
//! | git says                                                   | the row                                   |
//! |------------------------------------------------------------|-------------------------------------------|
//! | renamed (staged, or in the commit that removed the path)   | follows the file to its new path          |
//! | the same file, byte for byte, sits at another path         | follows it there                          |
//! | the cited lines turned up whole in a new, untracked file   | follows it there                          |
//! | deleted (in the working tree, or by a commit on this branch)| `unavailable`, reason `file_deleted`      |
//! | nothing (no git, never tracked, another branch)            | stays `unavailable`, reason `file_missing`|
//!
//! A row that follows its file is `moved` when its lines read the same there,
//! and `pending` when they changed (the everyday model then judges the change
//! the usual way). A `file_deleted` row is steep's work: the note is rewritten
//! without what only those lines supported, and the row is removed
//! ([`crate::steep`]). Only a file that is missing for a reason git cannot
//! give is left for a person.
//!
//! Every git command runs with an argument array, a `--` before any path, a
//! ten second limit and `GIT_OPTIONAL_LOCKS=0`, so a check never takes the
//! repository's index lock. Nothing here writes to the repository.

use crate::code_anchor::{self, CodeAnchor, LineSpan, Relocation};
use crate::code_basis::{read_code_file_stamped, CodeFileProblem};
use crate::db::{CodeBasisRow, CodeBasisVerdict, MemoryDB};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use wenlan_types::CodeBasisLocator;

/// `unavailable_reason` of a row whose file git saw deleted.
pub const FILE_DELETED: &str = "file_deleted";

const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Most git output one command may produce.
const MAX_GIT_OUTPUT: u64 = 16 * 1024 * 1024;

/// Most new files read to look for the cited lines in.
const MAX_CANDIDATES: usize = 300;

/// Most renames followed in a row (a file renamed, then renamed again).
const MAX_HOPS: usize = 3;

/// A file still missing after a look is looked at again only this often.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// When each still-missing `(repo_root, path)` was last looked into.
static LAST_LOOK: std::sync::LazyLock<Mutex<HashMap<(String, String), Instant>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

async fn git(repo_root: &str, args: &[&str]) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let run = async {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(MAX_GIT_OUTPUT + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() as u64 > MAX_GIT_OUTPUT {
            return None;
        }
        let status = child.wait().await.ok()?;
        status.success().then_some(bytes)
    };
    tokio::time::timeout(GIT_TIMEOUT, run).await.ok()?
}

/// `-z` output split into its NUL-terminated fields.
fn fields(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect()
}

/// What `git status` says about a repository, read once per check.
#[derive(Debug, Default)]
struct Status {
    /// Staged renames: old path to new path.
    renamed: HashMap<String, String>,
    /// Tracked files deleted in the index or the working tree.
    deleted: HashSet<String>,
    /// Files added to the index or not tracked at all: where a renamed file
    /// may have gone.
    new_files: Vec<String>,
}

/// `git status --porcelain=v1 -z`: `XY path`, and for a rename or copy
/// `XY new` followed by the old path as its own field.
fn parse_status(out: &[u8]) -> Status {
    let mut status = Status::default();
    let mut entries = fields(out).into_iter();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let (x, y) = (code.as_bytes()[0], code.as_bytes()[1]);
        if x == b'R' || x == b'C' {
            if let Some(from) = entries.next() {
                if x == b'R' {
                    status.renamed.insert(from, path.to_string());
                }
            }
            continue;
        }
        if x == b'D' || y == b'D' {
            status.deleted.insert(path.to_string());
        } else if x == b'A' || code.starts_with("??") {
            status.new_files.push(path.to_string());
        }
    }
    status
}

/// A repository as git describes it, or `None` when `repo_root` is not the
/// top of a git work tree git can read.
struct Repo {
    root: String,
    status: Status,
    /// Tracked path for each blob id in the index, when it was read.
    by_blob: Option<HashMap<String, Vec<String>>>,
}

impl Repo {
    async fn open(root: &str) -> Option<Self> {
        let prefix = git(root, &["rev-parse", "--show-prefix"]).await?;
        // Paths git prints are relative to the top of the work tree; a code
        // basis whose root is a folder inside one is not followed.
        if !String::from_utf8_lossy(&prefix).trim().is_empty() {
            return None;
        }
        let out = git(root, &["status", "--porcelain=v1", "-z", "-uall"]).await?;
        Some(Self {
            root: root.to_string(),
            status: parse_status(&out),
            by_blob: None,
        })
    }

    async fn paths_with_blob(&mut self, blob: &str) -> Vec<String> {
        if self.by_blob.is_none() {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            if let Some(out) = git(&self.root, &["ls-files", "-s", "-z"]).await {
                // `<mode> <blob> <stage>\t<path>`
                for entry in fields(&out) {
                    let Some((meta, path)) = entry.split_once('\t') else {
                        continue;
                    };
                    if let Some(id) = meta.split(' ').nth(1) {
                        map.entry(id.to_string())
                            .or_default()
                            .push(path.to_string());
                    }
                }
            }
            self.by_blob = Some(map);
        }
        self.by_blob
            .as_ref()
            .and_then(|map| map.get(blob))
            .cloned()
            .unwrap_or_default()
    }

    /// What the last commit on this branch that touched `path` did to it:
    /// renamed it (to the new path), deleted it, or neither.
    async fn last_commit_change(&self, path: &str) -> Option<CommitChange> {
        let out = git(&self.root, &["log", "-1", "--format=%H", "--", path]).await?;
        let commit = String::from_utf8_lossy(&out).trim().to_string();
        if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let out = git(
            &self.root,
            &[
                "diff-tree",
                "-r",
                "-z",
                "-M",
                "--name-status",
                "--no-commit-id",
                &commit,
            ],
        )
        .await?;
        let mut entries = fields(&out).into_iter();
        let mut change = None;
        while let Some(kind) = entries.next() {
            if kind.starts_with('R') || kind.starts_with('C') {
                let (Some(from), Some(to)) = (entries.next(), entries.next()) else {
                    break;
                };
                if kind.starts_with('R') && from == path {
                    return Some(CommitChange::Renamed(to));
                }
            } else {
                let Some(changed) = entries.next() else {
                    break;
                };
                if kind == "D" && changed == path {
                    change = Some(CommitChange::Deleted);
                }
            }
        }
        change
    }
}

enum CommitChange {
    Renamed(String),
    Deleted,
}

/// What became of one missing file.
enum Fate {
    /// It is at this path now; `by_git` when git itself recorded the rename.
    Moved {
        path: String,
        by_git: bool,
    },
    Deleted,
    Unknown,
}

fn exists(root: &str, path: &str) -> bool {
    crate::code_basis::resolve_code_path(root, path).is_ok_and(|full| full.is_file())
}

/// Follow git's renames from `path`, at most [`MAX_HOPS`] of them, to a file
/// that exists; or say git saw it deleted.
async fn fate_by_git(repo: &Repo, path: &str) -> Fate {
    let mut current = path.to_string();
    let mut renamed = false;
    for _ in 0..MAX_HOPS {
        let next = match repo.status.renamed.get(&current) {
            Some(to) => Some(to.clone()),
            None if repo.status.deleted.contains(&current) => return Fate::Deleted,
            None => match repo.last_commit_change(&current).await {
                Some(CommitChange::Renamed(to)) => Some(to),
                Some(CommitChange::Deleted) => return Fate::Deleted,
                None => None,
            },
        };
        let Some(next) = next else { break };
        if exists(&repo.root, &next) {
            return Fate::Moved {
                path: next,
                by_git: true,
            };
        }
        current = next;
        renamed = true;
    }
    // Renamed to a path that is gone too, with no further record: whatever
    // happened to it, the cited file is no longer in the repository.
    if renamed {
        Fate::Deleted
    } else {
        Fate::Unknown
    }
}

/// Look for the file by its content: the exact file the rows were cited
/// against at another tracked path, or the cited lines found whole in one of
/// the files git sees as new.
async fn fate_by_content(
    repo: &mut Repo,
    path: &str,
    rows: &[&CodeBasisRow],
    blobs: &[String],
    cited: &HashSet<&str>,
) -> Option<String> {
    let base = path.rsplit('/').next().unwrap_or(path);
    let root = repo.root.clone();
    let pick = |mut found: Vec<String>| -> Option<String> {
        found.sort();
        found.dedup();
        // A file a note already relies on was there all along: a copy, not
        // where this one went.
        found.retain(|p| p != path && !cited.contains(p.as_str()) && exists(&root, p));
        match found.len() {
            1 => found.pop(),
            _ => {
                let same_name: Vec<&String> = found
                    .iter()
                    .filter(|p| p.rsplit('/').next() == Some(base))
                    .collect();
                (same_name.len() == 1).then(|| same_name[0].clone())
            }
        }
    };
    for blob in blobs {
        let tracked = repo.paths_with_blob(blob).await;
        if let Some(found) = pick(tracked) {
            return Some(found);
        }
    }
    let candidates: Vec<String> = repo
        .status
        .new_files
        .iter()
        .filter(|p| p.as_str() != path && !cited.contains(p.as_str()))
        .take(MAX_CANDIDATES)
        .cloned()
        .collect();
    let mut same_file = Vec::new();
    let mut same_lines = Vec::new();
    for candidate in candidates {
        let Ok((text, _, blob)) = read_code_file_stamped(&root, &candidate).await else {
            continue;
        };
        if blobs.contains(&blob) {
            same_file.push(candidate);
            continue;
        }
        // Every row's lines must be there, unchanged, for a new file to count
        // as the old one under a new name.
        let all_there = !rows.is_empty()
            && rows.iter().all(|row| {
                matches!(
                    code_anchor::relocate(&text, None, &row.anchor),
                    Relocation::Unchanged { .. } | Relocation::Moved { .. }
                )
            });
        if all_there {
            same_lines.push(candidate);
        }
    }
    pick(same_file).or_else(|| (same_lines.len() == 1).then(|| same_lines.remove(0)))
}

/// The row at its file's new path: `moved` when its lines read the same there,
/// `pending` when they changed. `by_git`: git recorded the rename, so even
/// lines that cannot be found any more follow it (and wait for a look).
async fn follow(row: &CodeBasisRow, path: &str, by_git: bool) -> Option<CodeBasisVerdict> {
    let root = &row.reference.repo_root;
    let (text, stamp, _) = read_code_file_stamped(root, path).await.ok()?;
    let hint = LineSpan {
        start: row.reference.line_start,
        end: row.reference.line_end,
    };
    let (span, anchor): (LineSpan, Option<CodeAnchor>) =
        match code_anchor::relocate(&text, Some(hint), &row.anchor) {
            Relocation::Unchanged { span } | Relocation::Moved { span } => {
                (span, code_anchor::anchor(&text, span).ok())
            }
            Relocation::Changed { span } => (span, None),
            Relocation::Unresolved if by_git => {
                let lines = u32::try_from(text.lines().count())
                    .unwrap_or(u32::MAX)
                    .max(1);
                let start = hint.start.min(lines);
                let end = hint.end.min(lines).max(start);
                (LineSpan { start, end }, None)
            }
            Relocation::Unresolved => return None,
        };
    Some(CodeBasisVerdict::Renamed {
        path: path.to_string(),
        span,
        anchor,
        stamp,
    })
}

/// Whether `key` may be looked into now, recording the look.
fn may_look(key: &(String, String), already_missing: bool) -> bool {
    let mut last = LAST_LOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    last.retain(|_, at| now.duration_since(*at) < RETRY_AFTER);
    if already_missing && last.contains_key(key) {
        return false;
    }
    last.insert(key.clone(), now);
    true
}

/// Replace each `file_missing` verdict in `verdicts` with what git says
/// became of the file: [`CodeBasisVerdict::Renamed`] to follow it, or
/// `unavailable` with [`FILE_DELETED`]. A row already marked deleted whose
/// file is still gone stays deleted without a look. A file that stayed missing
/// for no reason git gives is looked into again only after [`RETRY_AFTER`].
/// `rows` is every row of the check: a file one of them cites is never taken
/// for where a missing one went.
pub async fn follow_missing_files(
    db: &MemoryDB,
    rows: &[CodeBasisRow],
    verdicts: &mut [(&CodeBasisRow, CodeBasisVerdict)],
) {
    let missing = CodeFileProblem::FileMissing.as_str();
    let mut groups: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, (row, verdict)) in verdicts.iter_mut().enumerate() {
        if !matches!(verdict, CodeBasisVerdict::Unavailable(reason) if *reason == missing) {
            continue;
        }
        if row.reference.reason.as_deref() == Some(FILE_DELETED) {
            *verdict = CodeBasisVerdict::Unavailable(FILE_DELETED);
            continue;
        }
        groups
            .entry((row.reference.repo_root.clone(), row.reference.path.clone()))
            .or_default()
            .push(i);
    }
    let mut cited_in: HashMap<String, HashSet<String>> = HashMap::new();
    for row in rows {
        cited_in
            .entry(row.reference.repo_root.clone())
            .or_default()
            .insert(row.reference.path.clone());
    }
    let mut repos: HashMap<String, Option<Repo>> = HashMap::new();
    for (key, indexes) in groups {
        let already_missing = indexes.iter().all(|&i| {
            let row = verdicts[i].0;
            row.reference.state == crate::code_basis::CODE_BASIS_STATE_UNAVAILABLE
                && row.reference.reason.as_deref() == Some(missing)
        });
        if !may_look(&key, already_missing) {
            continue;
        }
        let (root, path) = &key;
        if !repos.contains_key(root) {
            repos.insert(root.clone(), Repo::open(root).await);
        }
        let Some(Some(repo)) = repos.get_mut(root) else {
            continue;
        };
        let rows: Vec<&CodeBasisRow> = indexes.iter().map(|&i| verdicts[i].0).collect();
        let fate = match fate_by_git(repo, path).await {
            moved @ Fate::Moved { .. } => moved,
            by_git => {
                // A file moved without telling git looks deleted to it, so the
                // content is looked for either way.
                let deleted = matches!(by_git, Fate::Deleted);
                let mut blobs = Vec::new();
                for row in &rows {
                    if let Ok(Some(cited)) = db
                        .get_code_basis_cited(&row.page_id, &CodeBasisLocator::from(&row.reference))
                        .await
                    {
                        if let Some(blob) = cited.cited_blob {
                            if !blobs.contains(&blob) {
                                blobs.push(blob);
                            }
                        }
                    }
                }
                let cited: HashSet<&str> = cited_in
                    .get(root)
                    .map(|paths| paths.iter().map(String::as_str).collect())
                    .unwrap_or_default();
                match fate_by_content(repo, path, &rows, &blobs, &cited).await {
                    Some(found) => Fate::Moved {
                        path: found,
                        by_git: false,
                    },
                    None if deleted => Fate::Deleted,
                    None => Fate::Unknown,
                }
            }
        };
        for i in indexes {
            let row = verdicts[i].0;
            let next = match &fate {
                Fate::Moved { path, by_git } => follow(row, path, *by_git).await,
                Fate::Deleted => Some(CodeBasisVerdict::Unavailable(FILE_DELETED)),
                Fate::Unknown => None,
            };
            if let Some(next) = next {
                verdicts[i].1 = next;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_renames_deletions_and_new_files() {
        let out = b"R  src/consumer.ts\0src/worker.ts\0 D src/gone.ts\0D  src/staged.ts\0?? src/new.ts\0A  src/added.ts\0 M src/kept.ts\0";
        let status = parse_status(out);
        assert_eq!(
            status.renamed.get("src/worker.ts").map(String::as_str),
            Some("src/consumer.ts")
        );
        assert!(status.deleted.contains("src/gone.ts"));
        assert!(status.deleted.contains("src/staged.ts"));
        assert_eq!(status.new_files, vec!["src/new.ts", "src/added.ts"]);
    }
}
