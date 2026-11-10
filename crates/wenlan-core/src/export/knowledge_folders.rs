// SPDX-License-Identifier: Apache-2.0
//! Physical organization of the Page projection. This module never changes DB scope.
use super::*;
use unicode_normalization::UnicodeNormalization as _;

const MOVE_JOURNAL: &str = ".page-move-v1.json";
const TREE_LIMIT: usize = 50_000;
const DEPTH_LIMIT: usize = 32;

pub(super) fn deserialize_pages<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, PageFileState>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Pages;
    impl<'de> serde::de::Visitor<'de> for Pages {
        type Value = HashMap<String, PageFileState>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("unique Page mappings")
        }
        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut pages = HashMap::new();
            while let Some((id, entry)) = map.next_entry::<String, PageFileState>()? {
                if pages.len() >= TREE_LIMIT || pages.insert(id, entry).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive Page mappings",
                    ));
                }
            }
            Ok(pages)
        }
    }
    deserializer.deserialize_map(Pages)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnowledgeFolderEntry {
    pub path: String,
    pub parent_path: String,
    pub name: String,
}

fn invalid_path() -> WenlanError {
    WenlanError::Validation("knowledge_folder_path_invalid".into())
}

fn reserved_directory(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("archive")
        || name.eq_ignore_ascii_case("_sources")
        || name.eq_ignore_ascii_case(crate::attachments::ATTACHMENTS_DIR)
}

fn validate_component(name: &str) -> Result<(), WenlanError> {
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('.')
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
    {
        return Err(invalid_path());
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let port = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || port.is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    {
        return Err(invalid_path());
    }
    Ok(())
}

/// Validate a portable, root-relative folder path. The empty string names the root.
pub fn validate_knowledge_folder_path(path: &str) -> Result<String, WenlanError> {
    if path.is_empty() {
        return Ok(String::new());
    }
    if path.len() > 4096 || path.split('/').count() > DEPTH_LIMIT {
        return Err(invalid_path());
    }
    for component in path.split('/') {
        validate_component(component)?;
        if reserved_directory(component) {
            return Err(invalid_path());
        }
    }
    Ok(path.to_string())
}

pub(super) fn validate_new_page_path(path: &str) -> Result<(), WenlanError> {
    if path.len() > 4096 {
        return Err(invalid_path());
    }
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    validate_knowledge_folder_path(folder)?;
    validate_component(name)?;
    if !name.ends_with(".md") {
        return Err(invalid_path());
    }
    Ok(())
}

// Existing local names are not new portable-path requests. Keep old allocations
// (including `con.md` and `.md`) and user directories without relaxing traversal.
fn validate_existing_component(name: &str) -> Result<(), WenlanError> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.len() > 255
        || name
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
    {
        return Err(invalid_path());
    }
    Ok(())
}

fn validate_existing_folder_path(path: &str) -> Result<(), WenlanError> {
    if path.is_empty() {
        return Ok(());
    }
    if path.len() > 4096 || path.split('/').count() > DEPTH_LIMIT {
        return Err(invalid_path());
    }
    for component in path.split('/') {
        validate_existing_component(component)?;
        if reserved_directory(component) {
            return Err(invalid_path());
        }
    }
    Ok(())
}

pub(super) fn validate_page_path(path: &str) -> Result<(), WenlanError> {
    if path.len() > 4096 {
        return Err(invalid_path());
    }
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    validate_existing_folder_path(folder)?;
    validate_existing_component(name)?;
    if !name.ends_with(".md") {
        return Err(invalid_path());
    }
    Ok(())
}

pub(super) fn open_existing_folder(root: &Dir, path: &str) -> Result<Dir, WenlanError> {
    validate_existing_folder_path(path)?;
    let mut directory = root.try_clone()?;
    for component in path.split('/').filter(|part| !part.is_empty()) {
        ensure_case_available(&directory, component, true)?;
        directory = directory.open_dir_nofollow(component)?;
    }
    Ok(directory)
}

pub(super) fn open_folder(root: &Dir, path: &str) -> Result<Dir, WenlanError> {
    validate_knowledge_folder_path(path)?;
    let mut directory = root.try_clone()?;
    for component in path.split('/').filter(|part| !part.is_empty()) {
        // Refuse ambiguous case spellings even on a case-sensitive host.
        ensure_case_available(&directory, component, true)?;
        directory = directory.open_dir_nofollow(component)?;
    }
    Ok(directory)
}

pub(super) fn page_parent(root: &Dir, path: &str) -> Result<(Dir, String), WenlanError> {
    validate_page_path(path)?;
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    Ok((open_existing_folder(root, folder)?, name.to_string()))
}

pub(super) fn ensure_case_available(
    directory: &Dir,
    name: &str,
    allow_exact: bool,
) -> Result<(), WenlanError> {
    let wanted = collision_key(name);
    for (index, entry) in directory.entries()?.enumerate() {
        if index >= TREE_LIMIT {
            return Err(WenlanError::Conflict("knowledge_tree_limit".into()));
        }
        let entry = entry?;
        let existing = entry.file_name();
        if collision_key(&existing.to_string_lossy()) == wanted
            && (!allow_exact || existing != OsStr::new(name))
        {
            return Err(WenlanError::Conflict("knowledge_path_collision".into()));
        }
    }
    Ok(())
}

pub(super) fn collision_key(name: &str) -> String {
    name.nfc().flat_map(char::to_uppercase).collect()
}

