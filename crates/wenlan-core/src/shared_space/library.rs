// SPDX-License-Identifier: Apache-2.0
//! The library share's folder layout, its computers, and the jobs only its
//! main computer runs.
//!
//! The person chooses one folder `X`. Inside it:
//!
//! ```text
//! X/同步資料（請勿打開）/        the engine's files (`space.json`, `members/…`)
//! X/給 AI 讀的頁面/             the phone read copy (one-way, `reading_copy`)
//! X/給 AI 讀的頁面/收件匣/       files a phone AI leaves for Wenlan to take
//! X/給 AI 讀的頁面/收件匣/已收/   what was taken
//! ```
//!
//! `space.json` lists the library's computers (`devices`) and which one is
//! `main_device`. Only the main computer writes the read copy, takes the
//! inbox and lets AI write page text (`page_writing_allowed_at`); the other
//! computers sync and let the person edit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use wenlan_types::shared_space::SharedSyncCounts;

use super::{
    own_shares, read_space_file, share_settings_at, Scope, ShareSettings, SpaceDevice, SpaceFile,
    SPACE_FILE,
};
use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::reading_copy::{self, CopyState};

/// The engine's folder inside the chosen folder.
pub const ENGINE_DIR: &str = "同步資料（請勿打開）";
/// The phone read copy's folder inside the chosen folder.
pub const READ_DIR: &str = "給 AI 讀的頁面";
/// Where a phone AI leaves new things, inside [`READ_DIR`].
pub const INBOX_DIR: &str = "收件匣";
/// Where taken inbox files go, inside [`INBOX_DIR`].
pub const TAKEN_DIR: &str = "已收";
/// The label of pages with no Space in the read copy.
const UNFILED_LABEL: &str = "未分類";
/// `app_metadata` key of the read copy's [`CopyState`].
pub const COPY_STATE_KEY: &str = "reading_copy.state";

/// The read copy is rewritten at most this often, unless the pass changed
/// something.
const COPY_EVERY: Duration = Duration::from_secs(300);
/// An inbox file changed more recently than this may still be arriving.
const INBOX_SETTLE: Duration = Duration::from_secs(10);
/// The shortest text a memory is made of (the app's own capture rule).
const CAPTURE_MIN_CHARS: usize = 10;

/// The folder made for the library inside a cloud drive when the person has
/// not chosen one.
pub const LIBRARY_FOLDER_NAME: &str = "Wenlan";
/// Google Drive's own folder, by the names it has in the person's language.
const MY_DRIVE: [&str; 11] = [
    "My Drive",
    "我的雲端硬碟",
    "我的云端硬盘",
    "マイドライブ",
    "내 드라이브",
    "Mi unidad",
    "Mon Drive",
    "Meine Ablage",
    "Meu Drive",
    "Il mio Drive",
    "Mijn Drive",
];

/// A cloud drive's folder on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudDrive {
    GoogleDrive,
    ICloud,
}

