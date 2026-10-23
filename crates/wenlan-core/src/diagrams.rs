// SPDX-License-Identifier: Apache-2.0
//! Mistakes in a page's ```mermaid blocks that stop Mermaid drawing them.
//!
//! An agent write hears which diagram and line to fix (as a write warning),
//! and an automatic rewrite drops a diagram it broke, so a broken one never
//! reaches the reader or a repository wiki on GitHub. Mermaid's own parser is
//! JavaScript; this one knows the mistakes AI writers make most and stays
//! silent on anything it does not understand, so a diagram Mermaid draws is
//! never flagged. `tests/fixtures/mermaid/cases.json` holds the cases, and
//! the app's test suite checks each against the real parser.

use std::sync::LazyLock;

use regex::Regex;

/// One mistake in one diagram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagramProblem {
    /// Which ```mermaid block on the page, from 1.
    pub diagram: usize,
    /// Line inside the block, from 1 (the `flowchart LR` line), as Mermaid
    /// counts in its own errors.
    pub line: usize,
    pub kind: ProblemKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    Empty,
    UnknownType,
    BadDirection,
    OpenQuote,
    OpenBox,
    LabelBrackets,
    LooseArrow,
    EndAsBox,
    OpenBlock,
    ExtraEnd,
    MessageParty,
    MessageText,
    Citation,
}

impl ProblemKind {
    /// Stable name, used by the shared test fixture.
    pub fn code(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::UnknownType => "unknown_type",
            Self::BadDirection => "bad_direction",
            Self::OpenQuote => "open_quote",
            Self::OpenBox => "open_box",
            Self::LabelBrackets => "label_brackets",
            Self::LooseArrow => "loose_arrow",
            Self::EndAsBox => "end_as_box",
            Self::OpenBlock => "open_block",
            Self::ExtraEnd => "extra_end",
            Self::MessageParty => "message_party",
            Self::MessageText => "message_text",
            Self::Citation => "citation",
        }
    }

    fn phrase(self) -> &'static str {
        match self {
            Self::Empty => "the diagram is empty",
            Self::UnknownType => {
                "the first line names no diagram type Mermaid knows, such as flowchart LR or \
                 sequenceDiagram"
            }
            Self::BadDirection => "the direction after flowchart must be LR, TD, TB, RL or BT",
            Self::OpenQuote => "a quote is not closed",
            Self::OpenBox => "a box's label is not closed",
            Self::LabelBrackets => "a label holds brackets; put the whole label in double quotes",
            Self::LooseArrow => "an arrow is missing the box at one end",
            Self::EndAsBox => "a box is named end, which Mermaid reserves; give it another name",
            Self::OpenBlock => "a subgraph or block is not closed with end",
            Self::ExtraEnd => "an end has nothing to close",
            Self::MessageParty => "a message is missing who sends it or who gets it",
            Self::MessageText => "a message has no text after a colon",
            Self::Citation => {
                "a number in square brackets reads as a citation in Wenlan; quote the label, as \
                 in a[\"1\"], or drop the brackets"
            }
        }
    }
}

impl DiagramProblem {
    /// What an agent is told after a write that landed with this problem.
    pub fn warning(&self) -> String {
        format!(
            "diagram {} line {}: {}. Fix it and write the page again, or remove the diagram.",
            self.diagram,
            self.line,
            self.kind.phrase()
        )
    }
}

/// The first problem in each ```mermaid block of `content`.
pub fn diagram_problems(content: &str) -> Vec<DiagramProblem> {
    let lines: Vec<&str> = content.lines().collect();
    mermaid_blocks(&lines)
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            check_diagram(&lines[block.body.clone()]).map(|(line, kind)| DiagramProblem {
                diagram: index + 1,
                line,
                kind,
            })
        })
        .collect()
}

/// One write warning per diagram with a problem.
pub fn diagram_warnings(content: &str) -> Vec<String> {
    diagram_problems(content)
        .iter()
        .map(DiagramProblem::warning)
        .collect()
}