pub(super) fn scan_tree(
    root: &Dir,
) -> Result<(Vec<KnowledgeFolderEntry>, Vec<String>), WenlanError> {
    fn walk(
        directory: &Dir,
        parent: &str,
        depth: usize,
        count: &mut usize,
        folders: &mut Vec<KnowledgeFolderEntry>,
        pages: &mut Vec<String>,
    ) -> Result<(), WenlanError> {
        if depth > DEPTH_LIMIT {
            return Err(WenlanError::Conflict("knowledge_tree_limit".into()));
        }
        let mut entries = directory
            .entries()?
            .take(TREE_LIMIT + 1)
            .collect::<Result<Vec<_>, _>>()?;
        if entries.len() > TREE_LIMIT {
            return Err(WenlanError::Conflict("knowledge_tree_limit".into()));
        }
        entries.sort_by_key(|entry| entry.file_name());
        let mut names = std::collections::HashSet::new();
        for entry in entries {
            *count += 1;
            if *count > TREE_LIMIT {
                return Err(WenlanError::Conflict("knowledge_tree_limit".into()));
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(invalid_path());
            };
            let metadata = directory.symlink_metadata(name)?;
            if metadata.is_dir() && reserved_directory(name) {
                continue;
            }
            if !metadata.is_dir() && !name.ends_with(".md") {
                continue;
            }
            validate_existing_component(name)?;
            if !names.insert(collision_key(name)) {
                return Err(WenlanError::Conflict("knowledge_path_collision".into()));
            }
            // Never follow external directory/file links.
            if metadata.file_type().is_symlink() {
                continue;
            }
            let path = if parent.is_empty() {
                name.to_string()
            } else {
                format!("{parent}/{name}")
            };
            if metadata.is_dir() {
                folders.push(KnowledgeFolderEntry {
                    path: path.clone(),
                    parent_path: parent.to_string(),
                    name: name.to_string(),
                });
                walk(
                    &directory.open_dir_nofollow(name)?,
                    &path,
                    depth + 1,
                    count,
                    folders,
                    pages,
                )?;
            } else if metadata.is_file() && name.ends_with(".md") {
                pages.push(path);
            }
        }
        Ok(())
    }
    let (mut folders, mut pages) = (Vec::new(), Vec::new());
    walk(root, "", 0, &mut 0, &mut folders, &mut pages)?;
    Ok((folders, pages))
}

pub(super) fn read_state_strict(control: &Dir) -> Result<KnowledgeState, WenlanError> {
    let mut budget = RepairReadBudget::new();
    let Some((bytes, _, _)) = read_optional_projection_state_identity_nofollow(
        control,
        OsStr::new("state.json"),
        &mut budget,
    )?
    else {
        return Ok(KnowledgeState::default());
    };
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| WenlanError::Conflict("page_projection_state_invalid".into()))?;
    let state = if json.get("concepts").is_some() && json.get("pages").is_none() {
        let legacy: LegacyKnowledgeStateV1 = serde_json::from_slice(&bytes)
            .map_err(|_| WenlanError::Conflict("page_projection_state_invalid".into()))?;
        KnowledgeState {
            schema_version: KNOWLEDGE_STATE_SCHEMA_V2,
            pages: legacy
                .concepts
                .into_iter()
                .map(|(id, entry)| {
                    (
                        id.strip_prefix("concept_")
                            .map(|suffix| format!("page_{suffix}"))
                            .unwrap_or(id),
                        entry,
                    )
                })
                .collect(),
            ..KnowledgeState::default()
        }
    } else {
        serde_json::from_slice::<KnowledgeState>(&bytes)
            .map_err(|_| WenlanError::Conflict("page_projection_state_invalid".into()))?
    };
    let mut state = state;
    if state.schema_version == 0 {
        state.schema_version = KNOWLEDGE_STATE_SCHEMA_V2;
    }
    if state.schema_version != KNOWLEDGE_STATE_SCHEMA_V2 {
        return Err(WenlanError::Conflict(
            "page_projection_state_invalid".into(),
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for entry in state.pages.values() {
        validate_page_path(&entry.file)?;
        if !paths.insert(collision_key(&entry.file)) {
            return Err(WenlanError::Conflict(
                "page_projection_state_invalid".into(),
            ));
        }
    }
    Ok(state)
}

fn save_state(control: &Dir, state: &KnowledgeState) -> Result<(), WenlanError> {
    let temporary = format!(
        ".projection-state-{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    write_page_atomically_nofollow(
        control,
        "state.json",
        &temporary,
        &serde_json::to_vec_pretty(state)?,
    )?;
    sync_dir_capability(control)
}

pub(super) fn read_root(path: &Path) -> Result<Dir, WenlanError> {
    let parent = path.parent().ok_or_else(invalid_path)?;
    let name = path.file_name().ok_or_else(invalid_path)?;
    let parent = Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
    Ok(parent.open_dir_nofollow(Path::new(name))?)
}

pub fn list_knowledge_folders(path: &Path) -> Result<Vec<KnowledgeFolderEntry>, WenlanError> {
    let root = match read_root(path) {
        Ok(root) => root,
        Err(WenlanError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new())
        }
        Err(error) => return Err(error),
    };
    Ok(scan_tree(&root)?.0)
}

/// Bounded, nofollow discovery shared by daemon watcher and offline CLI reader.
pub fn knowledge_markdown_paths(path: &Path) -> Result<Vec<String>, WenlanError> {
    match read_root(path) {
        Ok(root) => Ok(scan_tree(&root)?.1),
        Err(WenlanError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Vec::new())
        }
        Err(error) => Err(error),
    }
}

pub fn read_knowledge_markdown(
    path: &Path,
    relative: &str,
) -> Result<(String, std::time::SystemTime), WenlanError> {
    let root = read_root(path)?;
    let (parent, name) = page_parent(&root, relative)?;
    let mut file = parent.open_with(&name, &regular_read_options())?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid_path());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(REPAIR_CAPABILITY_READ_MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > REPAIR_CAPABILITY_READ_MAX_BYTES {
        return Err(repair_projection_rollback_too_large());
    }
    let modified = metadata.modified()?.into_std();
    Ok((
        String::from_utf8(bytes).map_err(|_| invalid_path())?,
        modified,
    ))
}

/// Recover before taking a watcher snapshot, then identify only unambiguous mappings.
pub fn knowledge_edit_targets(path: &Path) -> Result<HashMap<String, String>, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let state = read_state_strict(&capabilities.wenlan)?;
        let mut claims: HashMap<String, Vec<String>> = HashMap::new();
        for path in scan_tree(&capabilities.root)?.1 {
            if is_reserved_okf_filename(path.rsplit('/').next().unwrap_or(&path)) {
                continue;
            }
            let (parent, name) = page_parent(&capabilities.root, &path)?;
            if let Some(id) = KnowledgeWriter::read_origin_id(&parent, &OsString::from(name)) {
                claims.entry(id).or_default().push(path);
            }
        }
        Ok(state
            .pages
            .into_iter()
            .filter_map(|(id, entry)| {
                (claims
                    .get(&id)
                    .is_some_and(|paths| paths.len() == 1 && paths[0] == entry.file))
                .then_some((id, entry.file))
            })
            .collect())
    })
}

