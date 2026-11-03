// SPDX-License-Identifier: Apache-2.0
//! "What you're working on", read straight from the conversations people
//! have with their AI tools (UX-THING-1), so nobody has to remember to tell
//! Wenlan.
//!
//! Three steps, each cheaper than the next:
//! 1. [`scan`] finds the recent conversations Claude Code and Codex keep on
//!    this computer and reads what the person typed. The transcripts stay
//!    where the tools keep them; this only reads.
//! 2. [`keep_work`] drops what is plainly not a piece of work, by rules:
//!    runs a program started on its own, conversations with nothing typed or
//!    a single question, and the fixed prompts pasted into many
//!    conversations.
//! 3. A model reads each remaining conversation once ([`label_prompt`]):
//!    is it work, what is it called, where does it stand. A stronger model
//!    then puts the labelled conversations into things ([`group_prompt`]);
//!    only the one-line labels are sent for that, never the conversation.

use std::collections::{HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use wenlan_types::session_context::TranscriptRoots;

/// A conversation counts while its last line is at most this old: the same
/// week a thing stays on home (UX-HOME-5).
pub const INTAKE_WINDOW_SECS: i64 = wenlan_types::THING_QUIET_AFTER_SECS;
/// A line longer than this is a pasted file or log, not something said.
const MAX_JSONL_LINE_BYTES: usize = 4 * 1024 * 1024;
/// A transcript longer than this is read from its end only.
const MAX_TRANSCRIPT_BYTES: u64 = 32 * 1024 * 1024;
/// Text kept from each thing the person typed.
const MAX_SAID_CHARS: usize = 2_000;
/// Things the person typed kept from one conversation (the first ones and
/// the latest one are what the model reads).
const MAX_SAID: usize = 40;
/// Text kept from the AI's last reply.
const MAX_REPLY_CHARS: usize = 1_000;
/// Folders under the transcript roots looked at, at most.
const MAX_DIRS: usize = 2_000;
/// A line at least this long that shows up in this many conversations is a
/// fixed prompt or a template, not what a conversation is about.
const TEMPLATE_LINE_CHARS: usize = 20;
const TEMPLATE_LINE_SEEN: usize = 3;
/// What is left of a conversation once its template lines are gone must be
/// at least this long to count as the person's own words.
const OWN_WORDS_CHARS: usize = 15;

/// One conversation found in an AI tool's transcripts.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundConversation {
    /// `claude-code` or `codex`, as in `agent_sessions.tool`.
    pub tool: String,
    /// The tool's own id for the conversation, as in
    /// `agent_sessions.external_id`.
    pub external_id: String,
    pub path: PathBuf,
    /// Size of the transcript when read; it grows while the conversation goes on.
    pub len: u64,
    /// The folder the conversation ran in, the one it ran in most.
    pub cwd: Option<String>,
    pub branch: Option<String>,
    /// What the person typed, oldest first, without what the tool injected.
    pub said: Vec<String>,
    /// The end of the AI's last reply.
    pub last_reply: String,
    pub first_at: i64,
    /// The conversation's own last time. A file can be touched long after.
    pub last_at: i64,
    /// Started by a program, or a sub-task of another conversation.
    pub automated: bool,
    /// Typed by an assistant the person asked to run this for them (Claude
    /// projects and Remote Control), so the words are not the person's own
    /// but the work is.
    pub on_behalf: bool,
}

/// Why conversations were left out, for the log.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub automated: usize,
    pub nothing_typed: usize,
    pub one_message: usize,
    pub fixed_prompt: usize,
}

/// The conversations whose last line is at most `window` seconds before
/// `now`, newest first.
pub fn scan(roots: &TranscriptRoots, now: i64, window: i64) -> Vec<FoundConversation> {
    let since = now - window;
    let mut found = Vec::new();
    if let Some(dir) = &roots.claude_dir {
        for path in recent_files(&dir.join("projects"), 1, since) {
            if let Some(conv) = read_claude_code(&path).filter(|c| c.last_at >= since) {
                found.push(conv);
            }
        }
    }
    if let Some(dir) = &roots.codex_dir {
        for path in recent_files(&dir.join("sessions"), 4, since) {
            if let Some(conv) = read_codex(&path).filter(|c| c.last_at >= since) {
                found.push(conv);
            }
        }
    }
    found.sort_by(|a, b| b.last_at.cmp(&a.last_at).then(a.path.cmp(&b.path)));
    found
}