/// Where the library goes when the person turns syncing on: a library
/// already in any of their cloud drives (their other computer's), else a new
/// [`LIBRARY_FOLDER_NAME`] folder there; iCloud Drive when there is no
/// Google Drive, since a phone AI reads only Google Drive. `None` when this
/// computer has neither. `drive_roots` are Windows drive roots (`G:\`),
/// where Google Drive for desktop keeps its own drive.
pub fn suggested_folder(home: &Path, drive_roots: &[PathBuf]) -> Option<(CloudDrive, PathBuf)> {
    let mut google: Vec<PathBuf> = Vec::new();
    let mut accounts: Vec<PathBuf> = std::fs::read_dir(home.join("Library/CloudStorage"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("GoogleDrive-"))
        })
        .collect();
    accounts.sort();
    for parent in accounts
        .iter()
        .chain(drive_roots)
        .chain(std::iter::once(&home.join("Google Drive")))
    {
        google.extend(MY_DRIVE.iter().map(|name| parent.join(name)));
    }
    // Backup and Sync, before Drive for desktop: the drive is the folder.
    google.push(home.join("Google Drive"));
    let icloud = [
        home.join("Library/Mobile Documents/com~apple~CloudDocs"),
        home.join("iCloudDrive"),
    ];
    let roots: Vec<(CloudDrive, PathBuf)> = google
        .into_iter()
        .map(|root| (CloudDrive::GoogleDrive, root))
        .chain(icloud.into_iter().map(|root| (CloudDrive::ICloud, root)))
        .filter(|(_, root)| root.is_dir())
        .collect();
    // A library already in any of them (the other computer's) comes first,
    // so a second account or drive never splits the library in two.
    for (cloud, root) in &roots {
        let mut libraries: Vec<PathBuf> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|folder| holds_library(folder))
            .collect();
        libraries.sort();
        if let Some(folder) = libraries.into_iter().next() {
            return Some((*cloud, folder));
        }
    }
    roots
        .into_iter()
        .next()
        .map(|(cloud, root)| (cloud, root.join(LIBRARY_FOLDER_NAME)))
}

/// `folder` holds a synced library (not a shared Space, not something else).
fn holds_library(folder: &Path) -> bool {
    read_space_file(&folder.join(ENGINE_DIR))
        .ok()
        .flatten()
        .is_some_and(|file| file.library)
}

/// [`suggested_folder`] on this computer.
pub fn suggested_folder_here() -> Option<(CloudDrive, PathBuf)> {
    let home = dirs::home_dir()?;
    let drive_roots: Vec<PathBuf> = if cfg!(windows) {
        ('D'..='Z')
            .map(|letter| PathBuf::from(format!("{letter}:\\")))
            .filter(|root| root.is_dir())
            .collect()
    } else {
        Vec::new()
    };
    suggested_folder(&home, &drive_roots)
}

/// The engine folder of the library the person put in `chosen`.
pub fn engine_folder(chosen: &Path) -> PathBuf {
    let mut best: Option<(String, PathBuf)> = None;
    if let Ok(entries) = std::fs::read_dir(chosen) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !is_engine_name(&name) || !entry.path().is_dir() {
                continue;
            }
            let Ok(Some(file)) = read_space_file(&entry.path()) else {
                continue;
            };
            let Some(id) = file.members.iter().map(|m| m.id.clone()).min() else {
                continue;
            };
            if file.library && best.as_ref().is_none_or(|(best, _)| &id < best) {
                best = Some((id, entry.path()));
            }
        }
    }
    best.map(|(_, path)| path)
        .unwrap_or_else(|| chosen.join(ENGINE_DIR))
}

/// [`ENGINE_DIR`], or the copy a cloud drive keeps when two computers made
/// it before seeing each other's: `<name> (2)` (Google Drive) or `<name> 2`
/// (iCloud Drive).
fn is_engine_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(ENGINE_DIR) else {
        return false;
    };
    let number = rest
        .strip_prefix(" (")
        .and_then(|rest| rest.strip_suffix(')'))
        .or_else(|| rest.strip_prefix(' '))
        .unwrap_or(rest);
    rest.is_empty() || (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

/// The phone read copy's folder of the library in `chosen`.
pub fn read_folder(chosen: &Path) -> PathBuf {
    chosen.join(READ_DIR)
}

/// The inbox of the library in `chosen`.
pub fn inbox_folder(chosen: &Path) -> PathBuf {
    read_folder(chosen).join(INBOX_DIR)
}

/// This computer's name, for telling the library's computers apart.
pub fn computer_name() -> String {
    let clean = |name: String| {
        let name = name.trim().to_string();
        (!name.is_empty()).then_some(name)
    };
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| clean(String::from_utf8_lossy(&out.stdout).into_owned()))
    };
    if let Some(name) = std::env::var("WENLAN_TEST_COMPUTER_NAME")
        .ok()
        .and_then(clean)
    {
        return name;
    }
    std::env::var("COMPUTERNAME")
        .ok()
        .and_then(clean)
        .or_else(|| run("scutil", &["--get", "ComputerName"]))
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .and_then(clean)
        })
        .or_else(|| std::env::var("HOSTNAME").ok().and_then(clean))
        .or_else(|| run("hostname", &[]))
        .unwrap_or_else(|| "This computer".to_string())
}