/// A note the person put in the folder, as `state.json` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonPageFile {
    pub file: String,
    /// SHA-256 of the bytes last recorded for it.
    pub digest: Option<String>,
    pub frontmatter: String,
}

/// Every page recorded as a note the person put in the folder.
pub fn person_page_files(path: &Path) -> Result<HashMap<String, PersonPageFile>, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let state = read_state_strict(&capabilities.wenlan)?;
        Ok(state
            .pages
            .into_iter()
            .filter_map(|(id, entry)| {
                let frontmatter = entry.person_frontmatter?;
                Some((
                    id,
                    PersonPageFile {
                        file: entry.file,
                        digest: entry.content_sha256,
                        frontmatter,
                    },
                ))
            })
            .collect())
    })
}

/// Every page Wenlan writes itself (not a note the person put in the folder)
/// whose `state.json` entry records the SHA-256 of the bytes last written:
/// page id -> (file, digest). A file holding exactly those bytes is Wenlan's
/// own write, whatever today's render of the page looks like.
pub fn wenlan_page_digests(path: &Path) -> Result<HashMap<String, (String, String)>, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let state = read_state_strict(&capabilities.wenlan)?;
        Ok(state
            .pages
            .into_iter()
            .filter(|(_, entry)| entry.person_frontmatter.is_none())
            .filter_map(|(id, entry)| Some((id, (entry.file, entry.content_sha256?))))
            .collect())
    })
}

/// Adoptions whose id line is written and whose page may not be recorded
/// yet (page id -> file).
pub fn pending_adoptions(path: &Path) -> Result<BTreeMap<String, String>, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        Ok(read_state_strict(&capabilities.wenlan)?.pending_adoptions)
    })
}

/// The frontmatter of a note (the text between its `---` lines, with LF
/// line ends), or `None` when it has none.
pub fn adoption_frontmatter(raw: &str) -> Option<String> {
    let lf = raw.replace("\r\n", "\n");
    let (yaml, _) = crate::sources::obsidian::split_frontmatter_untrimmed(&lf)?;
    Some(yaml.trim_start_matches('\n').to_string())
}

/// `raw` with an `origin_id` line naming `page_id` as the first key of its
/// frontmatter, or with a frontmatter holding only that line when it has
/// none. Nothing else changes, line ends included.
fn with_origin_id(raw: &str, page_id: &str) -> String {
    // A byte-order mark would hide the frontmatter from every reader.
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    for open in ["---\r\n", "---\n"] {
        if raw.starts_with(open)
            && crate::sources::obsidian::split_frontmatter_untrimmed(raw).is_some()
        {
            let end = &open[3..];
            return format!("{open}origin_id: {page_id}{end}{}", &raw[open.len()..]);
        }
    }
    format!("---\norigin_id: {page_id}\n---\n\n{raw}")
}

/// `raw` without the `origin_id` line [`with_origin_id`] put in it, or
/// `None` when that line is not where it put it.
fn without_origin_id(raw: &str, page_id: &str) -> Option<String> {
    for end in ["\r\n", "\n"] {
        let added_block = format!("---{end}origin_id: {page_id}{end}---{end}{end}");
        if let Some(rest) = raw.strip_prefix(&added_block) {
            return Some(rest.to_string());
        }
        let line = format!("---{end}origin_id: {page_id}{end}");
        if let Some(rest) = raw.strip_prefix(&line) {
            return Some(format!("---{end}{rest}"));
        }
    }
    None
}

