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
use std::sync::LazyLock;

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
    /// What the conversation actually worked on, most often first: the
    /// files it changed, relative to its folder, and rule ids such as
    /// UX-HOME-4 it named. Names only, never what the files say; the
    /// grouping model reads them so conversations that change the same
    /// things land in the same thing even under different titles.
    pub touched: Vec<String>,
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

/// Files a conversation changed and rule ids it named, counted.
#[derive(Default)]
struct Touched {
    files: HashMap<String, usize>,
    ids: HashMap<String, usize>,
}

/// Files and rule ids kept for one conversation, each.
const MAX_TOUCHED_FILES: usize = 8;
const MAX_TOUCHED_IDS: usize = 8;

/// A rule or ticket id with at least two letter parts, such as UX-HOME-4.
/// One-part ids (UTF-8, SHA-256) say nothing about the work.
static RULE_ID: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b[A-Z]{2,}(?:-[A-Z]{2,})+-\d+\b").expect("rule id pattern")
});

impl Touched {
    fn file(&mut self, path: &str) {
        let path = path.trim();
        if !path.is_empty() {
            *self.files.entry(path.to_string()).or_default() += 1;
        }
    }

    fn ids_in(&mut self, text: &str) {
        for found in RULE_ID.find_iter(text) {
            *self.ids.entry(found.as_str().to_string()).or_default() += 1;
        }
    }

    /// The files inside `cwd`, relative to it, then the ids, most often
    /// first. A file outside the conversation's folder (a scratch file, a
    /// note elsewhere) says little about its work and is left out.
    fn finish(self, cwd: Option<&str>) -> Vec<String> {
        fn top(counts: HashMap<String, usize>, max: usize) -> Vec<String> {
            let mut all: Vec<(String, usize)> = counts.into_iter().collect();
            all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            all.into_iter().take(max).map(|(name, _)| name).collect()
        }
        let mut files: HashMap<String, usize> = HashMap::new();
        if let Some(cwd) = cwd
            .map(|c| c.trim_end_matches('/'))
            .filter(|c| !c.is_empty())
        {
            for (path, n) in self.files {
                let inside = if Path::new(&path).is_absolute() {
                    path.strip_prefix(cwd)
                        .and_then(|rest| rest.strip_prefix('/'))
                        .map(str::to_string)
                } else {
                    Some(path.trim_start_matches("./").to_string())
                };
                if let Some(inside) = inside.filter(|p| !p.is_empty() && !p.starts_with("..")) {
                    *files.entry(inside).or_default() += n;
                }
            }
        }
        let mut out = top(files, MAX_TOUCHED_FILES);
        out.extend(top(self.ids, MAX_TOUCHED_IDS));
        out
    }
}

/// Files an `apply_patch` call changed; false when it is no patch. The
/// patch may sit inside a script's string (Codex's `exec` tool), with its
/// line breaks written `\n` and a quote right after the path.
fn patched_files(patch: &str, touch: &mut Touched) -> bool {
    let mut any = false;
    for line in patch.replace("\\n", "\n").lines() {
        for head in ["*** Update File: ", "*** Add File: ", "*** Delete File: "] {
            let Some(at) = line.find(head) else {
                continue;
            };
            let path = line[at + head.len()..]
                .split(['"', '\\', '`', '\''])
                .next()
                .unwrap_or_default()
                .trim();
            if !path.is_empty() {
                touch.file(path);
                any = true;
            }
        }
    }
    any
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
    /// A tool call's name and input.
    name: Option<String>,
    input: Option<serde_json::Value>,
}