/// `.jsonl` files at most `depth` folders below `root` changed since `since`.
fn recent_files(root: &Path, depth: usize, since: i64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![(root.to_path_buf(), 0usize)];
    let mut seen = 0usize;
    while let Some((dir, level)) = dirs.pop() {
        seen += 1;
        if seen > MAX_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if level < depth {
                    dirs.push((path, level + 1));
                }
            } else if kind.is_file()
                && path.extension().is_some_and(|e| e == "jsonl")
                && modified_secs(&path).is_some_and(|m| m >= since)
            {
                out.push(path);
            }
        }
    }
    out
}

fn modified_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(secs).ok()
}

/// A transcript's lines, read one at a time.
type Lines = Box<dyn Iterator<Item = Vec<u8>>>;

/// The lines of a transcript, the end only when it is long.
fn transcript_lines(path: &Path) -> Option<(u64, Lines)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let reader: Box<dyn BufRead> = if len <= MAX_TRANSCRIPT_BYTES {
        Box::new(std::io::BufReader::new(file))
    } else {
        file.seek(SeekFrom::Start(len - MAX_TRANSCRIPT_BYTES))
            .ok()?;
        let mut reader = std::io::BufReader::new(file.take(MAX_TRANSCRIPT_BYTES));
        let mut cut = Vec::new();
        reader.read_until(b'\n', &mut cut).ok()?;
        Box::new(reader)
    };
    let lines = reader
        .split(b'\n')
        .map_while(Result::ok)
        .filter(|raw| raw.len() <= MAX_JSONL_LINE_BYTES);
    Some((len, Box::new(lines)))
}

fn unix_seconds(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|t| t.timestamp())
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

fn cut_end(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    text.chars().skip(count - max).collect()
}

/// Text a tool put in the person's turn on its own: slash-command echoes,
/// command output, reminders, interruption notes, pasted instructions files.
fn injected(text: &str) -> bool {
    if text.starts_with("[Request interrupted")
        || text.starts_with("Caveat: ")
        || text.starts_with("# AGENTS.md")
    {
        return true;
    }
    let Some(rest) = text.strip_prefix('<') else {
        return false;
    };
    let tag: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    !tag.is_empty() && rest[tag.len()..].starts_with(['>', ' ', '\n'])
}

/// Tags a tool wraps around text inside a turn; the text in them is kept and
/// the tags dropped, for turns typed on the person's behalf.
fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        match after.find('>') {
            Some(close) if close <= 200 => rest = &after[close + 1..],
            _ => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[derive(Deserialize)]
struct ClaudeEntry {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, rename = "isMeta")]
    is_meta: bool,
    #[serde(default, rename = "isSidechain")]
    is_sidechain: bool,
    #[serde(default, rename = "isCompactSummary")]
    is_compact_summary: bool,
    #[serde(default, rename = "projectsUserTurn")]
    projects_user_turn: Option<serde_json::Value>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    entrypoint: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    content: Option<ClaudeContent>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ClaudeContent {
    Text(String),
    Blocks(Vec<ClaudeBlock>),
}