/// The `origin_id` a note's frontmatter names, read line by line so a note
/// whose YAML does not parse (`title: Meeting: notes`) still has one.
pub fn origin_id_line(raw: &str) -> Option<String> {
    let lf = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .replace("\r\n", "\n");
    let (yaml, _) = crate::sources::obsidian::split_frontmatter_untrimmed(&lf)?;
    yaml.lines().find_map(|line| {
        let value = line.strip_prefix("origin_id:")?.trim();
        let value = value.trim_matches(|c| c == '"' || c == '\'').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Replaces the note at `relative` with `next`, but only while it still
/// holds `expected`. The note is moved aside first and put back if it
/// changed, so an editor save that lands at the same moment is never lost:
/// a save before the move is seen and kept, and one after it recreates the
/// name, which the new file then does not replace.
fn replace_person_note(
    capabilities: &ProjectionCapabilities,
    relative: &str,
    expected: &[u8],
    next: &[u8],
    page_id: &str,
) -> Result<(), WenlanError> {
    let (parent, name) = page_parent(&capabilities.root, relative)?;
    let temporary = format!(
        ".{page_id}.{}.{}.tmp",
        std::process::id(),
        TEMP_FILE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut file = parent.open_with(&temporary, &options)?;
    let written = (|| {
        file.write_all(next)?;
        if let Ok(permissions) = parent
            .open_with(&name, &regular_read_options())
            .and_then(|current| current.metadata())
            .map(|metadata| metadata.permissions())
        {
            file.set_permissions(permissions)?;
        }
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = written {
        let _ = parent.remove_file(&temporary);
        return Err(error.into());
    }
    let result = (|| {
        match capabilities.wenlan.create_dir("projection-recovery") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let recovery = capabilities
            .wenlan
            .open_dir_nofollow("projection-recovery")?;
        let aside = format!("{page_id}.{}.md", uuid::Uuid::new_v4());
        parent.rename(&name, &recovery, &aside)?;
        let mut budget = RepairReadBudget::new();
        let moved = read_regular_nofollow(&recovery, OsStr::new(&aside), &mut budget)?;
        // Puts the moved-aside note back unless an editor made the name
        // again, in which case both files stay.
        let put_back = || {
            let back = recovery.hard_link(&aside, &parent, &name).is_ok()
                || (matches!(
                    parent.symlink_metadata(&name),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                ) && recovery.rename(&aside, &parent, &name).is_ok());
            if back {
                let _ = recovery.remove_file(&aside);
            } else {
                log::warn!(
                    "[knowledge] {relative} was saved while Wenlan was writing it; kept \
                     both, the earlier one is .wenlan/projection-recovery/{aside}"
                );
            }
        };
        if moved != expected {
            put_back();
            return Err(WenlanError::Conflict("page_adoption_changed".into()));
        }
        if let Err(error) = parent.hard_link(&temporary, &parent, &name) {
            put_back();
            return Err(error.into());
        }
        let _ = recovery.remove_file(&aside);
        Ok(())
    })();
    let _ = parent.remove_file(&temporary);
    result
}

fn adoption_taken() -> WenlanError {
    WenlanError::Conflict("page_adoption_taken".into())
}

fn read_page_bytes(root: &Dir, relative: &str) -> Result<Vec<u8>, WenlanError> {
    let (parent, name) = page_parent(root, relative)?;
    let mut budget = RepairReadBudget::new();
    read_regular_nofollow(&parent, OsStr::new(&name), &mut budget)
}

/// Adopts the note at `relative` as page `page_id`: writes the `origin_id`
/// line into it, when the file still holds exactly `expected` and no page
/// is recorded at that file or under that id, and returns the new text.
/// The adoption is recorded as pending first; [`record_person_note`]
/// finishes it once the page exists.
pub fn stamp_person_note(
    path: &Path,
    relative: &str,
    expected: &str,
    page_id: &str,
) -> Result<String, WenlanError> {
    validate_page_path(relative)?;
    if is_reserved_okf_filename(relative.rsplit('/').next().unwrap_or(relative)) {
        return Err(invalid_path());
    }
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        let key = collision_key(relative);
        if state.pages.contains_key(page_id)
            || state
                .pages
                .values()
                .any(|entry| collision_key(&entry.file) == key)
            || state.pending_adoptions.contains_key(page_id)
        {
            return Err(adoption_taken());
        }
        if read_page_bytes(&capabilities.root, relative)? != expected.as_bytes() {
            return Err(WenlanError::Conflict("page_adoption_changed".into()));
        }
        let stamped = with_origin_id(expected, page_id);
        state
            .pending_adoptions
            .insert(page_id.to_string(), relative.to_string());
        save_state(&capabilities.wenlan, &state)?;
        if let Err(error) = replace_person_note(
            capabilities,
            relative,
            expected.as_bytes(),
            stamped.as_bytes(),
            page_id,
        ) {
            state.pending_adoptions.remove(page_id);
            save_state(&capabilities.wenlan, &state)?;
            return Err(error);
        }
        Ok(stamped)
    })
}

/// Puts the `origin_id` line back into a note already recorded as page
/// `page_id` at `relative`, after a save from an editor that had the note
/// open since before the line was added dropped it. The text the person
/// saved is kept; the next pass reads it back as their edit.
pub fn restamp_person_note(
    path: &Path,
    relative: &str,
    expected: &str,
    page_id: &str,
) -> Result<(), WenlanError> {
    validate_page_path(relative)?;
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let state = read_state_strict(&capabilities.wenlan)?;
        let recorded = state.pages.get(page_id).is_some_and(|entry| {
            entry.person_frontmatter.is_some()
                && collision_key(&entry.file) == collision_key(relative)
        });
        if !recorded || origin_id_line(expected).is_some() {
            return Err(adoption_taken());
        }
        replace_person_note(
            capabilities,
            relative,
            expected.as_bytes(),
            with_origin_id(expected, page_id).as_bytes(),
            page_id,
        )
    })
}

/// Undoes a pending adoption whose page cannot be made (the note was
/// emptied, say): takes the `origin_id` line out of the note again, when it
/// is still where it was put, and forgets the adoption.
pub fn unstamp_person_note(
    path: &Path,
    relative: &str,
    expected: &str,
    page_id: &str,
) -> Result<(), WenlanError> {
    validate_page_path(relative)?;
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        if state.pages.contains_key(page_id) {
            return Err(adoption_taken());
        }
        if let Some(original) = without_origin_id(expected, page_id) {
            replace_person_note(
                capabilities,
                relative,
                expected.as_bytes(),
                original.as_bytes(),
                page_id,
            )?;
        }
        if state.pending_adoptions.remove(page_id).is_some() {
            save_state(&capabilities.wenlan, &state)?;
        }
        Ok(())
    })
}

/// Records `page`, adopted from the note at `relative`, as that note:
/// Wenlan writes it from then on with the person's own frontmatter.
pub fn record_person_note(path: &Path, relative: &str, page: &Page) -> Result<(), WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        let key = collision_key(relative);
        if state.pages.contains_key(&page.id)
            || state
                .pages
                .values()
                .any(|entry| collision_key(&entry.file) == key)
        {
            return Err(adoption_taken());
        }
        let bytes = read_page_bytes(&capabilities.root, relative)?;
        let raw = String::from_utf8(bytes.clone()).map_err(|_| invalid_path())?;
        if origin_id_line(&raw).as_deref() != Some(page.id.as_str()) {
            return Err(adoption_taken());
        }
        state.pages.insert(
            page.id.clone(),
            PageFileState {
                file: relative.to_string(),
                content_sha256: Some(projection_digest(&bytes)),
                version: page.version,
                last_written: page.last_modified.clone(),
                title: Some(page.title.clone()),
                description: page.summary.clone(),
                space: page.space.clone(),
                person_frontmatter: Some(adoption_frontmatter(&raw).unwrap_or_default()),
            },
        );
        state.pending_adoptions.remove(&page.id);
        save_state(&capabilities.wenlan, &state)?;
        if let Err(error) = KnowledgeWriter::regenerate_index_cap(capabilities, &state) {
            log::warn!("[knowledge] adopted a note but index regeneration failed: {error}");
        }
        Ok(())
    })
}

/// Drops a pending adoption whose note is gone.
pub fn drop_pending_adoption(path: &Path, page_id: &str) -> Result<(), WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        if state.pending_adoptions.remove(page_id).is_some() {
            save_state(&capabilities.wenlan, &state)?;
        }
        Ok(())
    })
}