fn write_space_file(folder: &Path, file: &SpaceFile) -> Result<(), WenlanError> {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new_in(folder)?;
    tmp.write_all((serde_json::to_string_pretty(file)? + "\n").as_bytes())?;
    tmp.flush()?;
    tmp.persist(folder.join(SPACE_FILE))
        .map_err(|error| WenlanError::Io(error.error))?;
    Ok(())
}

/// Adds `device` to the engine folder's `space.json` (which must be there)
/// when it is not listed yet. A listed computer is never rewritten, even
/// under another name: a file two computers rewrite every pass would become
/// conflict copies. The first computer listed is the main one.
pub fn add_device(folder: &Path, device: &SpaceDevice) -> Result<SpaceFile, WenlanError> {
    let Some(mut file) = read_space_file(folder)? else {
        return Err(WenlanError::NotFound(
            folder.join(SPACE_FILE).display().to_string(),
        ));
    };
    if device.id.is_empty() || file.devices.iter().any(|listed| listed.id == device.id) {
        return Ok(file);
    }
    if file.devices.is_empty() && file.main_device.is_empty() {
        file.main_device = device.id.clone();
    }
    file.devices.push(device.clone());
    write_space_file(folder, &file)?;
    Ok(file)
}

/// Makes `device_id` the library's main computer (the person's choice). The
/// computer must be listed in the engine folder of the library in `chosen`.
pub fn set_main_device(chosen: &Path, device_id: &str) -> Result<SpaceFile, WenlanError> {
    let folder = engine_folder(chosen);
    let Some(mut file) = read_space_file(&folder)? else {
        return Err(WenlanError::NotFound(
            folder.join(SPACE_FILE).display().to_string(),
        ));
    };
    if !file.devices.iter().any(|device| device.id == device_id) {
        return Err(WenlanError::Validation(format!(
            "{device_id} is not a computer of this library"
        )));
    }
    if file.main_device != device_id {
        file.main_device = device_id.to_string();
        write_space_file(&folder, &file)?;
    }
    Ok(file)
}

// ===== What this process knows between passes =====

#[derive(Default)]
struct Known {
    /// Whether this computer was the main one at the last look.
    main: Option<bool>,
    last_copy: Option<Instant>,
    copy_error: Option<String>,
    warned_inbox: std::collections::HashSet<PathBuf>,
}

static KNOWN: LazyLock<Mutex<HashMap<PathBuf, Known>>> = LazyLock::new(Default::default);

fn with_known<T>(settings_path: &Path, f: impl FnOnce(&mut Known) -> T) -> T {
    let mut all = KNOWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(all.entry(settings_path.to_path_buf()).or_default())
}

/// Why the last read copy failed, if it did.
pub(super) fn reading_copy_error(settings_path: &Path) -> Option<String> {
    with_known(settings_path, |known| known.copy_error.clone())
}

fn is_main(file: &SpaceFile, settings: &ShareSettings) -> bool {
    // A folder from before computers were listed has no main: every
    // computer counts, as it did.
    file.main_device.is_empty() || file.main_device == settings.device_id
}

/// Whether AI may write or rewrite page text on this computer: always,
/// unless it shares the library and another computer is the main one. The
/// answer when the folder cannot be read is the last one known, else no: a
/// computer that cannot tell must not write over the main one's work.
pub fn page_writing_allowed_at(settings_path: &Path) -> bool {
    let Ok(Some(settings)) = share_settings_at(settings_path, &Scope::Library) else {
        return true;
    };
    if settings.device_id.is_empty() {
        return true;
    }
    match read_space_file(&engine_folder(Path::new(&settings.folder))) {
        Ok(Some(file)) => {
            let main = is_main(&file, &settings);
            with_known(settings_path, |known| known.main = Some(main));
            main
        }
        _ => with_known(settings_path, |known| known.main).unwrap_or(false),
    }
}

