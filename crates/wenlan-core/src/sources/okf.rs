// SPDX-License-Identifier: Apache-2.0
//! OKF v0.2 bundle source connector.
//!
//! An OKF (Open Knowledge Format) v0.2 bundle is a folder of markdown
//! "concept" files carrying YAML frontmatter (title, type, tags, status, and
//! so on). This module parses one concept file at a time: extract the
//! frontmatter and body, map recognized fields onto [`OkfConcept`], resolve
//! body/`sources[]` links to sibling concept ids, and (mirroring
//! `sources::directory::file_to_documents` for `.md`) turn the body into
//! chunked [`RawDocument`]s ready for the folder-provenance pipeline.

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::chunker::ChunkingEngine;
use crate::sources::RawDocument;

// ---------------------------------------------------------------------------
// Pre-compiled regexes
// ---------------------------------------------------------------------------

/// Same shape as `export::okf::MD_LINK_RE`: `[text](target)`. That regex is
/// private to its module, so this mirrors it locally rather than widening its
/// visibility just for this one shared shape.
static LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\(([^)\s]+)\)").unwrap());

/// A URI scheme prefix, e.g. `https:`, `repo:`, `mailto:`.
static URI_SCHEME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").unwrap());

// ---------------------------------------------------------------------------
// Reserved files and concept ids
// ---------------------------------------------------------------------------

/// The file names OKF reserves, spec 3.1: a directory's index and its update
/// history. Matched by file name only (any depth), ASCII case-insensitive.
/// Neither may be a concept document, so neither is ever imported as one.
const RESERVED_NAMES: &[&str] = &["index.md", "log.md"];

/// The producer convention for a folder's agent instructions. NOT reserved by
/// the spec, so it is skipped on evidence rather than on its name: see
/// [`check_instructions_file`].
const INSTRUCTIONS_NAME: &str = "INSTRUCTIONS.md";

/// True when `path`'s file name is `index.md` or `log.md` (ASCII
/// case-insensitive), at any depth in the bundle.
pub fn is_reserved(path: &Path) -> bool {
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => RESERVED_NAMES.iter().any(|r| name.eq_ignore_ascii_case(r)),
        None => false,
    }
}

/// What the scan learned about one file with respect to the `INSTRUCTIONS.md`
/// convention.
#[derive(Debug, PartialEq)]
enum InstructionsCheck {
    /// Not an `INSTRUCTIONS.md`, or one that carries a `type`: a concept.
    Concept,
    /// An `INSTRUCTIONS.md` confirmed to carry no `type`.
    Untyped,
    /// An `INSTRUCTIONS.md` that could not be read or parsed, so whether it is
    /// typed is unknown. Carries the failure detail.
    Unresolved(String),
}

/// Classify `path` for the scan. `INSTRUCTIONS.md` is a producer convention,
/// not an OKF reservation: Google's own `okf` package writes one to tell an
/// agent how to edit the folder, and that file has no `type`, so skipping it
/// is right. Skipping it by NAME is not: a bundle whose author wrote a real,
/// typed concept at `INSTRUCTIONS.md` had it silently dropped at import, and
/// knowledge that never enters ingestion is knowledge the user cannot find
/// again. A `type` is the evidence that the file is a concept.
///
/// A file that cannot be read or parsed is no evidence either way. It is
/// [`InstructionsCheck::Unresolved`], never `Untyped`: dropping it from the
/// scan makes sync treat a typed file that is briefly malformed mid-save (or
/// unreadable) as deleted and remove what it had imported.
fn check_instructions_file(
    path: &Path,
    read: impl Fn(&Path) -> std::io::Result<Vec<u8>>,
) -> InstructionsCheck {
    if !path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(INSTRUCTIONS_NAME))
    {
        return InstructionsCheck::Concept;
    }
    match read_frontmatter(path, read) {
        FrontmatterRead::Failed(detail) => InstructionsCheck::Unresolved(detail),
        FrontmatterRead::Absent => InstructionsCheck::Untyped,
        FrontmatterRead::Parsed(frontmatter) => {
            let typed = frontmatter
                .get("type")
                .and_then(scalar_to_string)
                .is_some_and(|okf_type| !okf_type.trim().is_empty());
            if typed {
                InstructionsCheck::Concept
            } else {
                InstructionsCheck::Untyped
            }
        }
    }
}

/// Strip a case-insensitive `.md` suffix, keeping the rest exactly as spelled.
/// `None` when the string does not end in `.md`.
fn strip_md_suffix_case_insensitive(s: &str) -> Option<String> {
    if s.len() < 3 {
        return None;
    }
    let (head, tail) = s.split_at(s.len() - 3);
    if tail.eq_ignore_ascii_case(".md") {
        Some(head.to_string())
    } else {
        None
    }
}

/// Bundle-relative concept id: `abs_path` relative to `bundle_root`, with `/`
/// separators and the (case-insensitive) trailing `.md` removed. `None` when
/// `abs_path` is not under `bundle_root` or is not a `.md` file. Pure path
/// arithmetic, no filesystem access.
pub fn concept_id(bundle_root: &Path, abs_path: &Path) -> Option<String> {
    let is_md = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    if !is_md {
        return None;
    }
    let relative = abs_path.strip_prefix(bundle_root).ok()?;
    let normalized: String = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    strip_md_suffix_case_insensitive(&normalized)
}

// ---------------------------------------------------------------------------
// Concept and frontmatter mapping
// ---------------------------------------------------------------------------

/// One parsed OKF concept file.
#[derive(Debug, Clone, PartialEq)]
pub struct OkfConcept {
    pub concept_id: String,
    pub title: String,
    pub description: Option<String>,
    pub okf_type: Option<String>,
    pub tags: Vec<String>,
    pub status: Option<String>,
    pub stale_after: Option<String>,
    /// Whole frontmatter mapping as JSON, values unchanged ("as parsed").
    /// `{}` when the file has no frontmatter.
    pub frontmatter: serde_json::Value,
    /// Normalized target concept ids from body links and `sources[].resource`,
    /// sorted, deduplicated, excluding this concept's own id.
    pub links: Vec<String>,
}