/// After the person changed a note they put in the folder: records its
/// frontmatter as it is now, so Wenlan's next write keeps it, and, with
/// `version`, the bytes (`raw`) and version the page now has. Nothing is
/// recorded when the file changed again since `raw` was read.
pub fn note_person_file(
    path: &Path,
    page_id: &str,
    raw: &str,
    version: Option<i64>,
) -> Result<(), WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        let Some(entry) = state.pages.get_mut(page_id) else {
            return Ok(());
        };
        if entry.person_frontmatter.is_none()
            || read_page_bytes(&capabilities.root, &entry.file)? != raw.as_bytes()
        {
            return Ok(());
        }
        let frontmatter = adoption_frontmatter(raw).unwrap_or_default();
        let digest = projection_digest(raw.as_bytes());
        let unchanged = entry.person_frontmatter.as_deref() == Some(frontmatter.as_str())
            && (version.is_none()
                || (entry.content_sha256.as_deref() == Some(digest.as_str())
                    && version == Some(entry.version)));
        if unchanged {
            return Ok(());
        }
        entry.person_frontmatter = Some(frontmatter);
        if let Some(version) = version {
            entry.content_sha256 = Some(digest);
            entry.version = version;
        }
        save_state(&capabilities.wenlan, &state)
    })
}

/// Follows pages the person moved or renamed in the pages folder, outside
/// Wenlan (in Finder, Obsidian, a sync app): a page whose recorded file is
/// gone, and whose id is now claimed by exactly one other file no page is
/// recorded at, is recorded at that file. Identity is the `origin_id` in
/// the file, so a rename or a moved folder keeps the page, its history
/// and its links. A copy (the old file still there) or two files with the
/// same id change nothing. Returns how many pages were followed.
pub fn follow_external_moves(path: &Path) -> Result<usize, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        let gone = pages_without_their_file(&capabilities.root, &state);
        if gone.is_empty() {
            return Ok(0);
        }
        let (claims, _) = files_by_origin_id(&capabilities.root)?;
        let mut recorded: std::collections::HashSet<String> = state
            .pages
            .values()
            .map(|entry| collision_key(&entry.file))
            .collect();
        let mut followed = 0;
        for id in gone {
            let Some([found]) = claims.get(&id).map(Vec::as_slice) else {
                continue;
            };
            let entry = state.pages.get_mut(&id).ok_or_else(invalid_path)?;
            let (old_key, new_key) = (collision_key(&entry.file), collision_key(found));
            // Another page is recorded at that name: not this page's move.
            if validate_page_path(found).is_err()
                || (new_key != old_key && recorded.contains(&new_key))
            {
                continue;
            }
            recorded.remove(&old_key);
            recorded.insert(new_key);
            log::info!(
                "[page-watcher] page {id} moved outside Wenlan: {} -> {found}",
                entry.file
            );
            entry.file = found.clone();
            followed += 1;
        }
        if followed > 0 {
            save_state(&capabilities.wenlan, &state)?;
            if let Err(error) = KnowledgeWriter::regenerate_index_cap(capabilities, &state) {
                log::warn!(
                    "[knowledge] followed moved pages but index regeneration failed: {error}"
                );
            }
        }
        Ok(followed)
    })
}

/// A page's file and record as they were before a write that went to the
/// file first (see [`KnowledgeProjectionWrite::write_page_first`]).
pub(super) struct FileBefore {
    pub(super) entry: Option<PageFileState>,
    pub(super) bytes: Option<Vec<u8>>,
}

pub(super) fn capture_page_file(path: &Path, page_id: &str) -> Result<FileBefore, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let entry = read_state_strict(&capabilities.wenlan)?
            .pages
            .get(page_id)
            .cloned();
        let bytes = match &entry {
            Some(entry) => read_page_file_in(&capabilities.root, &entry.file)?,
            None => None,
        };
        Ok(FileBefore { entry, bytes })
    })
}

pub(super) fn read_page_file(path: &Path, file: &str) -> Result<Option<Vec<u8>>, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        read_page_file_in(&capabilities.root, file)
    })
}

fn read_page_file_in(root: &Dir, file: &str) -> Result<Option<Vec<u8>>, WenlanError> {
    let Ok((parent, name)) = page_parent(root, file) else {
        return Ok(None);
    };
    let mut budget = RepairReadBudget::new();
    read_optional_regular_nofollow(&parent, OsStr::new(&name), &mut budget)
}

/// Puts back what a file-first write replaced, when its database write did
/// not land: only while the file still holds exactly `written` (a save by
/// the person since is theirs and stays) and the write did not move the
/// page to another file. Returns whether it put anything back.
pub(super) fn undo_file_first(
    path: &Path,
    page_id: &str,
    file: &str,
    written: &[u8],
    before: &FileBefore,
) -> Result<bool, WenlanError> {
    let (Some(entry), Some(bytes)) = (&before.entry, &before.bytes) else {
        return Ok(false);
    };
    if entry.file != file {
        return Ok(false);
    }
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        if read_page_file_in(&capabilities.root, file)?.as_deref() != Some(written) {
            return Ok(false);
        }
        // Moved aside and checked first, like any write over a page file, so
        // a save landing in between is kept rather than replaced.
        match replace_person_note(capabilities, file, written, bytes, page_id) {
            Ok(()) => {}
            Err(WenlanError::Conflict(reason)) if reason == "page_adoption_changed" => {
                return Ok(false)
            }
            Err(error) => return Err(error),
        }
        state.pages.insert(page_id.to_string(), entry.clone());
        save_state(&capabilities.wenlan, &state)?;
        Ok(true)
    })
}

/// Pages whose recorded file is no longer there. An iCloud placeholder
/// (`.<name>.icloud`, left when macOS offloads a file) counts as there: the
/// file is still the person's, only not downloaded.
fn pages_without_their_file(root: &Dir, state: &KnowledgeState) -> Vec<String> {
    let mut gone: Vec<String> = state
        .pages
        .iter()
        .filter(|(_, entry)| {
            !matches!(
                page_parent(root, &entry.file).map(|(dir, name)| {
                    dir.symlink_metadata(&name).is_ok()
                        || dir.symlink_metadata(format!(".{name}.icloud")).is_ok()
                }),
                Ok(true)
            )
        })
        .map(|(id, _)| id.clone())
        .collect();
    gone.sort();
    gone
}