// ===== Main computer jobs =====

/// After a library pass that ended `Done`: on the main computer, the read
/// copy and the inbox. Errors are logged and shown in the status; they never
/// fail the sync.
pub(super) async fn main_jobs(
    db: &MemoryDB,
    settings_path: &Path,
    settings: &ShareSettings,
    counts: &SharedSyncCounts,
) {
    let engine = engine_folder(Path::new(&settings.folder));
    let file = match tokio::task::spawn_blocking(move || read_space_file(&engine)).await {
        Ok(Ok(Some(file))) => file,
        _ => return,
    };
    let main = is_main(&file, settings);
    with_known(settings_path, |known| known.main = Some(main));
    if !main {
        with_known(settings_path, |known| known.copy_error = None);
        return;
    }
    let chosen = PathBuf::from(&settings.folder);
    let changed = counts.applied > 0 || counts.exported > 0;
    let due = with_known(settings_path, |known| {
        changed || known.last_copy.is_none_or(|at| at.elapsed() >= COPY_EVERY)
    });
    if due {
        let outcome = write_copy(db, settings_path, &chosen).await;
        with_known(settings_path, |known| {
            known.last_copy = Some(Instant::now());
            known.copy_error = match &outcome {
                Ok(()) => None,
                Err(error) => {
                    if known.copy_error.as_deref() != Some(error.as_str()) {
                        log::warn!("[shared_space] reading copy not written: {error}");
                    }
                    Some(error.clone())
                }
            };
        });
    }
    if let Err(error) = take_inbox(db, settings_path, &chosen).await {
        log::warn!("[shared_space] inbox not taken: {error}");
    }
}

async fn write_copy(db: &MemoryDB, settings_path: &Path, chosen: &Path) -> Result<(), String> {
    let (own_spaces, own_pages) = own_shares(settings_path).map_err(|e| e.to_string())?;
    let mut state: CopyState = match db.get_app_metadata(COPY_STATE_KEY).await {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
        Ok(None) => CopyState::default(),
        Err(error) => return Err(error.to_string()),
    };
    let mut pages = reading_copy::pages_for_copy(db, &own_spaces, &state)
        .await
        .map_err(|e| e.to_string())?;
    pages.retain(|page| !own_pages.contains(&page.id));
    let dir = read_folder(chosen);
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (state, result) = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        let result =
            reading_copy::write_reading_copy(&dir, &pages, &mut state, &now, UNFILED_LABEL)?;
        Ok::<_, WenlanError>((state, result))
    })
    .await
    .map_err(|e| format!("reading copy task: {e}"))?
    .map_err(|e| e.to_string())?;
    // Written before the state is kept: a state that is lost only means the
    // files are compared again.
    let raw = serde_json::to_string(&state).map_err(|e| e.to_string())?;
    db.set_app_metadata(COPY_STATE_KEY, &raw)
        .await
        .map_err(|e| e.to_string())?;
    if result.written + result.removed > 0 {
        log::info!(
            "[shared_space] reading copy: {} written, {} removed, {} unchanged",
            result.written,
            result.removed,
            result.unchanged
        );
    }
    Ok(())
}

/// The memory a person's own quick capture makes: the same document the
/// app's capture sends to `/api/ingest/memory` (source `memory`, a
/// `manual_` id, `capture_source` manual, no Space).
pub async fn capture_text(db: &MemoryDB, title: &str, content: &str) -> Result<usize, WenlanError> {
    capture_text_as(
        db,
        &format!("manual_{}", uuid::Uuid::new_v4()),
        title,
        content,
    )
    .await
}