#[derive(Deserialize)]
struct ClaudeBlock {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

impl ClaudeContent {
    fn texts(self) -> Vec<String> {
        match self {
            Self::Text(text) => vec![text],
            Self::Blocks(blocks) => blocks
                .into_iter()
                .filter(|b| b.kind.as_deref() == Some("text"))
                .filter_map(|b| b.text)
                .collect(),
        }
    }
}

/// One Claude Code transcript (`~/.claude/projects/<folder>/<id>.jsonl`).
pub fn read_claude_code(path: &Path) -> Option<FoundConversation> {
    let (len, lines) = transcript_lines(path)?;
    let mut entries = Vec::new();
    for raw in lines {
        if let Ok(entry) = serde_json::from_slice::<ClaudeEntry>(&raw) {
            entries.push(entry);
        }
    }
    let on_behalf = entries.iter().any(|e| e.projects_user_turn.is_some());
    let mut cwds: HashMap<String, usize> = HashMap::new();
    let (mut first_at, mut last_at) = (None::<i64>, None::<i64>);
    let (mut said, mut last_reply) = (Vec::new(), String::new());
    let (mut automated, mut branch, mut id) = (false, None, None);
    for entry in entries {
        if let Some(at) = entry.timestamp.as_deref().and_then(unix_seconds) {
            first_at = Some(first_at.map_or(at, |f| f.min(at)));
            last_at = Some(last_at.map_or(at, |l| l.max(at)));
        }
        if let Some(cwd) = entry.cwd.filter(|c| !c.is_empty()) {
            *cwds.entry(cwd).or_default() += 1;
        }
        if entry
            .entrypoint
            .as_deref()
            .is_some_and(|e| e.starts_with("sdk"))
            || entry.is_sidechain
        {
            automated = true;
        }
        branch = entry.git_branch.filter(|b| !b.is_empty()).or(branch);
        id = id.or(entry.session_id);
        if entry.is_meta || entry.is_compact_summary || entry.is_sidechain {
            continue;
        }
        let Some(content) = entry.message.and_then(|m| m.content) else {
            continue;
        };
        match entry.kind.as_deref() {
            Some("user") => {
                // A turn's first text block is what was typed; the rest are
                // attachments and reminders.
                let typed = content.texts().into_iter().find_map(|text| {
                    let text = if on_behalf {
                        strip_tags(&text)
                    } else {
                        text.trim().to_string()
                    };
                    (!text.is_empty() && !injected(&text)).then_some(text)
                });
                if let Some(text) = typed {
                    said.push(cut(&text, MAX_SAID_CHARS));
                }
            }
            Some("assistant") => {
                if let Some(text) = content
                    .texts()
                    .into_iter()
                    .rev()
                    .find(|t| !t.trim().is_empty())
                {
                    last_reply = cut_end(text.trim(), MAX_REPLY_CHARS);
                }
            }
            _ => {}
        }
    }
    let external_id = id.or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()))?;
    let cwd = cwds
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(c, _)| c);
    let fallback = modified_secs(path)?;
    Some(FoundConversation {
        tool: "claude-code".into(),
        external_id,
        path: path.to_path_buf(),
        len,
        cwd,
        branch: branch.filter(|b| b != "HEAD"),
        said: keep_ends(said),
        last_reply,
        first_at: first_at.unwrap_or(fallback),
        last_at: last_at.unwrap_or(fallback),
        automated,
        on_behalf,
    })
}

#[derive(Deserialize)]
struct CodexEntry {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(default)]
    payload: serde_json::Value,
}