/// Every Markdown file in the pages folder, by the `origin_id` it carries.
/// Also how many files could not be opened (a sync app holding one, say):
/// what those carry is unknown.
fn files_by_origin_id(root: &Dir) -> Result<(HashMap<String, Vec<String>>, usize), WenlanError> {
    let mut claims: HashMap<String, Vec<String>> = HashMap::new();
    let mut unreadable = 0;
    for path in scan_tree(root)?.1 {
        if is_reserved_okf_filename(path.rsplit('/').next().unwrap_or(&path)) {
            continue;
        }
        let (parent, name) = page_parent(root, &path)?;
        // The quick head read first; long generated frontmatter needs the
        // full one.
        let id = KnowledgeWriter::read_origin_id(&parent, &OsString::from(&name))
            .or_else(|| KnowledgeWriter::read_origin_stamp_in(&parent, &name).map(|(id, _)| id));
        match id {
            Some(id) => claims.entry(id).or_default().push(path),
            None if parent.open(&name).is_err() => unreadable += 1,
            None => {}
        }
    }
    Ok((claims, unreadable))
}

/// Whether page `page_id`'s recorded file is still gone, checked again just
/// before it is archived.
pub fn page_file_still_gone(path: &Path, page_id: &str) -> Result<bool, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let state = read_state_strict(&capabilities.wenlan)?;
        Ok(state.pages.contains_key(page_id)
            && pages_without_their_file(&capabilities.root, &state)
                .iter()
                .any(|id| id == page_id))
    })
}

/// Records the file at `relative` as page `page`'s file again, after the
/// person brought back the file of a page that was archived (from the
/// trash, by switching a git branch back): the folder is the source of
/// truth, so the page is live again at that file. Refused when another
/// page is recorded at that file or this page has a file already, or the
/// file no longer carries the page's id.
/// Whether `raw`'s frontmatter has the `origin_version` line Wenlan writes
/// into every page file it renders.
pub fn has_origin_version(raw: &str) -> bool {
    let lf = raw.replace("\r\n", "\n");
    crate::sources::obsidian::split_frontmatter_untrimmed(&lf).is_some_and(|(yaml, _)| {
        yaml.lines()
            .any(|line| line.trim_end().starts_with("origin_version:"))
    })
}

pub fn reclaim_returned_page(path: &Path, relative: &str, page: &Page) -> Result<(), WenlanError> {
    validate_page_path(relative)?;
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        let key = collision_key(relative);
        if state.pages.contains_key(&page.id)
            || state
                .pages
                .values()
                .any(|entry| collision_key(&entry.file) == key)
        {
            return Err(WenlanError::Conflict(
                "page_projection_identity_ambiguous".into(),
            ));
        }
        let (parent, name) = page_parent(&capabilities.root, relative)?;
        let mut budget = RepairReadBudget::new();
        let bytes = read_regular_nofollow(&parent, OsStr::new(&name), &mut budget)?;
        let raw = String::from_utf8_lossy(&bytes);
        let id = KnowledgeWriter::read_origin_stamp_in(&parent, &name)
            .map(|(id, _)| id)
            .or_else(|| origin_id_line(&raw));
        if id.as_deref() != Some(page.id.as_str()) {
            return Err(WenlanError::Conflict(
                "page_projection_identity_ambiguous".into(),
            ));
        }
        // A note the person put in the folder carries no version stamp; it
        // comes back as theirs, with their own frontmatter kept.
        let person_frontmatter =
            (!has_origin_version(&raw)).then(|| adoption_frontmatter(&raw).unwrap_or_default());
        state.pages.insert(
            page.id.clone(),
            PageFileState {
                file: relative.to_string(),
                content_sha256: Some(projection_digest(&bytes)),
                version: page.version,
                last_written: page.last_modified.clone(),
                title: Some(page.title.clone()),
                description: page.summary.clone(),
                space: page.space.clone(),
                person_frontmatter,
            },
        );
        state.archived_from.remove(&page.id);
        state.archived_versions.remove(&page.id);
        state.archived_person_frontmatter.remove(&page.id);
        state.missing_since.remove(&page.id);
        save_state(&capabilities.wenlan, &state)
    })
}

/// A page file must stay gone this long before its page is archived.
pub const MISSING_SETTLE_SECS: i64 = 60;
/// More files than this gone at once, and more than half of all pages, is
/// held: an unmounted drive, a sync tool resetting the folder or a bad
/// checkout looks exactly like that, and archiving would follow a mistake.
pub const MASS_MISSING_FLOOR: usize = 10;

/// What [`note_missing_pages`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MissingPages {
    /// Pages whose file has stayed gone for [`MISSING_SETTLE_SECS`], with no
    /// other file carrying their id: deleted in the folder.
    pub deleted: Vec<String>,
    /// Gone, but not for long enough yet.
    pub waiting: usize,
    /// Set when so many files are gone at once that none is treated as
    /// deleted: how many are gone.
    pub held: Option<usize>,
}

/// Find pages whose file the person deleted from the folder.
///
/// A page counts as deleted only when its recorded file is gone, no other
/// file in the folder carries its id (a move is followed by
/// [`follow_external_moves`], and a copy or conflict file still holds the
/// page), and the file has been gone since a pass at least
/// [`MISSING_SETTLE_SECS`] before `now`. The first sighting is kept in
/// `state.json`, so a pass that sees the file back clears it.
///
/// Only records of pages in `known` (every page the database has) count,
/// for what is gone and for how many pages there are: a record left from a
/// page the database no longer has names nothing to archive, and hundreds
/// of them would otherwise hold every real deletion.
pub fn note_missing_pages(
    path: &Path,
    now: i64,
    known: &std::collections::HashSet<String>,
) -> Result<MissingPages, WenlanError> {
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let mut state = read_state_strict(&capabilities.wenlan)?;
        let mut gone = pages_without_their_file(&capabilities.root, &state);
        gone.retain(|id| known.contains(id));
        let recorded = state.pages.keys().filter(|id| known.contains(*id)).count();
        let mut unreadable = 0;
        if !gone.is_empty() {
            let (claims, unknown) = files_by_origin_id(&capabilities.root)?;
            gone.retain(|id| !claims.contains_key(id));
            unreadable = unknown;
        }
        let held = gone.len() > MASS_MISSING_FLOOR && gone.len() * 2 > recorded;
        let before = state.missing_since.clone();
        if held {
            // Not a sighting: once the hold lifts, what is still gone waits
            // the full time again from then.
            state.missing_since.clear();
        } else {
            state.missing_since.retain(|id, _| gone.contains(id));
            for id in &gone {
                state.missing_since.entry(id.clone()).or_insert(now);
            }
        }
        if state.missing_since != before {
            save_state(&capabilities.wenlan, &state)?;
        }
        let mut found = MissingPages::default();
        if held {
            found.held = Some(gone.len());
            return Ok(found);
        }
        if unreadable > 0 {
            // A file that could not be read may be one of these, moved.
            found.waiting = gone.len();
            return Ok(found);
        }
        for id in gone {
            let since = state.missing_since.get(&id).copied().unwrap_or(now);
            if now - since >= MISSING_SETTLE_SECS {
                found.deleted.push(id);
            } else {
                found.waiting += 1;
            }
        }
        Ok(found)
    })
}