impl ClaudeContent {
    /// The files a turn's tool calls changed, and the rule ids its commands
    /// (commit messages, PR titles) name.
    fn tool_calls(&self, touch: &mut Touched) {
        let Self::Blocks(blocks) = self else {
            return;
        };
        for block in blocks
            .iter()
            .filter(|b| b.kind.as_deref() == Some("tool_use"))
        {
            let Some(input) = &block.input else {
                continue;
            };
            if matches!(
                block.name.as_deref(),
                Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit")
            ) {
                if let Some(file) = input["file_path"]
                    .as_str()
                    .or_else(|| input["notebook_path"].as_str())
                {
                    touch.file(file);
                }
            }
            // A command (a commit message, a PR title) names what the work
            // is; a file's new text would name every id in it.
            if let Some(command) = input["command"].as_str() {
                touch.ids_in(command);
            }
        }
    }

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
    let mut touch = Touched::default();
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
        if entry.kind.as_deref() == Some("assistant") {
            content.tool_calls(&mut touch);
        }
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
                    touch.ids_in(&text);
                    said.push(cut(&text, MAX_SAID_CHARS));
                }
            }
            Some("assistant") => {
                let texts = content.texts();
                for text in &texts {
                    touch.ids_in(text);
                }
                if let Some(text) = texts.into_iter().rev().find(|t| !t.trim().is_empty()) {
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
    let touched = touch.finish(cwd.as_deref());
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
        touched,
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
    let mut touch = Touched::default();
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
            (Some("event_msg"), Some("user_message")) => {
                let text = text_of(&p["message"]);
                if let Some(text) = &text {
                    touch.ids_in(text);
                }
                typed(text, &mut said);
            }
            // A tool call: `apply_patch` names the files it changes; any
            // call's input (a command, a PR title) can name rule ids.
            (Some("response_item"), Some("function_call" | "custom_tool_call")) => {
                let input = p["arguments"]
                    .as_str()
                    .or_else(|| p["input"].as_str())
                    .unwrap_or_default();
                // `arguments` is JSON text; its escaped newlines would hide
                // the patch's file lines.
                let input = serde_json::from_str::<serde_json::Value>(input)
                    .ok()
                    .and_then(|v| {
                        v["input"]
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| v["patch"].as_str().map(str::to_string))
                            .or_else(|| Some(v.to_string()))
                    })
                    .unwrap_or_else(|| input.to_string());
                // A patch's text would name every id in the file it changes.
                if !patched_files(&input, &mut touch) {
                    touch.ids_in(&input);
                }
            }
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
                    touch.ids_in(&reply);
                    last_reply = cut_end(&reply, MAX_REPLY_CHARS);
                }
            }
            _ => {}
        }
    }
    let external_id = id?;
    let fallback = modified_secs(path)?;
    let touched = touch.finish(cwd.as_deref());
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
        touched,
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
conversation (for example Wenlan, Heptabase); do not translate or transliterate them. \
Write for the person, in plain words: no pull request or issue numbers, commit hashes, \
version-control branch names, file paths, ids or dates.";

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
question, a greeting, a test of the tool, a one-off lookup, or a one-off setup or cleanup. A \
discussion, plan or decision about one of the person's projects is work, even when nothing was \
built in it.\n\
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
    /// Where this conversation left things, as its label says.
    pub status: String,
    /// Files it changed and rule ids it named ([`FoundConversation::touched`]).
    pub touched: Vec<String>,
}

/// A thing already on the list, as the grouping model sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenThing {
    pub id: String,
    pub title: String,
    pub about: String,
    pub space: String,
    /// Where its conversations stand, newest first, from those read again
    /// since its line was last written: the line needs writing again.
    pub news: Vec<String>,
    /// What its conversations worked on, most often first.
    pub touched: Vec<String>,
}

/// A conversation the person moved to another thing: what it was about, and
/// the titles of the thing it left and the one it went to (UX-THING-2).
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    pub title: String,
    pub about: String,
    pub from: String,
    pub to: String,
}

/// ` | worked on: a, b` for a non-empty [`FoundConversation::touched`].
fn touched_note(touched: &[String]) -> String {
    if touched.is_empty() {
        String::new()
    } else {
        format!(" | worked on: {}", touched.join(", "))
    }
}