/// One Codex transcript (`~/.codex/sessions/YYYY/MM/DD/rollout-…jsonl`).
pub fn read_codex(path: &Path) -> Option<FoundConversation> {
    let (len, lines) = transcript_lines(path)?;
    let (mut first_at, mut last_at) = (None::<i64>, None::<i64>);
    let (mut said, mut last_reply) = (Vec::<String>::new(), String::new());
    let (mut automated, mut cwd, mut branch, mut id) = (false, None, None, None);
    let mut seen_meta = false;
    let text_of = |value: &serde_json::Value| value.as_str().map(str::trim).map(str::to_string);
    let typed = |text: Option<String>, said: &mut Vec<String>| {
        let Some(text) = text.filter(|t| !t.is_empty() && !injected(t)) else {
            return;
        };
        // Older Codex records a turn twice: as an event and as a message.
        if said.last().map(String::as_str) != Some(cut(&text, MAX_SAID_CHARS).as_str()) {
            said.push(cut(&text, MAX_SAID_CHARS));
        }
    };
    for raw in lines {
        let Ok(entry) = serde_json::from_slice::<CodexEntry>(&raw) else {
            continue;
        };
        if let Some(at) = entry.timestamp.as_deref().and_then(unix_seconds) {
            first_at = Some(first_at.map_or(at, |f| f.min(at)));
            last_at = Some(last_at.map_or(at, |l| l.max(at)));
        }
        let p = &entry.payload;
        match (
            entry.kind.as_deref(),
            p.get("type").and_then(|t| t.as_str()),
        ) {
            // The file's own session comes first; later ones describe the
            // sessions it started.
            (Some("session_meta"), _) if !seen_meta => {
                seen_meta = true;
                id = text_of(&p["id"]);
                cwd = text_of(&p["cwd"]);
                branch = text_of(&p["git"]["branch"]);
                let origin = text_of(&p["originator"]);
                let source = text_of(&p["source"]);
                // Run by a program (`codex exec`) or a sub-agent another
                // conversation started: its prompt is not the person's.
                if origin.as_deref() == Some("codex_exec")
                    || source.as_deref() == Some("exec")
                    || p["source"].get("subagent").is_some()
                {
                    automated = true;
                }
            }
            (Some("event_msg"), Some("user_message")) => typed(text_of(&p["message"]), &mut said),
            (Some("response_item"), Some("message")) if p["role"] == "user" => {
                let first = p["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| c["type"] == "input_text" || c["type"] == "text")
                    .find_map(|c| text_of(&c["text"]).filter(|t| !t.is_empty()));
                typed(first, &mut said);
            }
            (Some("response_item"), Some("message")) if p["role"] == "assistant" => {
                let reply = p["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| text_of(&c["text"]))
                    .rfind(|t| !t.is_empty());
                if let Some(reply) = reply {
                    last_reply = cut_end(&reply, MAX_REPLY_CHARS);
                }
            }
            _ => {}
        }
    }
    let external_id = id?;
    let fallback = modified_secs(path)?;
    Some(FoundConversation {
        tool: "codex".into(),
        external_id,
        path: path.to_path_buf(),
        len,
        cwd,
        branch,
        said: keep_ends(said),
        last_reply,
        first_at: first_at.unwrap_or(fallback),
        last_at: last_at.unwrap_or(fallback),
        automated,
        on_behalf: false,
    })
}

/// The first messages and the latest one: what the model reads.
fn keep_ends(mut said: Vec<String>) -> Vec<String> {
    if said.len() > MAX_SAID {
        let last = said.pop();
        said.truncate(MAX_SAID - 1);
        said.extend(last);
    }
    said
}

/// The conversations worth a model's look, and why the rest were left out.
/// Template lines (a line of at least 20 characters found in three or more
/// conversations) are taken out of what the person said first, so a fixed
/// prompt pasted into every conversation is not mistaken for its topic.
pub fn keep_work(found: Vec<FoundConversation>) -> (Vec<FoundConversation>, Dropped) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for conv in &found {
        let lines: HashSet<&str> = conv
            .said
            .iter()
            .flat_map(|s| s.lines())
            .map(str::trim)
            .filter(|l| l.chars().count() >= TEMPLATE_LINE_CHARS)
            .collect();
        for line in lines {
            *seen.entry(line.to_string()).or_default() += 1;
        }
    }
    let mut dropped = Dropped::default();
    let mut kept = Vec::new();
    for mut conv in found {
        let own: Vec<String> = conv
            .said
            .iter()
            .map(|s| {
                s.lines()
                    .filter(|l| {
                        let l = l.trim();
                        l.chars().count() < TEMPLATE_LINE_CHARS
                            || seen.get(l).copied().unwrap_or(0) < TEMPLATE_LINE_SEEN
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string()
            })
            .collect();
        let templated =
            own != conv.said && !own.iter().any(|s| s.chars().count() >= OWN_WORDS_CHARS);
        conv.said = own.into_iter().filter(|s| !s.is_empty()).collect();
        if conv.automated {
            dropped.automated += 1;
        } else if templated {
            dropped.fixed_prompt += 1;
        } else if conv.said.is_empty() {
            dropped.nothing_typed += 1;
        } else if conv.said.len() == 1 && !conv.on_behalf {
            dropped.one_message += 1;
        } else {
            kept.push(conv);
        }
    }
    (kept, dropped)
}

/// What the labelling model says about one conversation.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Label {
    /// Part of a piece of work the person would want to see again.
    pub work: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub space: String,
    #[serde(default)]
    pub about: String,
    #[serde(default)]
    pub status: String,
}

/// Characters of each of the first messages the labelling model reads.
const OPENING_CHARS: usize = 600;
/// Characters of the latest message it reads.
const LATEST_CHARS: usize = 300;
/// Characters of the AI's last reply it reads.
const REPLY_CHARS: usize = 500;
/// Longest stored title, about and status, in characters.
pub const MAX_TITLE_CHARS: usize = 40;
pub const MAX_ABOUT_CHARS: usize = 200;
pub const MAX_STATUS_CHARS: usize = 120;

const KEEP_NAMES: &str = "Keep product and project names exactly as written in the \
conversation (for example Wenlan, Heptabase); do not translate or transliterate them.";

/// The language every title and line is written in.
pub fn language_name(locale: Option<&str>) -> &'static str {
    let lower = locale.unwrap_or("").to_ascii_lowercase().replace('_', "-");
    if lower.starts_with("zh-hant") || lower == "zh-tw" || lower == "zh-hk" {
        "Traditional Chinese"
    } else if lower.starts_with("zh") {
        "Simplified Chinese"
    } else if lower.starts_with("ja") {
        "Japanese"
    } else if lower.starts_with("es") {
        "Spanish"
    } else {
        "English"
    }
}

/// The folder name a conversation ran in, or `None` for a home folder.
pub fn folder_name(cwd: Option<&str>) -> Option<String> {
    let name = Path::new(cwd?).file_name()?.to_string_lossy().into_owned();
    (!name.is_empty()).then_some(name)
}

/// The repository a folder is in: the folder holding `.git`, or for a
/// worktree the main checkout it belongs to. Reads no more than the folders
/// above `cwd`; runs no program.
pub fn repo_root_of(cwd: &str) -> Option<String> {
    let mut dir = Some(Path::new(cwd));
    while let Some(d) = dir {
        let git = d.join(".git");
        if git.is_dir() {
            return Some(d.to_string_lossy().into_owned());
        }
        if git.is_file() {
            let pointer = std::fs::read_to_string(&git).unwrap_or_default();
            let main = pointer
                .trim()
                .strip_prefix("gitdir:")
                .and_then(|g| {
                    g.trim()
                        .split("/.git/worktrees/")
                        .next()
                        .map(str::to_string)
                })
                .filter(|m| !m.is_empty() && !m.ends_with(".git"));
            return Some(main.unwrap_or_else(|| d.to_string_lossy().into_owned()));
        }
        dir = d.parent();
    }
    None
}

/// The prompt that labels one conversation. `spaces` are the person's
/// Spaces; `language` comes from [`language_name`].
pub fn label_prompt(conv: &FoundConversation, spaces: &[String], language: &str) -> String {
    let opening = conv
        .said
        .iter()
        .take(3)
        .map(|s| cut(s, OPENING_CHARS))
        .collect::<Vec<_>>()
        .join("\n---\n");
    let latest = if conv.said.len() > 3 {
        conv.said
            .last()
            .map(|s| cut(s, LATEST_CHARS))
            .unwrap_or_default()
    } else {
        "(same as above)".into()
    };
    let reply = if conv.last_reply.is_empty() {
        "(none)".to_string()
    } else {
        cut_end(&conv.last_reply, REPLY_CHARS)
    };
    let who = if conv.on_behalf {
        "These are instructions the person had an assistant carry out for them; treat the task as the person's own work."
    } else {
        "What the person typed:"
    };
    let folder = folder_name(conv.cwd.as_deref()).unwrap_or_else(|| "no folder".into());
    format!(
        "A conversation with an AI tool, in folder \"{folder}\". {who}\n{opening}\n\n\
Latest message: {latest}\nAssistant's last reply (end): {reply}\n\n\
Spaces the person uses: {spaces}\n\n\
Is this part of a piece of work the person would want to see later? Say false for a quick \
question, a greeting, a test of the tool, a one-off lookup, or a one-off setup or cleanup.\n\
Give, in {language}: \"title\" (at most 12 words or 12 Chinese characters; name the larger goal \
this serves, not this step), \"space\" (exactly one of the spaces above, or \"\" if none fits), \
\"about\" (one sentence), \"status\" (one short sentence: where it stands now). {KEEP_NAMES}\n\
Answer with one line of JSON only: {{\"work\": true|false, \"title\": string, \"space\": string, \
\"about\": string, \"status\": string}}",
        spaces = if spaces.is_empty() {
            "(none)".to_string()
        } else {
            spaces.join(", ")
        },
    )
}

/// The first JSON object in a model's answer, after any reasoning it wrote.
fn json_object(text: &str) -> Option<&str> {
    let text = match text.rfind("</think>") {
        Some(end) => &text[end + "</think>".len()..],
        None => text,
    };
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// The label in a model's answer. A space that is not one of the person's
/// Spaces is dropped; a work label without a title is not work.
pub fn parse_label(text: &str, spaces: &[String]) -> Option<Label> {
    let mut label: Label = serde_json::from_str(json_object(text)?).ok()?;
    label.title = cut(label.title.trim(), MAX_TITLE_CHARS);
    label.about = cut(label.about.trim(), MAX_ABOUT_CHARS);
    label.status = cut(label.status.trim(), MAX_STATUS_CHARS);
    let space = label.space.trim();
    label.space = spaces
        .iter()
        .find(|s| s.as_str() == space)
        .cloned()
        .unwrap_or_default();
    if label.title.is_empty() {
        label.work = false;
    }
    Some(label)
}

/// A labelled conversation waiting to be put into a thing.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub session_id: String,
    pub title: String,
    pub about: String,
    pub space: String,
}

/// A thing already on the list, as the grouping model sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenThing {
    pub id: String,
    pub title: String,
    pub about: String,
    pub space: String,
}