/// `content` without the diagrams Mermaid could not draw, and how many were
/// dropped. Everything else is kept line for line.
pub fn drop_broken_diagrams(content: &str) -> (String, usize) {
    let lines: Vec<&str> = content.lines().collect();
    let broken: Vec<Block> = mermaid_blocks(&lines)
        .into_iter()
        .filter(|block| check_diagram(&lines[block.body.clone()]).is_some())
        .collect();
    if broken.is_empty() {
        return (content.to_string(), 0);
    }
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut index = 0;
    while index < lines.len() {
        match broken.iter().find(|block| block.fence == index) {
            Some(block) => {
                index = block.end;
                // Keep one blank line where the diagram stood, not two.
                let blank_before = kept.last().is_none_or(|line| line.trim().is_empty());
                if blank_before && lines.get(index).is_some_and(|line| line.trim().is_empty()) {
                    index += 1;
                }
            }
            None => {
                kept.push(lines[index]);
                index += 1;
            }
        }
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }
    (out, broken.len())
}

/// A ```mermaid block: its opening fence line, its body lines, and the line
/// after its closing fence (or the end of the page when it never closes, as
/// CommonMark reads it).
struct Block {
    fence: usize,
    body: std::ops::Range<usize>,
    end: usize,
}

fn mermaid_blocks(lines: &[&str]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let Some((marker, width, info)) = fence_open(lines[index]) else {
            index += 1;
            continue;
        };
        let close = (index + 1..lines.len()).find(|&at| fence_closes(lines[at], marker, width));
        let body_end = close.unwrap_or(lines.len());
        let end = close.map_or(lines.len(), |at| at + 1);
        if info.split_whitespace().next() == Some("mermaid") {
            blocks.push(Block {
                fence: index,
                body: index + 1..body_end,
                end,
            });
        }
        index = end;
    }
    blocks
}

fn fence_open(line: &str) -> Option<(char, usize, &str)> {
    let trimmed = line.trim_start();
    let marker = trimmed.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let width = trimmed.chars().take_while(|c| *c == marker).count();
    if width < 3 {
        return None;
    }
    let info = trimmed[width..].trim();
    if marker == '`' && info.contains('`') {
        return None;
    }
    Some((marker, width, info))
}

fn fence_closes(line: &str, marker: char, width: usize) -> bool {
    let trimmed = line.trim();
    trimmed.chars().take_while(|c| *c == marker).count() >= width
        && trimmed.chars().all(|c| c == marker)
}

/// Type names Mermaid detects from a diagram's first word; a first word that
/// starts with none of them is "No diagram type detected".
const DIAGRAM_TYPES: &[&str] = &[
    "flowchart",
    "graph",
    "sequenceDiagram",
    "classDiagram",
    "stateDiagram",
    "erDiagram",
    "journey",
    "gantt",
    "pie",
    "quadrantChart",
    "requirementDiagram",
    "gitGraph",
    "C4Context",
    "C4Container",
    "C4Component",
    "C4Dynamic",
    "C4Deployment",
    "mindmap",
    "timeline",
    "zenuml",
    "sankey",
    "xychart",
    "block",
    "packet",
    "kanban",
    "architecture",
    "radar",
    "treemap",
    "info",
];

const FLOW_DIRECTIONS: &[&str] = &["LR", "RL", "TB", "TD", "BT", "<", ">", "^", "v"];

static CITATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d+\]").expect("regex"));