pub fn create_knowledge_folder(
    path: &Path,
    parent_path: &str,
    name: &str,
) -> Result<String, WenlanError> {
    validate_knowledge_folder_path(parent_path)?;
    validate_component(name)?;
    if reserved_directory(name) {
        return Err(invalid_path());
    }
    let created_path = if parent_path.is_empty() {
        name.to_string()
    } else {
        format!("{parent_path}/{name}")
    };
    validate_knowledge_folder_path(&created_path)?;
    create_projection_root_nofollow(path)?;
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        read_state_strict(&capabilities.wenlan)?;
        let parent = open_folder(&capabilities.root, parent_path)?;
        ensure_case_available(&parent, name, true)?;
        match parent.symlink_metadata(name) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(WenlanError::Conflict("knowledge_path_collision".into())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                parent.create_dir(name)?;
            }
            Err(error) => return Err(error.into()),
        }
        // Check the created object too; an external writer may have raced mkdir.
        parent.open_dir_nofollow(name)?;
        sync_dir_capability(&parent)?;
        Ok(created_path)
    })
}

#[derive(Serialize, Deserialize)]
struct MoveJournal {
    version: u32,
    operation_id: String,
    page_id: String,
    source: String,
    destination: String,
    stage: String,
    content_sha256: String,
    entry_sha256: String,
    device: u64,
    inode: u64,
}

fn recovery_required() -> WenlanError {
    WenlanError::Conflict("page_move_recovery_required".into())
}