/// The prompt that puts labelled conversations into things: only titles and
/// one-line abouts, never the conversations.
pub fn group_prompt(pending: &[Pending], open: &[OpenThing], language: &str) -> String {
    let things = if open.is_empty() {
        "(none yet)".to_string()
    } else {
        open.iter()
            .enumerate()
            .map(|(i, t)| format!("T{i}. {} [{}]: {}", t.title, t.space, t.about))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let convs = pending
        .iter()
        .enumerate()
        .map(|(i, p)| format!("C{i}. {} [{}]: {}", p.title, p.space, p.about))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "A person's pieces of work so far (id, title, [space], about):\n{things}\n\n\
New conversations they had with AI tools (id, title, [space], about):\n{convs}\n\n\
Put every new conversation into a piece of work, or leave it out. A piece is a goal the \
person is working toward (a product feature, a release, a launch, a paper, a trip), not a \
step of it: a review, a test run, a cleanup or a check done for a goal belongs to that goal. \
A conversation joins an existing piece when it serves the same goal; a shared product, \
folder or space alone is not enough. Conversations that serve the same new goal form one new \
piece together. Leave out a conversation that is only a small chore with no goal of its own \
(confirming a merge, sending a file, a one-off cleanup) and that fits no piece.\n\
Give each new piece a title in {language} of at most 12 words or 12 Chinese characters that \
names the goal. {KEEP_NAMES}\n\
Answer with JSON only: {{\"join\": [{{\"conversation\": \"C0\", \"piece\": \"T3\"}}], \
\"new\": [{{\"title\": string, \"conversations\": [\"C1\", \"C2\"]}}], \
\"leave_out\": [\"C4\"]}} and use every conversation exactly once."
    )
}