impl OkfConcept {
    /// `status` says "deprecated" (ASCII case-insensitive, trimmed).
    ///
    /// The scalar `status` is the normal shape. A bundle that writes
    /// `status: [deprecated]` still means it, and `scalar_to_string` drops a
    /// sequence, so the raw frontmatter is consulted as well. Skipping a
    /// deprecated concept is a user ruling, so an odd spelling must not import
    /// it by accident.
    pub fn is_deprecated(&self) -> bool {
        fn says_deprecated(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::String(s) => s.trim().eq_ignore_ascii_case("deprecated"),
                serde_json::Value::Array(items) => items.iter().any(says_deprecated),
                _ => false,
            }
        }
        if self
            .status
            .as_deref()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case("deprecated"))
        {
            return true;
        }
        self.frontmatter.get("status").is_some_and(says_deprecated)
    }
}

/// Convert one YAML scalar to its string form: strings as is, bools and
/// numbers via their YAML scalar text. `Null` and non-scalars (sequences,
/// mappings) are not scalars here.
fn scalar_to_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `tags:` field mapping: a list of scalars becomes a list of strings; a bare
/// string becomes a single tag; anything else is an empty list.
fn concept_tags(frontmatter: &serde_yaml::Value) -> Vec<String> {
    match frontmatter.get("tags") {
        Some(serde_yaml::Value::Sequence(seq)) => seq.iter().filter_map(scalar_to_string).collect(),
        Some(serde_yaml::Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// The first ATX `# ` (level-1) heading in `body`, skipping fenced code
/// blocks. Used as the title fallback when frontmatter carries none.
fn first_atx_heading(body: &str) -> Option<String> {
    let fenced = crate::sources::obsidian::code_block_ranges(body);
    let mut offset = 0;
    for line in body.split('\n') {
        let in_fence = fenced.iter().any(|r| r.contains(&offset));
        if !in_fence {
            let trimmed = line.trim_start();
            if let Some(rest) = trimmed.strip_prefix("# ") {
                let heading = rest.trim();
                if !heading.is_empty() {
                    return Some(heading.to_string());
                }
            }
        }
        offset += line.len() + 1;
    }
    None
}

/// Convert a `serde_yaml::Number` to `serde_json::Value`, matching
/// `serde_yaml::Number`'s own `Serialize` dispatch order (u64, then i64, then
/// f64) so this hand-written path agrees with `serde_json::to_value` of the
/// same `serde_yaml::Value`.
fn yaml_number_to_json(n: &serde_yaml::Number) -> serde_json::Value {
    if let Some(u) = n.as_u64() {
        serde_json::Value::from(u)
    } else if let Some(i) = n.as_i64() {
        serde_json::Value::from(i)
    } else if let Some(f) = n.as_f64() {
        serde_json::Number::from_f64(f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    }
}

/// Convert a whole `serde_yaml::Value` tree to `serde_json::Value`, unchanged
/// in shape: sequences stay arrays, mappings stay objects. A non-string
/// mapping key is converted through its scalar text (matching how
/// `serde_json`'s own serializer turns a non-string map key into a JSON
/// object key), falling back to a debug representation for a non-scalar key.
fn yaml_to_json(value: &serde_yaml::Value) -> serde_json::Value {
    match value {
        serde_yaml::Value::Null => serde_json::Value::Null,
        serde_yaml::Value::Bool(b) => serde_json::Value::Bool(*b),
        serde_yaml::Value::Number(n) => yaml_number_to_json(n),
        serde_yaml::Value::String(s) => serde_json::Value::String(s.clone()),
        serde_yaml::Value::Sequence(seq) => {
            serde_json::Value::Array(seq.iter().map(yaml_to_json).collect())
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = scalar_to_string(k).unwrap_or_else(|| format!("{k:?}"));
                obj.insert(key, yaml_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        serde_yaml::Value::Tagged(tagged) => yaml_to_json(&tagged.value),
    }
}

// ---------------------------------------------------------------------------
// Link normalization
// ---------------------------------------------------------------------------

/// Normalize one link or `sources[].resource` target to a concept id.
/// `from_dir` is the linking concept's folder as a concept-id-style path
/// ("" for the bundle root, "workflows" for workflows/x.md).
pub fn normalize_link_target(from_dir: &str, target: &str) -> Option<String> {
    // 1. Trim; strip one surrounding `<` `>` pair.
    let trimmed = target.trim();
    let unwrapped = match trimmed.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        Some(inner) => inner.trim(),
        None => trimmed,
    };

    // 2. Not a concept link if protocol-relative or carrying a URI scheme.
    if unwrapped.starts_with("//") || URI_SCHEME_RE.is_match(unwrapped) {
        return None;
    }

    // 3. Cut at the first `#` or `?`.
    let cut = unwrapped
        .find(['#', '?'])
        .map(|i| &unwrapped[..i])
        .unwrap_or(unwrapped);
    if cut.is_empty() {
        return None;
    }

    // 4. Leading `/` -> relative to the bundle root; otherwise `from_dir`.
    let (base_dir, rel) = match cut.strip_prefix('/') {
        Some(rest) => ("", rest),
        None => (from_dir, cut),
    };

    // 5. Resolve `.` / `..` segments; a `..` above the bundle root -> None.
    let mut stack: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').filter(|s| !s.is_empty()).collect()
    };
    for segment in rel.split('/') {
        match segment {
            "" | "." => continue,
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }

    // 6. Must end in `.md` (case-insensitive), then strip it.
    let joined = stack.join("/");
    strip_md_suffix_case_insensitive(&joined)
}

/// Byte ranges to skip when scanning for links: fenced code blocks plus
/// inline code spans on top of them.
fn skip_ranges(body: &str) -> Vec<std::ops::Range<usize>> {
    let fenced = crate::sources::obsidian::code_block_ranges(body);
    let inline = crate::export::okf::inline_code_ranges(body, &fenced);
    let mut ranges = fenced;
    ranges.extend(inline);
    ranges
}

/// All normalized link targets in `body` (`[text](target)`, skipping fenced
/// and inline code) plus every `sources[].resource` string in `frontmatter`.
fn extract_links(body: &str, frontmatter: &serde_yaml::Value, from_dir: &str) -> Vec<String> {
    let skip = skip_ranges(body);
    let mut links = Vec::new();

    for cap in LINK_RE.captures_iter(body) {
        let whole = cap.get(0).expect("capture group 0 always matches");
        if skip.iter().any(|r| r.contains(&whole.start())) {
            continue;
        }
        // `![alt](x.md)` is an embed, not a link between concepts. Without this
        // an illustration would mint a real page link.
        if whole.start() > 0 && body.as_bytes()[whole.start() - 1] == b'!' {
            continue;
        }
        let target = &cap[2];
        if let Some(id) = normalize_link_target(from_dir, target) {
            links.push(id);
        }
    }

    if let Some(serde_yaml::Value::Sequence(sources)) = frontmatter.get("sources") {
        for entry in sources {
            if let Some(resource) = entry.get("resource").and_then(|v| v.as_str()) {
                if let Some(id) = normalize_link_target(from_dir, resource) {
                    links.push(id);
                }
            }
        }
    }

    links
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse one concept file's text. `Err(detail)` when a frontmatter block is
/// present but is not valid YAML, or is valid YAML but not a mapping.
/// Returns the concept and the body (text after the frontmatter).
pub fn parse_concept(
    bundle_root: &Path,
    abs_path: &Path,
    content: &str,
) -> Result<(OkfConcept, String), String> {
    let id = concept_id(bundle_root, abs_path).unwrap_or_default();
    let stem = abs_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled")
        .to_string();

    let (frontmatter_yaml, body): (serde_yaml::Value, &str) =
        match crate::sources::obsidian::split_frontmatter(content) {
            Some((yaml_block, body)) => {
                let value: serde_yaml::Value = serde_yaml::from_str(yaml_block)
                    .map_err(|e| format!("invalid YAML frontmatter: {e}"))?;
                match &value {
                    serde_yaml::Value::Mapping(_) => {}
                    _ => return Err("frontmatter is not a mapping".to_string()),
                }
                (value, body)
            }
            None => (
                serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
                content,
            ),
        };

    let title = frontmatter_yaml
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| first_atx_heading(body))
        .unwrap_or(stem);

    let description = frontmatter_yaml
        .get("description")
        .and_then(scalar_to_string);
    let okf_type = frontmatter_yaml.get("type").and_then(scalar_to_string);
    let status = frontmatter_yaml.get("status").and_then(scalar_to_string);
    let stale_after = frontmatter_yaml
        .get("stale_after")
        .and_then(scalar_to_string);
    let tags = concept_tags(&frontmatter_yaml);
    let frontmatter = yaml_to_json(&frontmatter_yaml);

    let from_dir = id.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let mut links = extract_links(body, &frontmatter_yaml, from_dir);
    links.retain(|link| *link != id);
    links.sort();
    links.dedup();

    let concept = OkfConcept {
        concept_id: id,
        title,
        description,
        okf_type,
        tags,
        status,
        stale_after,
        frontmatter,
        links,
    };
    Ok((concept, body.to_string()))
}

// ---------------------------------------------------------------------------
// Documents and the file-level worker entry point
// ---------------------------------------------------------------------------

/// Per-file parse result for OKF ingest, mirroring
/// `directory::FileOutcome` plus the deprecated-concept case.
#[derive(Debug)]
pub enum ConceptOutcome {
    Ingested {
        concept: OkfConcept,
        documents: Vec<RawDocument>,
    },
    Deprecated(OkfConcept),
    Skipped(String),
    Error(String),
}

/// Build chunked `RawDocument`s for a concept body, following
/// `obsidian::note_to_documents` (`obsidian.rs:414-533`): no MOC skip, the
/// concept title replaces the file stem in chunk titles, `space: None` and
/// `memory_type: None`. `source`/`source_agent` are overwritten by
/// `finalize_file_documents`, which also stamps the content hash.
fn build_documents(
    source_id: &str,
    concept: &OkfConcept,
    body: &str,
    path_str: &str,
    mtime: i64,
) -> Vec<RawDocument> {
    let mut metadata = HashMap::new();
    metadata.insert("extension".to_string(), "md".to_string());
    if !concept.tags.is_empty() {
        metadata.insert("tags".to_string(), concept.tags.join(","));
    }

    let engine = ChunkingEngine::new();
    let chunks = engine.chunk(body, &concept.title, path_str, &metadata);

    if chunks.is_empty() {
        let source_id_full = format!("{source_id}::{path_str}::0");
        return vec![RawDocument {
            source: "memory".to_string(),
            source_id: source_id_full,
            title: concept.title.clone(),
            content: body.to_string(),
            last_modified: mtime,
            metadata,
            space: None,
            memory_type: None,
            source_agent: Some("folder".to_string()),
            ..Default::default()
        }];
    }

    chunks
        .iter()
        .enumerate()
        .map(|(i, chunk)| {
            let title = if chunks.len() == 1 {
                concept.title.clone()
            } else {
                match &chunk.semantic_unit {
                    Some(unit) => format!("{} ({})", concept.title, unit),
                    None => format!("{} (chunk {})", concept.title, i),
                }
            };
            let source_id_full = format!("{source_id}::{path_str}::{i}");
            RawDocument {
                source: "memory".to_string(),
                source_id: source_id_full,
                title,
                content: chunk.content.clone(),
                last_modified: mtime,
                metadata: metadata.clone(),
                space: None,
                memory_type: None,
                source_agent: Some("folder".to_string()),
                ..Default::default()
            }
        })
        .collect()
}

/// File-reading wrapper used by the worker (inside `spawn_blocking`). Mirrors
/// `directory::file_to_documents` for `.md`: not `.md` or not a regular file
/// -> Skipped; metadata/read failure -> Error; over `MAX_TEXT_SIZE` ->
/// Skipped with the same "oversized .md file: N bytes exceeds M bytes"
/// wording; decode with `decode_text_bytes`; a file carrying Wenlan's
/// projected-page marker -> Skipped with a reason; parse error -> Error;
/// deprecated -> Deprecated. Otherwise build documents and pass them through
/// `finalize_file_documents(docs, &sha256_hex(&bytes), "md")`, mapping its
/// Ingested/Skipped/Error to Ingested{concept, documents}/Skipped/Error.
pub fn concept_file_to_documents(
    source_id: &str,
    abs_path: &Path,
    bundle_root: &Path,
    knowledge_path: Option<&Path>,
) -> ConceptOutcome {
    let is_md = abs_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"));
    if !is_md {
        return ConceptOutcome::Skipped("unsupported file (not .md)".to_string());
    }

    let metadata = match std::fs::metadata(abs_path) {
        Ok(metadata) => metadata,
        Err(err) => return ConceptOutcome::Error(format!("metadata read failed: {err}")),
    };
    if !metadata.is_file() {
        return ConceptOutcome::Skipped("path is not a regular file".to_string());
    }
    if metadata.len() > crate::sources::directory::MAX_TEXT_SIZE {
        return ConceptOutcome::Skipped(format!(
            "oversized .md file: {} bytes exceeds {} bytes",
            metadata.len(),
            crate::sources::directory::MAX_TEXT_SIZE
        ));
    }

    let bytes = match std::fs::read(abs_path) {
        Ok(bytes) => bytes,
        Err(err) => return ConceptOutcome::Error(format!("read failed: {err}")),
    };
    let content_hash = crate::sources::directory::sha256_hex(&bytes);
    let content = crate::sources::directory::decode_text_bytes(&bytes);

    // Self-recapture exclusion, mirroring `obsidian::note_to_documents`: a
    // file carrying Wenlan's own projected-page marker is never re-ingested.
    // Two detections for the same reason as there: the YAML parse below is
    // fail-open on malformed frontmatter, so a tolerant probe is used first.
    let frontmatter_probe: serde_yaml::Value =
        crate::sources::obsidian::split_frontmatter(&content)
            .and_then(|(yaml, _)| serde_yaml::from_str(yaml).ok())
            .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    let has_marker = frontmatter_probe
        .get(crate::sources::obsidian::PROJECTED_PAGE_FRONTMATTER_KEY)
        .is_some()
        || crate::sources::obsidian::has_projection_marker_line(&content);
    if has_marker {
        return ConceptOutcome::Skipped(format!(
            "carries `{}` frontmatter, so it is one of Wenlan's own projected pages, not a source document",
            crate::sources::obsidian::PROJECTED_PAGE_FRONTMATTER_KEY
        ));
    }

    let (concept, body) = match parse_concept(bundle_root, abs_path, &content) {
        Ok(result) => result,
        Err(detail) => return ConceptOutcome::Error(detail),
    };

    if concept.is_deprecated() {
        return ConceptOutcome::Deprecated(concept);
    }

    let mtime = crate::sources::directory::modified_unix_seconds(&metadata);
    let path_str = crate::sources::directory::provenance_path(abs_path, knowledge_path);
    let docs = build_documents(source_id, &concept, &body, &path_str, mtime);

    match crate::sources::directory::finalize_file_documents(docs, &content_hash, "md") {
        crate::sources::directory::FileOutcome::Ingested(documents) => {
            ConceptOutcome::Ingested { concept, documents }
        }
        crate::sources::directory::FileOutcome::Skipped(reason) => ConceptOutcome::Skipped(reason),
        crate::sources::directory::FileOutcome::Error(detail) => ConceptOutcome::Error(detail),
    }
}

// ---------------------------------------------------------------------------
// Bundle walking (sync and registration)
// ---------------------------------------------------------------------------

/// True when `dir` holds Wenlan's OKF export marker: the folder is a bundle
/// Wenlan wrote, and importing it back would duplicate Wenlan's own pages.
pub fn is_wenlan_export(dir: &Path) -> bool {
    dir.join(crate::export::okf::MARKER_FILE).is_file()
}

/// One scan of a bundle: its concept files, and the files the scan could not
/// classify.
#[derive(Debug, Default, PartialEq)]
pub struct ConceptScan {
    /// `(concept id, absolute path)`, sorted by concept id.
    pub concepts: Vec<(String, std::path::PathBuf)>,
    /// `INSTRUCTIONS.md` files that could not be read or parsed, with the
    /// failure detail, sorted by path. They are neither concepts nor
    /// confirmed untyped, so a caller must neither import nor delete them.
    pub unresolved: Vec<(std::path::PathBuf, String)>,
}

/// The bundle's concept files as `(concept id, absolute path)`, sorted by
/// concept id: [`scan_bundle`] without the unresolved files.
pub fn scan_concepts(bundle_root: &Path) -> Vec<(String, std::path::PathBuf)> {
    scan_bundle(bundle_root).concepts
}

/// Scan the bundle. Starts from `directory::scan_directory` (no hidden files,
/// symlinks or oversized files) and keeps `.md` files that are not reserved
/// names, are not an untyped `INSTRUCTIONS.md`, and do not sit under a
/// subfolder holding a Wenlan export marker. An `INSTRUCTIONS.md` that cannot
/// be read or parsed is reported in [`ConceptScan::unresolved`] instead of
/// being dropped as untyped.
pub fn scan_bundle(bundle_root: &Path) -> ConceptScan {
    scan_bundle_with(bundle_root, |path| std::fs::read(path))
}

fn scan_bundle_with(
    bundle_root: &Path,
    read: impl Fn(&Path) -> std::io::Result<Vec<u8>>,
) -> ConceptScan {
    let mut exported: HashMap<std::path::PathBuf, bool> = HashMap::new();
    let mut scan = ConceptScan::default();
    for path in crate::sources::directory::scan_directory(bundle_root) {
        if is_reserved(&path) || under_wenlan_export(bundle_root, &path, &mut exported) {
            continue;
        }
        match check_instructions_file(&path, &read) {
            InstructionsCheck::Untyped => {}
            InstructionsCheck::Unresolved(detail) => scan.unresolved.push((path, detail)),
            InstructionsCheck::Concept => {
                if let Some(id) = concept_id(bundle_root, &path) {
                    scan.concepts.push((id, path));
                }
            }
        }
    }
    scan.concepts.sort();
    scan.unresolved.sort();
    scan
}

/// Whether a folder between `bundle_root` (exclusive) and `path` holds the
/// export marker. `memo` caches each folder's answer across one scan.
fn under_wenlan_export(
    bundle_root: &Path,
    path: &Path,
    memo: &mut HashMap<std::path::PathBuf, bool>,
) -> bool {
    let mut dir = path.parent();
    while let Some(current) = dir {
        if current == bundle_root || !current.starts_with(bundle_root) {
            return false;
        }
        let marked = *memo
            .entry(current.to_path_buf())
            .or_insert_with(|| is_wenlan_export(current));
        if marked {
            return true;
        }
        dir = current.parent();
    }
    false
}

/// True when `bytes` parse as a concept whose `status` is deprecated. A file
/// that does not parse is not deprecated: the worker reports its error.
pub fn bytes_are_deprecated_concept(bundle_root: &Path, abs_path: &Path, bytes: &[u8]) -> bool {
    let content = crate::sources::directory::decode_text_bytes(bytes);
    parse_concept(bundle_root, abs_path, &content).is_ok_and(|(concept, _)| concept.is_deprecated())
}

/// Whether a folder looks like an OKF bundle: a root `index.md` whose
/// frontmatter has `okf_version`, or a concept file with a non-empty `type`.
/// Stops reading at the first match.
pub fn has_okf_signal(bundle_root: &Path) -> bool {
    if frontmatter_of(&bundle_root.join("index.md"))
        .is_some_and(|frontmatter| frontmatter.get("okf_version").is_some())
    {
        return true;
    }
    scan_concepts(bundle_root).iter().any(|(_, path)| {
        frontmatter_of(path)
            .and_then(|frontmatter| frontmatter.get("type").and_then(scalar_to_string))
            .is_some_and(|okf_type| !okf_type.trim().is_empty())
    })
}

/// What reading a markdown file's frontmatter established.
#[derive(Debug)]
enum FrontmatterRead {
    /// The file was read and carries no frontmatter (or an empty block).
    Absent,
    /// The file was read and its frontmatter is a YAML mapping.
    Parsed(serde_yaml::Value),
    /// The file could not be read, is over the size cap or not a regular
    /// file, or its frontmatter is not a valid YAML mapping.
    Failed(String),
}

/// Read a markdown file's frontmatter through `read`, keeping "there is none"
/// apart from "it could not be read or parsed".
fn read_frontmatter(
    path: &Path,
    read: impl Fn(&Path) -> std::io::Result<Vec<u8>>,
) -> FrontmatterRead {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) => return FrontmatterRead::Failed(format!("metadata read failed: {err}")),
    };
    if !metadata.is_file() {
        return FrontmatterRead::Failed("path is not a regular file".to_string());
    }
    if metadata.len() > crate::sources::directory::MAX_TEXT_SIZE {
        return FrontmatterRead::Failed(format!(
            "oversized .md file: {} bytes exceeds {} bytes",
            metadata.len(),
            crate::sources::directory::MAX_TEXT_SIZE
        ));
    }
    let bytes = match read(path) {
        Ok(bytes) => bytes,
        Err(err) => return FrontmatterRead::Failed(format!("read failed: {err}")),
    };
    let content = crate::sources::directory::decode_text_bytes(&bytes);
    let Some((yaml, _)) = crate::sources::obsidian::split_frontmatter(&content) else {
        return FrontmatterRead::Absent;
    };
    match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
        Ok(serde_yaml::Value::Null) => FrontmatterRead::Absent,
        Ok(value @ serde_yaml::Value::Mapping(_)) => FrontmatterRead::Parsed(value),
        Ok(_) => FrontmatterRead::Failed("frontmatter is not a mapping".to_string()),
        Err(err) => FrontmatterRead::Failed(format!("invalid YAML frontmatter: {err}")),
    }
}

/// A markdown file's parsed frontmatter, when it is a regular file within the
/// text size cap and its frontmatter is a valid YAML mapping. Callers that
/// must tell "none" from "unreadable" use [`read_frontmatter`].
fn frontmatter_of(path: &Path) -> Option<serde_yaml::Value> {
    match read_frontmatter(path, |path| std::fs::read(path)) {
        FrontmatterRead::Parsed(frontmatter) => Some(frontmatter),
        FrontmatterRead::Absent | FrontmatterRead::Failed(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    // ── Bundle walking ───────────────────────────────────────────────────

    fn write_file(root: &Path, rel: &str, text: &str) -> std::path::PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn scan_concepts_drops_reserved_names_non_markdown_and_exported_subtrees() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(root, "index.md", "---\nokf_version: \"0.2\"\n---\n# Wiki\n");
        write_file(root, "log.md", "# Log\n");
        write_file(root, "INSTRUCTIONS.md", "# How to edit\n");
        write_file(root, "concepts/index.md", "# Concepts\n");
        write_file(
            root,
            "concepts/beta.md",
            "---\ntype: concept\n---\n# Beta\n",
        );
        write_file(root, "notes/alpha.md", "# Alpha\n");
        write_file(root, "notes/alpha.txt", "not markdown\n");
        write_file(root, "exported/pages/page.md", "# A projected page\n");
        write_file(root, "exported/.wenlan-okf-export.json", "{}");

        let found: Vec<String> = scan_concepts(root).into_iter().map(|(id, _)| id).collect();

        assert_eq!(
            found,
            vec!["concepts/beta".to_string(), "notes/alpha".to_string()]
        );
    }

    /// `INSTRUCTIONS.md` is a producer convention, not an OKF reservation. An
    /// untyped one is the producer's own note to an agent and is skipped; a
    /// typed one is a concept its author wrote, and dropping it by name lost
    /// real knowledge at import.
    #[test]
    fn scan_concepts_keeps_a_typed_instructions_file_and_drops_an_untyped_one() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(root, "INSTRUCTIONS.md", "# How to edit this folder\n");
        write_file(
            root,
            "workflows/INSTRUCTIONS.md",
            "---\ntype: workflow\ntitle: \"Onboarding instructions\"\n---\n# Steps\n",
        );

        let found: Vec<String> = scan_concepts(root).into_iter().map(|(id, _)| id).collect();

        assert_eq!(found, vec!["workflows/INSTRUCTIONS".to_string()]);
    }

    /// A typed `INSTRUCTIONS.md` that is momentarily malformed mid-save is not
    /// evidence that the file is untyped. Dropping it from the scan made sync
    /// read it as deleted and remove what it had imported, so the scan reports
    /// it as unresolved instead, and it is a concept again once it parses.
    #[test]
    fn scan_bundle_reports_a_malformed_instructions_file_instead_of_dropping_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let typed = "---\ntype: workflow\n---\n# Steps\n";
        let path = write_file(root, "INSTRUCTIONS.md", typed);
        assert_eq!(scan_bundle(root).concepts.len(), 1);

        write_file(
            root,
            "INSTRUCTIONS.md",
            "---\ntype: [workflow\n---\n# Steps\n",
        );
        let scan = scan_bundle(root);
        assert!(scan.concepts.is_empty());
        assert_eq!(scan.unresolved.len(), 1);
        assert_eq!(scan.unresolved[0].0, path);
        assert!(
            scan.unresolved[0].1.starts_with("invalid YAML frontmatter"),
            "{}",
            scan.unresolved[0].1
        );

        // Valid YAML that is not a mapping is just as malformed for a concept.
        write_file(root, "INSTRUCTIONS.md", "---\nty\n---\n# Steps\n");
        assert_eq!(scan_bundle(root).unresolved.len(), 1);

        write_file(root, "INSTRUCTIONS.md", typed);
        let scan = scan_bundle(root);
        assert_eq!(scan.concepts.len(), 1);
        assert!(scan.unresolved.is_empty());
    }

    /// A file that cannot be read is unresolved as well. The reader is
    /// injected because a permission-based failure cannot be produced when the
    /// tests run as root.
    #[test]
    fn scan_bundle_reports_an_unreadable_instructions_file_instead_of_dropping_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = write_file(
            root,
            "INSTRUCTIONS.md",
            "---\ntype: workflow\n---\n# Steps\n",
        );
        write_file(root, "notes/alpha.md", "# Alpha\n");

        let scan = scan_bundle_with(root, |p| {
            if p == path {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            } else {
                std::fs::read(p)
            }
        });

        assert_eq!(
            scan.concepts
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["notes/alpha"],
            "an unreadable file is not imported, and does not disturb the rest"
        );
        assert_eq!(scan.unresolved.len(), 1);
        assert_eq!(scan.unresolved[0].0, path);
        assert!(
            scan.unresolved[0].1.starts_with("read failed"),
            "{}",
            scan.unresolved[0].1
        );
    }

    /// The other ways a file can fail to be read: a directory where the file
    /// was, and a file over the size cap. Neither is a confirmed untyped file.
    #[test]
    fn check_instructions_file_is_unresolved_for_a_directory_or_an_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let as_directory = root.join("INSTRUCTIONS.md");
        std::fs::create_dir(&as_directory).unwrap();
        assert_eq!(
            check_instructions_file(&as_directory, |p| std::fs::read(p)),
            InstructionsCheck::Unresolved("path is not a regular file".to_string())
        );

        let oversized = write_file(root, "big/INSTRUCTIONS.md", "---\ntype: workflow\n---\n");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&oversized)
            .unwrap();
        file.set_len(crate::sources::directory::MAX_TEXT_SIZE + 1)
            .unwrap();
        assert!(matches!(
            check_instructions_file(&oversized, |p| std::fs::read(p)),
            InstructionsCheck::Unresolved(detail) if detail.starts_with("oversized .md file")
        ));

        let missing = root.join("gone/INSTRUCTIONS.md");
        assert!(matches!(
            check_instructions_file(&missing, |p| std::fs::read(p)),
            InstructionsCheck::Unresolved(detail) if detail.starts_with("metadata read failed")
        ));
    }

    /// Confirmed untyped stays filtered: no frontmatter, an empty block, and a
    /// mapping without a `type` are all readable files that say "not a
    /// concept", and none of them is reported as a failure.
    #[test]
    fn scan_bundle_still_drops_a_confirmed_untyped_instructions_file_quietly() {
        for text in [
            "# How to edit this folder\n",
            "",
            "---\n---\n# Empty block\n",
            "---\ntitle: Notes\n---\n# Not typed\n",
            "---\ntype: \"\"\n---\n# Blank type\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            write_file(dir.path(), "INSTRUCTIONS.md", text);
            assert_eq!(
                scan_bundle(dir.path()),
                ConceptScan::default(),
                "{text:?} is untyped, not unresolved"
            );
        }
    }

    #[test]
    fn a_bundle_that_is_itself_a_wenlan_export_is_recognized_but_still_scans() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_file(root, "pages/page.md", "# A projected page\n");
        assert!(!is_wenlan_export(root));

        write_file(root, ".wenlan-okf-export.json", "{}");
        assert!(is_wenlan_export(root));
        assert_eq!(
            scan_concepts(root).len(),
            1,
            "the root marker is the caller's refusal to make, not the scanner's"
        );
    }

    #[test]
    fn bytes_are_deprecated_only_when_the_concept_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("concepts/gone.md");

        let deprecated = b"---\ntype: concept\nstatus: Deprecated\n---\n# Gone\n";
        assert!(bytes_are_deprecated_concept(root, &path, deprecated));

        let live = b"---\ntype: concept\nstatus: active\n---\n# Here\n";
        assert!(!bytes_are_deprecated_concept(root, &path, live));
        assert!(!bytes_are_deprecated_concept(
            root,
            &path,
            b"# No frontmatter\n"
        ));

        let broken = b"---\ntype: [concept\nstatus: deprecated\n---\n# Broken\n";
        assert!(
            !bytes_are_deprecated_concept(root, &path, broken),
            "an unparseable file is an error to report, not a removal to perform"
        );
    }

    #[test]
    fn the_okf_signal_is_an_index_version_or_one_typed_concept() {
        let dir = tempfile::tempdir().unwrap();

        let plain = dir.path().join("plain");
        write_file(&plain, "note.md", "# Just notes\n\nNo frontmatter here.\n");
        write_file(&plain, "index.md", "---\ntitle: Notes\n---\n# Notes\n");
        assert!(
            !has_okf_signal(&plain),
            "an index without okf_version is not a bundle"
        );

        let versioned = dir.path().join("versioned");
        write_file(
            &versioned,
            "index.md",
            "---\nokf_version: \"0.2\"\n---\n# Wiki\n",
        );
        assert!(has_okf_signal(&versioned));

        let typed = dir.path().join("typed");
        write_file(
            &typed,
            "concepts/idea.md",
            "---\ntype: concept\n---\n# Idea\n",
        );
        assert!(has_okf_signal(&typed));

        let reserved_only = dir.path().join("reserved-only");
        write_file(&reserved_only, "log.md", "---\ntype: log\n---\n# Log\n");
        assert!(
            !has_okf_signal(&reserved_only),
            "a reserved file is never a concept, so it is never the signal"
        );

        let blank_type = dir.path().join("blank-type");
        write_file(&blank_type, "idea.md", "---\ntype: \"  \"\n---\n# Idea\n");
        assert!(!has_okf_signal(&blank_type));
    }

    fn fixtures_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/okf")
    }

    fn openwiki_root() -> PathBuf {
        fixtures_dir().join("openwiki")
    }

    fn extra_root() -> PathBuf {
        fixtures_dir().join("extra")
    }

    // -----------------------------------------------------------------
    // is_reserved
    // -----------------------------------------------------------------

    #[test]
    fn is_reserved_matches_the_two_spec_names_at_root_and_nested() {
        assert!(is_reserved(Path::new("index.md")));
        assert!(is_reserved(Path::new("log.md")));
        // `INSTRUCTIONS.md` is a producer convention, not a spec reservation:
        // it is skipped by `check_instructions_file`, on evidence.
        assert!(!is_reserved(Path::new("INSTRUCTIONS.md")));
        assert!(is_reserved(Path::new("concepts/index.md")));
        assert!(is_reserved(Path::new("a/b/c/log.md")));
        assert!(!is_reserved(Path::new("workflows/INSTRUCTIONS.md")));
    }

    #[test]
    fn is_reserved_is_case_insensitive() {
        assert!(is_reserved(Path::new("INDEX.MD")));
        assert!(is_reserved(Path::new("Log.Md")));
        assert!(!is_reserved(Path::new("instructions.md")));
    }

    #[test]
    fn is_reserved_does_not_match_other_names() {
        assert!(!is_reserved(Path::new("quickstart.md")));
        assert!(!is_reserved(Path::new("concepts/okf-output.md")));
    }

    // -----------------------------------------------------------------
    // concept_id
    // -----------------------------------------------------------------

    #[test]
    fn concept_id_for_nested_path() {
        let root = Path::new("/bundle");
        let abs = Path::new("/bundle/concepts/okf-output.md");
        assert_eq!(
            concept_id(root, abs).as_deref(),
            Some("concepts/okf-output")
        );
    }

    #[test]
    fn concept_id_strips_uppercase_md_case_insensitively() {
        let root = Path::new("/bundle");
        let abs = Path::new("/bundle/concepts/Weird.MD");
        assert_eq!(concept_id(root, abs).as_deref(), Some("concepts/Weird"));
    }

    #[test]
    fn concept_id_none_outside_bundle_root() {
        let root = Path::new("/bundle");
        let abs = Path::new("/elsewhere/concepts/okf-output.md");
        assert_eq!(concept_id(root, abs), None);
    }

    #[test]
    fn concept_id_none_for_non_md_file() {
        let root = Path::new("/bundle");
        let abs = Path::new("/bundle/concepts/okf-output.txt");
        assert_eq!(concept_id(root, abs), None);
    }

    // -----------------------------------------------------------------
    // normalize_link_target
    // -----------------------------------------------------------------

    #[test]
    fn normalize_link_target_trims_and_strips_angle_brackets() {
        assert_eq!(
            normalize_link_target("", "  <./concepts/foo.md>  "),
            Some("concepts/foo".to_string())
        );
    }

    #[test]
    fn normalize_link_target_rejects_uri_scheme() {
        assert_eq!(
            normalize_link_target("", "mailto:someone@example.com"),
            None
        );
        assert_eq!(normalize_link_target("", "https://example.com/x.md"), None);
        assert_eq!(normalize_link_target("", "repo://src/server.ts"), None);
    }

    #[test]
    fn normalize_link_target_rejects_protocol_relative() {
        assert_eq!(normalize_link_target("", "//example.com/path.md"), None);
    }

    #[test]
    fn normalize_link_target_fragment_only_is_none() {
        assert_eq!(normalize_link_target("concepts", "#section"), None);
    }

    #[test]
    fn normalize_link_target_strips_query() {
        assert_eq!(
            normalize_link_target("", "concepts/foo.md?x=1"),
            Some("concepts/foo".to_string())
        );
    }

    #[test]
    fn normalize_link_target_absolute_ignores_from_dir() {
        assert_eq!(
            normalize_link_target("workflows", "/concepts/foo.md"),
            Some("concepts/foo".to_string())
        );
    }

    #[test]
    fn normalize_link_target_relative_uses_from_dir() {
        assert_eq!(
            normalize_link_target("workflows", "concepts/foo.md"),
            Some("workflows/concepts/foo".to_string())
        );
    }

    #[test]
    fn normalize_link_target_resolves_dot_dot() {
        assert_eq!(
            normalize_link_target("concepts", "../sibling.md"),
            Some("sibling".to_string())
        );
    }

    #[test]
    fn normalize_link_target_dot_dot_above_root_is_none() {
        assert_eq!(normalize_link_target("concepts", "../../outside.md"), None);
    }

    #[test]
    fn normalize_link_target_requires_md_suffix() {
        assert_eq!(normalize_link_target("", "concepts/foo"), None);
    }

    // -----------------------------------------------------------------
    // Real fixture: concepts/okf-output.md
    // -----------------------------------------------------------------

    #[test]
    fn parses_real_okf_output_fixture() {
        let root = openwiki_root();
        let abs = root.join("concepts/okf-output.md");
        let content = fs::read_to_string(&abs).unwrap();
        let (concept, _body) = parse_concept(&root, &abs, &content).unwrap();

        assert_eq!(concept.okf_type.as_deref(), Some("concept"));
        assert!(!concept.tags.is_empty());
        assert!(concept.frontmatter["verified"].is_array());
        assert!(concept.frontmatter["generated"].is_object());

        // Structural equality against an independent yaml->json conversion of
        // the same frontmatter block.
        let (yaml_block, _) = crate::sources::obsidian::split_frontmatter(&content).unwrap();
        let independent: serde_yaml::Value = serde_yaml::from_str(yaml_block).unwrap();
        let independent_json = serde_json::to_value(&independent).unwrap();
        assert_eq!(concept.frontmatter, independent_json);
    }

    // -----------------------------------------------------------------
    // extra/concepts/link-cases.md
    // -----------------------------------------------------------------

    #[test]
    fn link_cases_fixture_produces_expected_links() {
        let root = extra_root();
        let abs = root.join("concepts/link-cases.md");
        let content = fs::read_to_string(&abs).unwrap();
        let (concept, _body) = parse_concept(&root, &abs, &content).unwrap();

        assert!(concept.frontmatter["verified"].is_object());

        let expected = vec![
            "Workflows/Wiki-Finalization".to_string(),
            "concepts/does-not-exist".to_string(),
            "concepts/okf-output".to_string(),
            "workflows/wiki-finalization".to_string(),
        ];
        assert_eq!(concept.links, expected);
    }

    // -----------------------------------------------------------------
    // Malformed / missing frontmatter
    // -----------------------------------------------------------------

    #[test]
    fn malformed_yaml_frontmatter_is_err() {
        let root = PathBuf::from("/bundle");
        let abs = root.join("broken.md");
        let content = "---\ntitle: \"unterminated\n---\nBody text here.\n";
        assert!(parse_concept(&root, &abs, content).is_err());
    }

    #[test]
    fn malformed_yaml_frontmatter_makes_concept_file_to_documents_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.md");
        fs::write(&path, "---\ntitle: \"unterminated\n---\nBody text here.\n").unwrap();
        let outcome = concept_file_to_documents("okf-test", &path, dir.path(), None);
        assert!(matches!(outcome, ConceptOutcome::Error(_)));
    }

    #[test]
    fn yaml_list_frontmatter_is_err() {
        let root = PathBuf::from("/bundle");
        let abs = root.join("list.md");
        let content = "---\n- item1\n- item2\n---\nBody.\n";
        assert!(parse_concept(&root, &abs, content).is_err());
    }

    #[test]
    fn no_frontmatter_ok_with_empty_map_and_h1_title() {
        let root = PathBuf::from("/bundle");
        let abs = root.join("plain.md");
        let content = "# My Title\n\nSome body text.\n";
        let (concept, _body) = parse_concept(&root, &abs, content).unwrap();
        assert_eq!(concept.frontmatter, serde_json::json!({}));
        assert_eq!(concept.title, "My Title");
    }

    #[test]
    fn no_frontmatter_no_h1_falls_back_to_stem() {
        let root = PathBuf::from("/bundle");
        let abs = root.join("plain.md");
        let content = "Just a paragraph with no heading.\n";
        let (concept, _body) = parse_concept(&root, &abs, content).unwrap();
        assert_eq!(concept.title, "plain");
    }

    // -----------------------------------------------------------------
    // concept_file_to_documents
    // -----------------------------------------------------------------

    #[test]
    fn deprecated_fixture_is_deprecated_outcome() {
        let root = extra_root();
        let abs = root.join("concepts/deprecated-note.md");
        let outcome = concept_file_to_documents("okf-test", &abs, &root, None);
        assert!(matches!(outcome, ConceptOutcome::Deprecated(_)));
    }

    #[test]
    fn a_list_valued_status_still_deprecates_the_concept() {
        // `scalar_to_string` drops a sequence, so `status` parses as None.
        // Skipping a deprecated concept is a user ruling, so the odd spelling
        // must not import it anyway.
        let root = std::path::Path::new("/bundle");
        let abs = root.join("concepts/listed.md");
        let (concept, _) = parse_concept(
            root,
            &abs,
            "---\ntype: concept\nstatus:\n  - deprecated\n---\n\n# Listed\n\nBody.\n",
        )
        .expect("parses");
        assert_eq!(concept.status, None, "a sequence is not a scalar status");
        assert!(
            concept.is_deprecated(),
            "a list-valued status still says deprecated"
        );
    }

    #[test]
    fn an_image_embed_is_not_a_concept_link() {
        let root = std::path::Path::new("/bundle");
        let abs = root.join("concepts/alpha.md");
        let (concept, _) = parse_concept(
            root,
            &abs,
            "---\ntype: concept\n---\n\n# Alpha\n\n\
             An illustration ![diagram](arch.md) and a real link [Beta](beta.md).\n",
        )
        .expect("parses");
        assert_eq!(
            concept.links,
            vec!["concepts/beta".to_string()],
            "the embed is an illustration, not a link between concepts"
        );
    }

    #[test]
    fn stale_fixture_is_ingested_with_folder_provenance() {
        let root = extra_root();
        let abs = root.join("concepts/stale-note.md");
        let outcome = concept_file_to_documents("okf-test", &abs, &root, None);
        match outcome {
            ConceptOutcome::Ingested { documents, .. } => {
                assert!(!documents.is_empty());
                for doc in &documents {
                    assert_eq!(doc.source_agent.as_deref(), Some("folder"));
                    assert_eq!(doc.space, None);
                    assert_eq!(doc.memory_type, None);
                    assert!(doc.content_hash.as_deref().is_some_and(|h| !h.is_empty()));
                }
            }
            other => panic!("expected Ingested, got {other:?}"),
        }
    }

    #[test]
    fn oversized_md_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.md");
        let mut file = fs::File::create(&path).unwrap();
        let chunk = "a".repeat(1024);
        for _ in 0..1130 {
            file.write_all(chunk.as_bytes()).unwrap();
        }
        drop(file);

        let outcome = concept_file_to_documents("okf-test", &path, dir.path(), None);
        match outcome {
            ConceptOutcome::Skipped(reason) => assert!(reason.contains("oversized")),
            other => panic!("expected Skipped(oversized), got {other:?}"),
        }
    }

    #[test]
    fn projected_page_marker_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("projected.md");
        fs::write(&path, "---\norigin_id: some-page-id\n---\nBody text.\n").unwrap();
        let outcome = concept_file_to_documents("okf-test", &path, dir.path(), None);
        assert!(matches!(outcome, ConceptOutcome::Skipped(_)));
    }
}