/// The id an inbox file is kept under: the same file is the same memory,
/// however often it is taken (a move to `已收` that failed, two computers
/// that both took it).
fn inbox_memory_id(title: &str, content: &str) -> String {
    let digest = super::version::sha256_hex(format!("{title}\n{content}").as_bytes());
    format!("manual_inbox_{}", &digest[..32])
}

async fn capture_text_as(
    db: &MemoryDB,
    source_id: &str,
    title: &str,
    content: &str,
) -> Result<usize, WenlanError> {
    let doc = crate::sources::RawDocument {
        source: "memory".into(),
        source_id: source_id.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        last_modified: chrono::Utc::now().timestamp(),
        metadata: [("capture_source".to_string(), "manual".to_string())].into(),
        ..Default::default()
    };
    db.upsert_documents(vec![doc]).await
}

/// A file in the inbox that is ready to take.
fn inbox_candidates(inbox: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(inbox) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("txt")
                })
        })
        .filter(|entry| {
            entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| SystemTime::now().duration_since(at).ok())
                .is_some_and(|age| age >= INBOX_SETTLE)
        })
        .map(|entry| entry.path())
        .collect();
    out.sort();
    out
}

/// Moves `file` into `taken`, adding " (2)", " (3)"… on a name clash.
fn move_to_taken(file: &Path, taken: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(taken)?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = file
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut target = taken.join(file.file_name().unwrap_or_default());
    let mut n = 2;
    while target.exists() {
        target = taken.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::rename(file, &target)?;
    Ok(target)
}

/// Saves each ready inbox file as a memory, then moves it to `已收`. A file
/// is never deleted, and one that could not be saved stays where it is.
async fn take_inbox(db: &MemoryDB, settings_path: &Path, chosen: &Path) -> Result<(), WenlanError> {
    let inbox = inbox_folder(chosen);
    let dir = inbox.clone();
    let files = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        Ok::<_, WenlanError>(inbox_candidates(&dir))
    })
    .await
    .map_err(|e| WenlanError::Generic(format!("inbox task: {e}")))??;
    for file in files {
        let path = file.clone();
        let read = tokio::task::spawn_blocking(move || std::fs::read(&path))
            .await
            .map_err(|e| WenlanError::Generic(format!("inbox task: {e}")))?;
        let text = match read {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(error) => {
                log::warn!(
                    "[shared_space] inbox file {} not read: {error}",
                    file.display()
                );
                continue;
            }
        };
        if text.trim().chars().count() < CAPTURE_MIN_CHARS {
            let first = with_known(settings_path, |known| {
                known.warned_inbox.insert(file.clone())
            });
            if first {
                log::warn!(
                    "[shared_space] inbox file {} is too short to keep; left where it is",
                    file.display()
                );
            }
            continue;
        }
        let title = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        let id = inbox_memory_id(&title, &text);
        let kept = db.get_memory_detail(&id).await.ok().flatten().is_some();
        let saved = if kept {
            // Taken before; only the move to `已收` is left.
            Ok(1)
        } else {
            capture_text_as(db, &id, &title, &text).await
        };
        match saved {
            Ok(chunks) if chunks > 0 => {}
            Ok(_) => {
                log::warn!(
                    "[shared_space] inbox file {} saved nothing; left",
                    file.display()
                );
                continue;
            }
            Err(error) => {
                log::warn!(
                    "[shared_space] inbox file {} not saved: {error}",
                    file.display()
                );
                continue;
            }
        }
        let taken = inbox.join(TAKEN_DIR);
        let from = file.clone();
        match tokio::task::spawn_blocking(move || move_to_taken(&from, &taken)).await {
            Ok(Ok(_)) => log::info!("[shared_space] took inbox file {}", file.display()),
            Ok(Err(error)) => {
                log::warn!(
                    "[shared_space] inbox file {} not moved: {error}",
                    file.display()
                )
            }
            Err(error) => log::warn!("[shared_space] inbox move task: {error}"),
        }
    }
    Ok(())
}