/// Where the grouping model put the conversations: indices into the
/// `pending` and `open` lists given to [`group_prompt`].
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Grouping {
    /// `(pending index, open-thing index)`.
    pub joins: Vec<(usize, usize)>,
    /// A new thing's title and its pending indices.
    pub new: Vec<(String, Vec<usize>)>,
    /// Pending indices that are no piece of work: small chores.
    pub left_out: Vec<usize>,
}

/// A group bigger than this swallowed most of a batch: the failure a weak
/// model makes, not a real piece of work. Its conversations stay apart.
pub const MAX_NEW_GROUP: usize = 15;

/// The grouping in a model's answer. Ids that do not exist or come twice are
/// ignored; a conversation placed in a piece is not also left out, and one
/// the model forgot becomes a thing of its own, titled by its label.
pub fn parse_grouping(text: &str, pending: &[Pending], open: usize) -> Option<Grouping> {
    #[derive(Deserialize)]
    struct Join {
        conversation: String,
        piece: String,
    }
    #[derive(Deserialize)]
    struct New {
        #[serde(default)]
        title: String,
        #[serde(default)]
        conversations: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        join: Vec<Join>,
        #[serde(default)]
        new: Vec<New>,
        #[serde(default)]
        leave_out: Vec<String>,
    }
    let answer: Answer = serde_json::from_str(json_object(text)?).ok()?;
    let index = |id: &str, prefix: char, len: usize| {
        id.trim()
            .strip_prefix(prefix)
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| *n < len)
    };
    let mut used = HashSet::new();
    let mut grouping = Grouping::default();
    for join in answer.join {
        let (Some(c), Some(t)) = (
            index(&join.conversation, 'C', pending.len()),
            index(&join.piece, 'T', open),
        ) else {
            continue;
        };
        if used.insert(c) {
            grouping.joins.push((c, t));
        }
    }
    for new in answer.new {
        let members: Vec<usize> = new
            .conversations
            .iter()
            .filter_map(|id| index(id, 'C', pending.len()))
            .filter(|c| !used.contains(c))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        if members.is_empty() || members.len() > MAX_NEW_GROUP {
            continue;
        }
        used.extend(members.iter().copied());
        let title = cut(new.title.trim(), MAX_TITLE_CHARS);
        let title = if title.is_empty() {
            pending[members[0]].title.clone()
        } else {
            title
        };
        grouping.new.push((title, members));
    }
    for id in &answer.leave_out {
        if let Some(c) = index(id, 'C', pending.len()) {
            if used.insert(c) {
                grouping.left_out.push(c);
            }
        }
    }
    for (c, p) in pending.iter().enumerate() {
        if !used.contains(&c) {
            grouping.new.push((p.title.clone(), vec![c]));
        }
    }
    Some(grouping)
}

#[cfg(test)]
#[path = "conversation_intake_test.rs"]
mod tests;