/// The first problem in one diagram's body, as (line from 1, kind).
fn check_diagram(body: &[&str]) -> Option<(usize, ProblemKind)> {
    // Line numbers count from the block's first line, as Mermaid's do.
    let mut numbered = body
        .iter()
        .enumerate()
        .map(|(at, line)| (at + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with("%%"));
    let Some((mut header_line, mut header)) = numbered.next() else {
        return Some((1, ProblemKind::Empty));
    };
    // A `---` front-matter block (title, config) comes before the type.
    if header == "---" {
        numbered.by_ref().find(|(_, line)| *line == "---")?;
        (header_line, header) = numbered.next().unwrap_or((body.len().max(1), ""));
        if header.is_empty() {
            return Some((header_line, ProblemKind::Empty));
        }
    }
    let rest: Vec<(usize, &str)> = numbered.collect();
    if let Some(&(line, _)) = std::iter::once(&(header_line, header))
        .chain(rest.iter())
        .find(|(_, line)| CITATION.is_match(line))
    {
        return Some((line, ProblemKind::Citation));
    }
    let header = header.trim_end_matches(';');
    let mut words = header.split_whitespace();
    let kind = words.next().unwrap_or("");
    if !DIAGRAM_TYPES.iter().any(|known| kind.starts_with(known)) {
        return Some((header_line, ProblemKind::UnknownType));
    }
    if kind == "flowchart" || kind == "graph" {
        if let Some(direction) = words.next() {
            if !FLOW_DIRECTIONS.contains(&direction.trim_end_matches(';')) {
                return Some((header_line, ProblemKind::BadDirection));
            }
        }
        return check_flowchart(&rest);
    }
    if kind == "sequenceDiagram" {
        return check_sequence(&rest);
    }
    None
}

// ---- flowchart ----

fn check_flowchart(lines: &[(usize, &str)]) -> Option<(usize, ProblemKind)> {
    let mut open: Vec<usize> = Vec::new();
    for &(number, line) in lines {
        if line.matches('"').count() % 2 == 1 {
            return Some((number, ProblemKind::OpenQuote));
        }
        let first = line.split_whitespace().next().unwrap_or("");
        match first.trim_end_matches(';') {
            "subgraph" => {
                open.push(number);
                continue;
            }
            "end" if line.trim_end_matches(';').trim() == "end" => {
                if open.pop().is_none() {
                    return Some((number, ProblemKind::ExtraEnd));
                }
                continue;
            }
            "direction" | "classDef" | "class" | "style" | "linkStyle" | "click" | "accTitle"
            | "accDescr" => continue,
            _ => {}
        }
        for statement in split_statements(line) {
            if let Some(kind) = check_flow_statement(statement) {
                return Some((number, kind));
            }
        }
    }
    open.first().map(|&line| (line, ProblemKind::OpenBlock))
}

/// A line split on `;` outside quotes.
fn split_statements(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quoted = false;
    for (at, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&line[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&line[start..]);
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect()
}

/// Box shapes as (opener, closers), longest opener first.
const SHAPES: &[(&str, &[&str])] = &[
    ("(((", &[")))"]),
    ("((", &["))"]),
    ("([", &["])"]),
    ("[[", &["]]"]),
    ("[(", &[")]"]),
    ("[/", &["/]", "\\]"]),
    ("[\\", &["\\]", "/]"]),
    ("{{", &["}}"]),
    ("(", &[")"]),
    ("[", &["]"]),
    ("{", &["}"]),
    (">", &["]"]),
];

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    // An arrow, with an optional head at either end: --> --- --x --o <-->
    // -.-> -.- ==> === ~~~ and their longer forms.
    Regex::new(r"^[<xo]?(?:-{2,}[>xo]?|-\.+-[>xo]?|={2,}[>xo]?|~{3,})").expect("regex")
});

static TEXT_LINK_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[<xo]?(?:--|-\.|==)\s").expect("regex"));

static TEXT_LINK_CLOSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s(?:-{2,}[>xo]?|\.-+[>xo]?|={2,}[>xo]?)").expect("regex"));

fn starts_link(rest: &str) -> bool {
    rest.starts_with("--")
        || rest.starts_with("==")
        || rest.starts_with("-.")
        || rest.starts_with("~~")
        || rest.starts_with("<-")
        || rest.starts_with("<=")
}

/// A statement's boxes and arrows, left to right. Anything this does not
/// recognize ends the check for the statement with no problem.
fn check_flow_statement(statement: &str) -> Option<ProblemKind> {
    let mut rest = statement.trim();
    let mut boxes = 0;
    let mut after_join = false;
    loop {
        // A box.
        rest = rest.trim_start();
        if rest.is_empty() {
            return after_join.then_some(ProblemKind::LooseArrow);
        }
        if starts_link(rest) {
            return Some(ProblemKind::LooseArrow);
        }
        let id_len = id_length(rest);
        if id_len == 0 {
            return None;
        }
        let id = &rest[..id_len];
        rest = &rest[id_len..];
        if id == "end" && (boxes > 0 || !rest.trim().is_empty()) {
            return Some(ProblemKind::EndAsBox);
        }
        boxes += 1;
        if let Some(after) = rest.strip_prefix("@{") {
            match after.find('}') {
                Some(close) => rest = &after[close + 1..],
                None => return Some(ProblemKind::OpenBox),
            }
        } else if let Some((opener, closers)) =
            SHAPES.iter().find(|(opener, _)| rest.starts_with(opener))
        {
            match shape_end(&rest[opener.len()..], closers) {
                Ok(end) => rest = &rest[opener.len() + end..],
                Err(kind) => return Some(kind),
            }
        }
        if let Some(after) = rest.strip_prefix(":::") {
            rest = &after[id_length(after)..];
        }
        // What joins it to the next box, if anything.
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        if let Some(after) = rest.strip_prefix('&') {
            rest = after;
            after_join = true;
            continue;
        }
        if TEXT_LINK_OPEN.is_match(rest) {
            let close = TEXT_LINK_CLOSE.find(rest)?;
            rest = &rest[close.end()..];
        } else if let Some(link) = LINK.find(rest) {
            rest = &rest[link.end()..];
        } else {
            return None;
        }
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('|') {
            let Some(close) = after.find('|') else {
                return Some(ProblemKind::OpenBox);
            };
            let label = after[..close].trim();
            if !is_quoted(label) && label.contains(['(', ')', '[', ']', '{', '}']) {
                return Some(ProblemKind::LabelBrackets);
            }
            rest = &after[close + 1..];
        }
        after_join = true;
    }
}