/// The prompt that puts labelled conversations into things and writes where
/// each thing stands: only the labels' one-line titles, abouts and statuses,
/// and the names of the files and rules each conversation worked on, never
/// the conversations. The person's past moves come along as examples,
/// so a case they corrected once is sorted their way (UX-THING-2).
pub fn group_prompt(
    pending: &[Pending],
    open: &[OpenThing],
    corrections: &[Correction],
    language: &str,
) -> String {
    let things = if open.is_empty() {
        "(none yet)".to_string()
    } else {
        open.iter()
            .enumerate()
            .map(|(i, t)| {
                let news = if t.news.is_empty() {
                    String::new()
                } else {
                    format!(" | news: {}", t.news.join(" / "))
                };
                format!(
                    "T{i}. {} [{}]: {}{}{news}",
                    t.title,
                    t.space,
                    t.about,
                    touched_note(&t.touched)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let convs = if pending.is_empty() {
        "(none)".to_string()
    } else {
        pending
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "C{i}. {} [{}]: {}{} | now: {}",
                    p.title,
                    p.space,
                    p.about,
                    touched_note(&p.touched),
                    p.status
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let corrected = if corrections.is_empty() {
        String::new()
    } else {
        let moves = corrections
            .iter()
            .map(|c| {
                let about = if c.about.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.about)
                };
                if c.from.is_empty() {
                    format!("- \"{}\"{about} belongs to \"{}\"", c.title, c.to)
                } else {
                    format!(
                        "- \"{}\"{about}: not \"{}\" but \"{}\"",
                        c.title, c.from, c.to
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "The person corrected where these conversations went. Sort a new conversation \
like one of them the way the person did: into the piece with that title, or a new piece \
with that title when none is listed.\n{moves}\n\n"
        )
    };
    format!(
        "A person's pieces of work so far (id, title, [space], about):\n{things}\n\n\
{corrected}New conversations they had with AI tools (id, title, [space], about):\n{convs}\n\n\
Put every new conversation into a piece of work, or leave it out. A piece is a goal the \
person is working toward (a product feature, a release, a launch, a paper, a trip), not a \
step of it: a review, a test run, a cleanup or a check done for a goal belongs to that goal. \
Judge by what the work actually is, not by how alike the titles are: \"worked on\" lists the \
files a conversation changed and the rule ids it named. Conversations that changed the same \
files or worked on the same rules usually serve one goal even under different titles. \
A conversation joins an existing piece when it serves the same goal; a shared product, \
folder or space alone is not enough. Conversations that serve the same new goal form one new \
piece together. Leave out a conversation that is only a small chore with no goal of its own \
(confirming a merge, sending a file, a one-off cleanup) and that fits no piece.\n\
Give each new piece a short title in {language} that names the goal: at most 6 words or 10 \
Chinese characters. {KEEP_NAMES}\n\
For every new piece, and every existing piece that gains a conversation or shows news, write \
\"now\": one sentence in {language}, at most 20 words or 30 Chinese characters, on where the \
whole piece stands and what comes next, from its newest conversations, not only their last \
small step.\n\
If two existing pieces are clearly the same goal under two names, list them in \"same\"; \
they are not merged, the person is asked.\n\
Answer with JSON only: {{\"join\": [{{\"conversation\": \"C0\", \"piece\": \"T3\"}}], \
\"new\": [{{\"title\": string, \"conversations\": [\"C1\", \"C2\"], \"now\": string}}], \
\"now\": [{{\"piece\": \"T3\", \"line\": string}}], \
\"leave_out\": [\"C4\"], \"same\": [[\"T1\", \"T5\"]]}} and use every conversation exactly once."
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
    /// Where a piece stands now, written by the model.
    pub lines: Vec<(Piece, String)>,
    /// Pairs of open-thing indices the model thinks are one goal: only ever
    /// a proposal to the person (UX-THING-3).
    pub same: Vec<(usize, usize)>,
}

/// A piece of work in a [`Grouping`]: an open thing or a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece {
    /// Index into the `open` list.
    Open(usize),
    /// Index into [`Grouping::new`].
    New(usize),
}

/// Longest "where it stands" line kept from the grouping model.
pub const MAX_LINE_CHARS: usize = 120;

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
        #[serde(default)]
        now: String,
    }
    #[derive(Deserialize)]
    struct Now {
        piece: String,
        #[serde(default)]
        line: String,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        join: Vec<Join>,
        #[serde(default)]
        new: Vec<New>,
        #[serde(default)]
        leave_out: Vec<String>,
        #[serde(default)]
        now: Vec<Now>,
        #[serde(default)]
        same: Vec<Vec<String>>,
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
        let line = cut(new.now.trim(), MAX_LINE_CHARS);
        if !line.is_empty() {
            grouping.lines.push((Piece::New(grouping.new.len()), line));
        }
        grouping.new.push((title, members));
    }
    for now in &answer.now {
        let line = cut(now.line.trim(), MAX_LINE_CHARS);
        if let (Some(t), false) = (index(&now.piece, 'T', open), line.is_empty()) {
            grouping.lines.retain(|(piece, _)| *piece != Piece::Open(t));
            grouping.lines.push((Piece::Open(t), line));
        }
    }
    for pair in &answer.same {
        if let [x, y] = pair.as_slice() {
            if let (Some(x), Some(y)) = (index(x, 'T', open), index(y, 'T', open)) {
                let pair = (x.min(y), x.max(y));
                if x != y && !grouping.same.contains(&pair) {
                    grouping.same.push(pair);
                }
            }
        }
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

// ===== Overnight: what a piece of work settled goes into long-term memory =====
//
// UX-CHAT-5. Each night the grouping model reads the conversations of every
// thing that went on that day and writes down what the person settled and
// what they left open. Those points become memories, so they join the big
// map like anything else Wenlan remembers. Like "dreaming" elsewhere: the
// conversations are only read, a point that no longer holds is replaced by
// its newer version, and the person can take back a night's points.

/// Most points one night writes for one thing.
pub const MAX_DREAM_POINTS: usize = 8;
/// Longest point kept, in characters.
pub const MAX_POINT_CHARS: usize = 200;
/// Most conversations of one thing read in one night, newest first.
pub const MAX_DREAM_CONVERSATIONS: usize = 6;
/// Characters of each thing the person typed that the model reads. Work
/// handed to an assistant comes as long instructions, so this is more than
/// a label reads.
const DREAM_SAID_CHARS: usize = 600;
/// Most of the person's messages read from one conversation, newest kept.
const DREAM_SAID: usize = 20;

/// A point an earlier night wrote for this thing, still standing.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorPoint {
    /// The memory it is.
    pub id: String,
    pub text: String,
    pub decided: bool,
}

/// A point the model wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct DreamPoint {
    pub text: String,
    /// Settled, or still open.
    pub decided: bool,
    /// Index of the conversation it came from.
    pub from: usize,
    /// Index of the earlier point it replaces, when that one no longer holds.
    pub replaces: Option<usize>,
    /// Index of the point on the thing's map it is, when it is one that was
    /// not kept yet.
    pub map: Option<usize>,
    /// Index of the topic on the big map it is about, when one fits.
    pub topic: Option<usize>,
}

/// A point on a thing's map that no night kept yet: the model can say a point
/// it writes is this one, so the map and long-term memory stay one point.
#[derive(Debug, Clone, PartialEq)]
pub struct MapPoint {
    pub text: String,
    /// Settled, open, or neither (a topic, a reason).
    pub decided: Option<bool>,
}

/// The prompt that reads a thing's conversations for what they settled.
pub fn dream_prompt(
    title: &str,
    conversations: &[FoundConversation],
    prior: &[PriorPoint],
    map: &[MapPoint],
    topics: &[String],
    language: &str,
) -> String {
    let mut chats = String::new();
    for (i, conv) in conversations.iter().enumerate() {
        let start = conv.said.len().saturating_sub(DREAM_SAID);
        let said = conv.said[start..]
            .iter()
            .map(|s| format!("- {}", cut(s, DREAM_SAID_CHARS)))
            .collect::<Vec<_>>()
            .join("\n");
        let reply = if conv.last_reply.is_empty() {
            "(none)".to_string()
        } else {
            cut_end(&conv.last_reply, MAX_REPLY_CHARS)
        };
        chats.push_str(&format!(
            "C{i}: what the person typed, oldest first:\n{said}\nAssistant's last reply (end): {reply}\n\n"
        ));
    }
    let prior_text = if prior.is_empty() {
        "(none)".to_string()
    } else {
        prior
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let state = if p.decided { "settled" } else { "open" };
                format!("P{i} ({state}): {}", p.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let map_text = if map.is_empty() {
        "(none)".to_string()
    } else {
        map.iter()
            .enumerate()
            .map(|(i, p)| {
                let state = match p.decided {
                    Some(true) => " (settled)",
                    Some(false) => " (open)",
                    None => "",
                };
                format!("N{i}{state}: {}", p.text)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    // No topics to offer: the question is not asked at all.
    let (topics_text, topic_field) = if topics.is_empty() {
        (String::new(), "")
    } else {
        (
            format!("{}\n", topic_list(topics)),
            ", \"topic\": \"T0\" or null",
        )
    };
    format!(
        "A piece of work the person is doing with AI tools: \"{title}\".\n\n\
Its conversations since the last time they were read:\n{chats}\
Points already kept from it:\n{prior_text}\n\n\
Points on its map, not kept yet:\n{map_text}\n\n\
Write down what these conversations settled and what the person left open about the work \
itself, so they can be remembered after it goes quiet. Only what the person themself decided \
or chose about the work (settled), and questions about the work they left unanswered (open). \
What was built, what a review found and where a pull request stands are not decisions. Leave \
out: steps the assistant took, chores, greetings; approving a merge, a review, a handoff or a run; \
how the person wants the assistant to work; where files are or problems with tools; and \
details nobody could follow without the conversation. Agreeing to go on is not an open \
question. A question answered or done later in the same conversations is settled, not open. \
One short sentence each, at most 15 words, that someone who did not read the conversation \
understands (name what it is about), in {language}. {KEEP_NAMES} Leave out anything already \
kept above. When a kept point no longer holds (it was decided differently, or what was open is now \
settled), give the new point and name the old one in \"replaces\". When a point you write is \
one of the map points above (the same point, perhaps now settled), name it in \"map\". At most \
{MAX_DREAM_POINTS} points; none is a fine answer.\n\
{topics_text}\
Answer with JSON only: {{\"points\": [{{\"text\": string, \"settled\": true|false, \
\"from\": \"C0\", \"replaces\": \"P0\" or null, \"map\": \"N0\" or null{topic_field}}}]}}"
    )
}

/// The topics a model may choose from, and how to choose.
fn topic_list(topics: &[String]) -> String {
    let list = topics
        .iter()
        .enumerate()
        .map(|(i, name)| format!("T{i}: {name}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Topics on the person's big map:\n{list}\n\n\
For each point also give the one topic above that it is really about, in \"topic\". \
Sharing a word is not enough: the point must be about that topic. When none fits, use null; \
do not make up a topic."
    )
}

/// Whether a topic's name can be offered at all. Names of code (paths,
/// functions, file names, test names, bare numbers) and shouted phrases such
/// as "EXIT CODE" are not what a person's point is about; product names such
/// as GitHub, macOS and ChatGPT stay.
pub fn topic_name_offered(name: &str) -> bool {
    static CODE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"_|::|/|\.(rs|ts|tsx|py|md|json|toml)\b|\(\)|^#?\d+$|\btests?\b|^https?:|^--",
        )
        .expect("the pattern is valid")
    });
    let shouted = name.split_whitespace().count() >= 2
        && name.chars().any(char::is_alphabetic)
        && !name.chars().any(char::is_lowercase);
    !name.trim().is_empty() && !CODE.is_match(name) && !shouted
}

/// How many times a topic's name is in the text, when it is specific enough
/// that this means something. Several words, or a Chinese, Japanese or Korean
/// name of three characters or more, count as they are. One word of four
/// letters or more counts only when it has capitals and is written with them
/// at least as often as without: "GitHub" does, "Memory" in a text that mostly
/// says "memory" does not. `lower` is `raw` in lowercase.
pub fn specific_name_hits(name: &str, raw: &str, lower: &str) -> Option<usize> {
    let wide = name.chars().any(|c| c as u32 > 0x2E80);
    if wide {
        return (name.chars().count() >= 3 && raw.contains(name))
            .then(|| raw.matches(name).count());
    }
    if name.chars().count() < 4 {
        return None;
    }
    if name.split_whitespace().count() >= 2 {
        let hits = lower.matches(&name.to_lowercase()).count();
        return (hits > 0).then_some(hits);
    }
    if !name.chars().any(char::is_uppercase) {
        return None;
    }
    let exact = raw.matches(name).count();
    let plain = raw.matches(&name.to_lowercase()).count();
    (exact > 0 && exact >= plain).then_some(exact)
}

/// What the person typed about a thing: where its topics' names are looked
/// for. Not the assistant's replies, which name everything they touched.
pub fn night_said(title: &str, conversations: &[FoundConversation]) -> String {
    let mut text = title.to_string();
    for conv in conversations {
        for said in &conv.said {
            text.push('\n');
            text.push_str(said);
        }
    }
    text
}

/// What a thing's nearest topics are searched by: its title, the end of what
/// the person typed, and the points already kept.
pub fn night_topic_query(
    title: &str,
    conversations: &[FoundConversation],
    prior: &[PriorPoint],
) -> String {
    let mut query = title.to_string();
    for conv in conversations {
        for said in &conv.said[conv.said.len().saturating_sub(5)..] {
            query.push('\n');
            query.push_str(&cut(said, 300));
        }
    }
    for point in prior {
        query.push('\n');
        query.push_str(&point.text);
    }
    query
}

/// The prompt that names the topic of points kept before nights did so.
pub fn night_links_prompt(title: &str, points: &[String], topics: &[String]) -> String {
    let list = points
        .iter()
        .enumerate()
        .map(|(i, text)| format!("P{i}: {text}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "A piece of work the person is doing with AI tools: \"{title}\".\n\n\
Points kept from it:\n{list}\n\n{}\n\
Answer with JSON only: {{\"points\": [{{\"point\": \"P0\", \"topic\": \"T0\" or null}}]}}",
        topic_list(topics)
    )
}

/// The (point, topic) pairs in a model's answer. A pair naming a point or a
/// topic that does not exist is ignored, and so is a second answer for a point.
pub fn parse_night_links(text: &str, points: usize, topics: usize) -> Option<Vec<(usize, usize)>> {
    #[derive(Deserialize)]
    struct Pair {
        #[serde(default)]
        point: String,
        #[serde(default)]
        topic: Option<String>,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        points: Vec<Pair>,
    }
    let answer: Answer = serde_json::from_str(json_object(text)?).ok()?;
    let mut seen = HashSet::new();
    Some(
        answer
            .points
            .into_iter()
            .filter_map(|pair| {
                let point = answer_index(&pair.point, 'P', points)?;
                seen.insert(point).then_some(())?;
                Some((point, answer_index(pair.topic.as_deref()?, 'T', topics)?))
            })
            .collect(),
    )
}

/// The number in an id a model answered with, such as "T3", when that one
/// exists.
fn answer_index(id: &str, prefix: char, len: usize) -> Option<usize> {
    id.trim()
        .strip_prefix(prefix)
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n < len)
}

/// The points in a model's answer. A point without text, or from a
/// conversation that does not exist, is dropped; a `replaces` that names no
/// kept point, or one already replaced, is ignored, and so is a `map` that
/// names no map point or one another point already is. A `topic` that names
/// no topic offered is ignored: the point is kept, without a topic.
pub fn parse_dream(
    text: &str,
    conversations: usize,
    prior: usize,
    map: usize,
    topics: usize,
) -> Option<Vec<DreamPoint>> {
    #[derive(Deserialize)]
    struct Point {
        #[serde(default)]
        text: String,
        #[serde(default)]
        settled: bool,
        #[serde(default)]
        from: String,
        #[serde(default)]
        replaces: Option<String>,
        #[serde(default)]
        map: Option<String>,
        #[serde(default)]
        topic: Option<String>,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        points: Vec<Point>,
    }
    let answer: Answer = serde_json::from_str(json_object(text)?).ok()?;
    let index = answer_index;
    let mut replaced = HashSet::new();
    let mut mapped = HashSet::new();
    let mut seen = HashSet::new();
    let mut points = Vec::new();
    for point in answer.points {
        let text = cut(point.text.trim(), MAX_POINT_CHARS);
        let Some(from) = index(&point.from, 'C', conversations) else {
            continue;
        };
        if text.is_empty() || !seen.insert(text.clone()) {
            continue;
        }
        let replaces = point
            .replaces
            .as_deref()
            .and_then(|id| index(id, 'P', prior))
            .filter(|p| replaced.insert(*p));
        let on_map = point
            .map
            .as_deref()
            .and_then(|id| index(id, 'N', map))
            .filter(|n| mapped.insert(*n));
        points.push(DreamPoint {
            text,
            decided: point.settled,
            from,
            replaces,
            map: on_map,
            topic: point.topic.as_deref().and_then(|id| index(id, 'T', topics)),
        });
        if points.len() == MAX_DREAM_POINTS {
            break;
        }
    }
    Some(points)
}

/// Most sentences in a thing's conclusion.
pub const MAX_CONCLUSION_SENTENCES: usize = 2;
/// Longest conclusion sentence kept, in characters.
pub const MAX_CONCLUSION_CHARS: usize = 160;
/// Most standing points a conclusion is written from, newest kept.
pub const MAX_CONCLUSION_POINTS: usize = 40;

/// The prompt that writes what a thing has come to from its standing points,
/// oldest first: the first paragraph of its page (UX-VIEW-6). Give it at most
/// [`MAX_CONCLUSION_POINTS`]; see [`conclusion_points`].
pub fn conclusion_prompt(title: &str, points: &[PriorPoint], language: &str) -> String {
    let list = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let state = if p.decided { "settled" } else { "open" };
            format!("P{i} ({state}): {}", p.text)
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "A piece of work the person is doing with AI tools: \"{title}\".\n\n\
What they settled and left open in it so far, oldest first:\n{list}\n\n\
Write what the work has come to, for the top of its page: at most \
{MAX_CONCLUSION_SENTENCES} short sentences in {language}. First the main thing that is \
settled, then what is still open or comes next. When nothing is settled yet, one sentence \
on what is still being decided. Say only what the points above say; add nothing. A later \
point wins over an earlier one. Each sentence names the points it rests on. {KEEP_NAMES}\n\
Answer with JSON only: {{\"conclusion\": [{{\"text\": string, \"points\": [\"P0\"]}}]}}"
    )
}

/// The standing points a conclusion is written from: the newest
/// [`MAX_CONCLUSION_POINTS`], still oldest first.
pub fn conclusion_points(points: &[PriorPoint]) -> &[PriorPoint] {
    &points[points.len().saturating_sub(MAX_CONCLUSION_POINTS)..]
}

/// A conclusion sentence in a model's answer: its text and the indices of the
/// points it rests on.
#[derive(Debug, Clone, PartialEq)]
pub struct ConclusionSentence {
    pub text: String,
    pub points: Vec<usize>,
}

/// The conclusion in a model's answer. A sentence that names no point of the
/// `points` given is dropped, since it would say something with no source;
/// sentences past [`MAX_CONCLUSION_SENTENCES`] are dropped and each is cut to
/// [`MAX_CONCLUSION_CHARS`]. `None` when the answer is not JSON.
pub fn parse_conclusion(text: &str, points: usize) -> Option<Vec<ConclusionSentence>> {
    #[derive(Deserialize)]
    struct Sentence {
        #[serde(default)]
        text: String,
        #[serde(default)]
        points: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Answer {
        #[serde(default)]
        conclusion: Vec<Sentence>,
    }
    let answer: Answer = serde_json::from_str(json_object(text)?).ok()?;
    let mut out = Vec::new();
    for sentence in answer.conclusion {
        let text = cut(sentence.text.trim(), MAX_CONCLUSION_CHARS);
        let mut cited: Vec<usize> = Vec::new();
        for id in &sentence.points {
            if let Some(i) = answer_index(id, 'P', points) {
                if !cited.contains(&i) {
                    cited.push(i);
                }
            }
        }
        if text.is_empty() || cited.is_empty() {
            continue;
        }
        out.push(ConclusionSentence {
            text,
            points: cited,
        });
        if out.len() == MAX_CONCLUSION_SENTENCES {
            break;
        }
    }
    Some(out)
}

#[cfg(test)]
#[path = "conversation_intake_test.rs"]
mod tests;