fn file_snapshot(
    directory: &Dir,
    name: &str,
) -> Result<Option<(String, ProjectionFileIdentity)>, WenlanError> {
    let mut budget = RepairReadBudget::new();
    match read_regular_identity_nofollow_bounded(directory, OsStr::new(name), &mut budget) {
        Ok((bytes, identity)) => Ok(Some((projection_digest(&bytes), identity))),
        Err(WenlanError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn matches_snapshot(
    snapshot: &Option<(String, ProjectionFileIdentity)>,
    journal: &MoveJournal,
) -> bool {
    snapshot.as_ref().is_some_and(|(digest, identity)| {
        digest == &journal.content_sha256
            && identity.device == journal.device
            && identity.inode == journal.inode
    })
}

fn verify_parent_identity(root: &Dir, relative: &str, pinned: &Dir) -> Result<(), WenlanError> {
    let folder = relative.rsplit_once('/').map_or("", |(folder, _)| folder);
    let current = open_folder(root, folder)?;
    if projection_file_identity(&current.dir_metadata()?)?
        != projection_file_identity(&pinned.dir_metadata()?)?
    {
        return Err(recovery_required());
    }
    Ok(())
}

/// Recover only exact journal-owned bytes/paths. Ambiguity preserves every inode.
pub(super) fn recover_move(capabilities: &ProjectionCapabilities) -> Result<(), WenlanError> {
    recover_move_with_checkpoint(capabilities, &mut |_| Ok(()))
}

fn recover_move_with_checkpoint(
    capabilities: &ProjectionCapabilities,
    checkpoint: &mut impl FnMut(&str) -> Result<(), WenlanError>,
) -> Result<(), WenlanError> {
    let mut budget = RepairReadBudget::new();
    let Some(bytes) = read_optional_regular_nofollow(
        &capabilities.wenlan,
        OsStr::new(MOVE_JOURNAL),
        &mut budget,
    )?
    else {
        return Ok(());
    };
    let journal: MoveJournal = serde_json::from_slice(&bytes).map_err(|_| recovery_required())?;
    if journal.version != 1
        || journal
            .stage
            .strip_prefix(".page-move-stage-")
            .and_then(|suffix| uuid::Uuid::parse_str(suffix).ok())
            .is_none()
    {
        return Err(recovery_required());
    }
    validate_page_path(&journal.source)?;
    validate_page_path(&journal.destination)?;
    let (source_dir, source_name) = page_parent(&capabilities.root, &journal.source)?;
    let (target_dir, target_name) = page_parent(&capabilities.root, &journal.destination)?;
    let source = file_snapshot(&source_dir, &source_name)?;
    let stage = file_snapshot(&capabilities.wenlan, &journal.stage)?;
    let target = file_snapshot(&target_dir, &target_name)?;
    let mut state = read_state_strict(&capabilities.wenlan)?;
    let entry = state
        .pages
        .get_mut(&journal.page_id)
        .ok_or_else(recovery_required)?;
    if entry.file != journal.source && entry.file != journal.destination {
        return Err(recovery_required());
    }
    let mut before_entry = entry.clone();
    before_entry.file = journal.source.clone();
    if projection_digest(&serde_json::to_vec(&before_entry)?) != journal.entry_sha256 {
        return Err(recovery_required());
    }
    if stage.is_none()
        && target.is_none()
        && matches_snapshot(&source, &journal)
        && entry.file == journal.source
    {
        capabilities.wenlan.remove_file(MOVE_JOURNAL)?;
        sync_dir_capability(&capabilities.wenlan)?;
        return Ok(());
    }
    if source.is_some()
        || (!matches_snapshot(&stage, &journal) && !matches_snapshot(&target, &journal))
        || target
            .as_ref()
            .is_some_and(|_| !matches_snapshot(&target, &journal))
        || stage
            .as_ref()
            .is_some_and(|_| !matches_snapshot(&stage, &journal))
    {
        return Err(recovery_required());
    }
    if target.is_none() {
        verify_parent_identity(&capabilities.root, &journal.destination, &target_dir)?;
        ensure_case_available(&target_dir, &target_name, false)?;
        capabilities
            .wenlan
            .hard_link(&journal.stage, &target_dir, &target_name)?;
        sync_dir_capability(&target_dir)?;
    }
    checkpoint("installed")?;
    verify_parent_identity(&capabilities.root, &journal.destination, &target_dir)?;
    verify_parent_identity(&capabilities.root, &journal.source, &source_dir)?;
    // Origin and inode are checked after installation; never overwrite a raced target.
    if KnowledgeWriter::read_origin_id(&target_dir, &OsString::from(&target_name)).as_deref()
        != Some(&journal.page_id)
        || !matches_snapshot(&file_snapshot(&target_dir, &target_name)?, &journal)
    {
        return Err(recovery_required());
    }
    if file_snapshot(&source_dir, &source_name)?.is_some() {
        return Err(recovery_required());
    }
    entry.file = journal.destination.clone();
    save_state(&capabilities.wenlan, &state)?;
    checkpoint("state")?;
    if file_snapshot(&source_dir, &source_name)?.is_some() {
        return Err(recovery_required());
    }
    if stage.is_some() {
        if !matches_snapshot(
            &file_snapshot(&capabilities.wenlan, &journal.stage)?,
            &journal,
        ) {
            return Err(recovery_required());
        }
        capabilities.wenlan.remove_file(&journal.stage)?;
    }
    capabilities.wenlan.remove_file(MOVE_JOURNAL)?;
    sync_dir_capability(&capabilities.wenlan)?;
    if let Err(error) = KnowledgeWriter::regenerate_index_cap(capabilities, &state) {
        log::warn!("[knowledge] moved page but index regeneration failed: {error}");
    }
    Ok(())
}

pub fn move_projected_page(
    path: &Path,
    page_id: &str,
    expected_storage_path: &str,
    folder_path: &str,
    operation_id: &str,
) -> Result<String, WenlanError> {
    move_projected_page_with_checkpoint(
        path,
        page_id,
        expected_storage_path,
        folder_path,
        operation_id,
        &mut |_| Ok(()),
    )
}

fn move_projected_page_with_checkpoint(
    path: &Path,
    page_id: &str,
    expected_storage_path: &str,
    folder_path: &str,
    operation_id: &str,
    checkpoint: &mut impl FnMut(&str) -> Result<(), WenlanError>,
) -> Result<String, WenlanError> {
    validate_page_path(expected_storage_path)?;
    validate_knowledge_folder_path(folder_path)?;
    if operation_id.is_empty()
        || operation_id.len() > 128
        || operation_id.chars().any(char::is_control)
    {
        return Err(invalid_path());
    }
    KnowledgeProjectionWrite::with_projection_capabilities(path, |capabilities| {
        let state = read_state_strict(&capabilities.wenlan)?;
        let entry = state
            .pages
            .get(page_id)
            .ok_or_else(|| WenlanError::Conflict("page_projection_missing".into()))?;
        let name = expected_storage_path
            .rsplit('/')
            .next()
            .ok_or_else(invalid_path)?;
        if is_reserved_okf_filename(name) {
            return Err(invalid_path());
        }
        let destination = if folder_path.is_empty() {
            name.to_string()
        } else {
            format!("{folder_path}/{name}")
        };
        let (target_dir, target_name) = page_parent(&capabilities.root, &destination)?;
        if entry.file == destination {
            ensure_case_available(&target_dir, &target_name, true)?;
            if origin_paths(&capabilities.root, page_id)? == [destination.clone()]
                && KnowledgeWriter::read_origin_id(&target_dir, &OsString::from(&target_name))
                    .as_deref()
                    == Some(page_id)
            {
                return Ok(destination);
            }
            return Err(recovery_required());
        }
        if entry.file != expected_storage_path {
            return Err(WenlanError::Conflict("page_storage_path_changed".into()));
        }
        ensure_case_available(&target_dir, &target_name, false)?;
        let claimants = origin_paths(&capabilities.root, page_id)?;
        if claimants != [expected_storage_path] {
            return Err(WenlanError::Conflict(
                "page_projection_identity_ambiguous".into(),
            ));
        }
        let (source_dir, source_name) = page_parent(&capabilities.root, expected_storage_path)?;
        let (digest, identity) =
            file_snapshot(&source_dir, &source_name)?.ok_or_else(recovery_required)?;
        // A nested mount point must not strand a source halfway through rename/link.
        if projection_file_identity(&capabilities.wenlan.dir_metadata()?)?.device != identity.device
            || projection_file_identity(&target_dir.dir_metadata()?)?.device != identity.device
        {
            return Err(WenlanError::Conflict("page_move_cross_device".into()));
        }
        let journal = MoveJournal {
            version: 1,
            operation_id: operation_id.to_string(),
            page_id: page_id.to_string(),
            source: expected_storage_path.to_string(),
            destination: destination.clone(),
            stage: format!(".page-move-stage-{}", uuid::Uuid::new_v4()),
            content_sha256: digest,
            entry_sha256: projection_digest(&serde_json::to_vec(entry)?),
            device: identity.device,
            inode: identity.inode,
        };
        write_new_file_nofollow(
            &capabilities.wenlan,
            OsStr::new(MOVE_JOURNAL),
            &serde_json::to_vec(&journal)?,
        )?;
        sync_dir_capability(&capabilities.wenlan)?;
        checkpoint("journal")?;
        verify_parent_identity(&capabilities.root, expected_storage_path, &source_dir)?;
        verify_parent_identity(&capabilities.root, &destination, &target_dir)?;
        source_dir.rename(&source_name, &capabilities.wenlan, &journal.stage)?;
        sync_dir_capability(&source_dir)?;
        sync_dir_capability(&capabilities.wenlan)?;
        checkpoint("stage")?;
        recover_move_with_checkpoint(capabilities, checkpoint)?;
        Ok(destination)
    })
}

pub(super) fn origin_paths(root: &Dir, page_id: &str) -> Result<Vec<String>, WenlanError> {
    let mut paths = Vec::new();
    for path in scan_tree(root)?.1 {
        // A user's index/log made by copying a Page is still a reserved navigation
        // document, not a second writable Page. Preserve its existing protection.
        if is_reserved_okf_filename(path.rsplit('/').next().unwrap_or(&path)) {
            continue;
        }
        let (parent, name) = page_parent(root, &path)?;
        if KnowledgeWriter::read_origin_id(&parent, &OsString::from(name)).as_deref()
            == Some(page_id)
        {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[cfg(test)]
#[path = "knowledge_folders_test.rs"]
mod tests;