/// Bytes of the node id at the start of `rest`.
fn id_length(rest: &str) -> usize {
    let mut end = 0;
    for (at, c) in rest.char_indices() {
        if c.is_whitespace() || "[](){}<>|&;:\"@".contains(c) || starts_link(&rest[at..]) {
            break;
        }
        end = at + c.len_utf8();
    }
    end
}

fn is_quoted(label: &str) -> bool {
    label.len() >= 2 && label.starts_with('"') && label.ends_with('"')
}

/// Where a box's label ends, just past its closer.
fn shape_end(inner: &str, closers: &[&str]) -> Result<usize, ProblemKind> {
    let first_closer = |text: &str| {
        closers
            .iter()
            .filter_map(|closer| text.find(closer).map(|at| at + closer.len()))
            .min()
    };
    let start = inner.len() - inner.trim_start().len();
    if inner[start..].starts_with('"') {
        // A quoted label ends at the quote right before the closer.
        let mut search = start + 1;
        while let Some(quote) = inner[search..].find('"') {
            let after = search + quote + 1;
            let gap = inner[after..].len() - inner[after..].trim_start().len();
            if closers
                .iter()
                .any(|closer| inner[after + gap..].starts_with(closer))
            {
                return first_closer(&inner[after + gap..])
                    .map(|end| after + gap + end)
                    .ok_or(ProblemKind::OpenBox);
            }
            search = after;
        }
        return Err(ProblemKind::OpenBox);
    }
    let end = first_closer(inner).ok_or(ProblemKind::OpenBox)?;
    let closer_len = closers
        .iter()
        .find(|closer| inner[..end].ends_with(*closer))
        .map_or(1, |closer| closer.len());
    if inner[..end - closer_len].contains(['(', ')', '[', ']', '{', '}']) {
        return Err(ProblemKind::LabelBrackets);
    }
    Ok(end)
}

// ---- sequenceDiagram ----

static SIGNAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<from>[^:]*?)\s*(?:<<-{1,2}>>|-{1,2}>>|-{1,2}>|-{1,2}x|-{1,2}\))\s*[+-]?(?P<to>[^:]*?)\s*(?P<text>:.*)?$")
        .expect("regex")
});

const SEQUENCE_BLOCKS: &[&str] = &[
    "loop", "alt", "opt", "par", "critical", "break", "rect", "box",
];

const SEQUENCE_LINES: &[&str] = &[
    "participant",
    "actor",
    "note",
    "else",
    "and",
    "option",
    "autonumber",
    "activate",
    "deactivate",
    "create",
    "destroy",
    "link",
    "links",
    "properties",
    "details",
    "title",
    "accTitle",
    "accDescr",
];

fn check_sequence(lines: &[(usize, &str)]) -> Option<(usize, ProblemKind)> {
    let mut open: Vec<usize> = Vec::new();
    for &(number, line) in lines {
        let first = line.split_whitespace().next().unwrap_or("");
        let word = first.trim_end_matches(':');
        if SEQUENCE_BLOCKS.contains(&word) || word.starts_with("par_") {
            open.push(number);
            continue;
        }
        if word == "end" {
            if open.pop().is_none() {
                return Some((number, ProblemKind::ExtraEnd));
            }
            continue;
        }
        if SEQUENCE_LINES
            .iter()
            .any(|known| word.eq_ignore_ascii_case(known))
        {
            continue;
        }
        let Some(signal) = SIGNAL.captures(line) else {
            continue;
        };
        if signal["from"].trim().is_empty() || signal["to"].trim().is_empty() {
            return Some((number, ProblemKind::MessageParty));
        }
        if signal.name("text").is_none() {
            return Some((number, ProblemKind::MessageText));
        }
    }
    open.first().map(|&line| (line, ProblemKind::OpenBlock))
}

#[cfg(test)]
#[path = "diagrams_test.rs"]
mod diagrams_test;
