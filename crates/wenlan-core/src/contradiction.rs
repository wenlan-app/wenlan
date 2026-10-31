// SPDX-License-Identifier: Apache-2.0
//! Structured field comparison for semantic contradiction pre-filtering.
//!
//! These functions are cheap (no LLM, no embedding) and serve as a
//! fast pre-filter before queuing the full LLM contradiction check.

use serde_json::Value;

/// Outcome of comparing a new memory against an existing one.
#[derive(Debug, Clone, PartialEq)]
pub enum ContradictionResult {
    Consistent,
    Contradicts { explanation: String },
    Supersedes { merged_content: String },
}

/// Check if two memories of the same type may contradict based on structured fields.
/// Returns true if key fields overlap but values differ — a candidate for LLM contradiction check.
/// Returns false on parse failure, missing fields, or non-overlapping contexts.
pub fn fields_may_contradict(memory_type: &str, existing_json: &str, new_json: &str) -> bool {
    let existing: serde_json::Map<String, Value> = match serde_json::from_str(existing_json) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let new: serde_json::Map<String, Value> = match serde_json::from_str(new_json) {
        Ok(m) => m,
        Err(_) => return false,
    };

    match memory_type {
        "preference" => compare_with_context(&existing, &new, "preference", "applies_when"),
        "identity" => {
            let e_claim = get_str(&existing, "claim");
            let n_claim = get_str(&new, "claim");
            match (e_claim, n_claim) {
                (Some(e), Some(n)) => e != n && bigram_jaccard(e, n) > 0.3,
                _ => false,
            }
        }
        "fact" => {
            let e_claim = get_str(&existing, "claim");
            let n_claim = get_str(&new, "claim");
            let e_domain = get_str(&existing, "domain");
            let n_domain = get_str(&new, "domain");
            match (e_claim, n_claim) {
                (Some(e), Some(n)) => {
                    let same_domain = match (e_domain, n_domain) {
                        (Some(ed), Some(nd)) => ed.to_lowercase() == nd.to_lowercase(),
                        _ => true,
                    };
                    same_domain && e != n && bigram_jaccard(e, n) > 0.3
                }
                _ => false,
            }
        }
        "decision" => compare_with_context(&existing, &new, "decision", "context"),
        "goal" => {
            let e_obj = get_str(&existing, "objective");
            let n_obj = get_str(&new, "objective");
            let e_status = get_str(&existing, "status");
            let n_status = get_str(&new, "status");
            match (e_obj, n_obj) {
                (Some(e), Some(n)) => bigram_jaccard(e, n) > 0.5 && e_status != n_status,
                _ => false,
            }
        }
        _ => false,
    }
}

fn compare_with_context(
    existing: &serde_json::Map<String, Value>,
    new: &serde_json::Map<String, Value>,
    value_key: &str,
    context_key: &str,
) -> bool {
    let e_val = get_str(existing, value_key);
    let n_val = get_str(new, value_key);
    let e_ctx = get_str(existing, context_key);
    let n_ctx = get_str(new, context_key);

    match (e_val, n_val, e_ctx, n_ctx) {
        (Some(ev), Some(nv), Some(ec), Some(nc)) => {
            bigram_jaccard(ec, nc) > 0.5 && ev.to_lowercase() != nv.to_lowercase()
        }
        (Some(ev), Some(nv), _, _) => ev.to_lowercase() != nv.to_lowercase(),
        _ => false,
    }
}

fn get_str<'a>(map: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(|v| v.as_str())
}

pub fn bigram_jaccard(a: &str, b: &str) -> f64 {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let a_bigrams: std::collections::HashSet<(char, char)> =
        a_lower.chars().zip(a_lower.chars().skip(1)).collect();
    let b_bigrams: std::collections::HashSet<(char, char)> =
        b_lower.chars().zip(b_lower.chars().skip(1)).collect();
    if a_bigrams.is_empty() && b_bigrams.is_empty() {
        return 1.0;
    }
    let intersection = a_bigrams.intersection(&b_bigrams).count();
    let union = a_bigrams.union(&b_bigrams).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

// ── A statement against a recorded decision ─────────────────────────────
//
// The in-chat check (`crate::chat_check`) asks whether one sentence an AI
// tool just wrote goes against a decision memory. It reuses the decision
// comparison above: the sentence is turned into the same `{decision,
// context}` fields a new decision memory would carry, and
// `fields_may_contradict("decision", …)` makes the final call. What is added
// here is only the cheap, deterministic reading of a sentence: which option
// it adopts and which it drops. No LLM and no embedding, so the check answers
// inside the hook's one-second budget.

/// Why a statement goes against a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionConflict {
    /// The statement adopts an option the decision turned down.
    AdoptsRejected { option: String },
    /// The statement drops the option the decision chose.
    DropsChosen { option: String },
}

impl DecisionConflict {
    pub fn option(&self) -> &str {
        match self {
            Self::AdoptsRejected { option } | Self::DropsChosen { option } => option,
        }
    }
}

/// What a decision memory chose and turned down.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionTerms {
    /// The decision as stated (structured `decision`, else the content).
    pub decision: String,
    pub context: Option<String>,
    /// Options the decision adopts (`use X`, or `X instead of Y` → X).
    pub chosen: Vec<String>,
    /// Options the decision turned down (`instead of Y`, `alternatives_considered`).
    pub rejected: Vec<String>,
}

/// Verbs that adopt the option after them.
const ADOPT_MARKERS: &[&str] = &[
    "keep using",
    "keep",
    "stick with",
    "stay with",
    "stay on",
    "switch to",
    "switching to",
    "switched to",
    "move to",
    "moving to",
    "migrate to",
    "migrating to",
    "go with",
    "going with",
    "went with",
    "replace it with",
    "adopt",
    "adopting",
    "use",
    "using",
    "chose",
    "choose",
    "pick",
    "picked",
    "改用",
    "換成",
    "换成",
    "改成",
    "採用",
    "采用",
    "使用",
    "選擇",
    "选择",
    "繼續用",
    "继续用",
    "保留",
    "用",
];

/// Markers that turn down the option after them.
const REJECT_MARKERS: &[&str] = &[
    "instead of",
    "rather than",
    "don't use",
    "do not use",
    "stop using",
    "no longer use",
    "get rid of",
    "drop",
    "remove",
    "avoid",
    "而不是",
    "而非",
    "不要用",
    "不再用",
    "不用",
    "不要",
    "別用",
    "别用",
    "放棄",
    "放弃",
    "拿掉",
    "移除",
    "不採用",
    "不采用",
    "不使用",
];

const LEADING_FILLERS: &[&str] = &["a ", "an ", "the ", "our ", "your ", "my "];

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF)
}

fn strip_filler(phrase: &str) -> &str {
    for filler in LEADING_FILLERS {
        if let Some(stripped) = phrase.strip_prefix(filler) {
            return stripped;
        }
    }
    phrase
}

/// Find `marker` in `lower` at a word boundary (CJK markers need none).
fn marker_positions(lower: &str, marker: &str) -> Vec<usize> {
    let cjk_marker = marker.chars().any(is_cjk);
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(marker) {
        let at = from + rel;
        let end = at + marker.len();
        let before_ok = cjk_marker
            || lower[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = cjk_marker
            || lower[end..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            found.push(end);
        }
        from = at + marker.len().max(1);
    }
    found
}

/// The option phrase that starts at `start`: up to punctuation, a clause
/// word, or a few words / CJK characters.
fn option_after(lower: &str, start: usize) -> Option<String> {
    let rest = lower[start..].trim_start();
    let mut phrase = strip_filler(rest);
    // `instead of switching to X`: the option is X, not the verb.
    let mut verbs: Vec<&&str> = ADOPT_MARKERS.iter().collect();
    verbs.sort_by_key(|marker| std::cmp::Reverse(marker.len()));
    for verb in verbs {
        if let Some(stripped) = phrase.strip_prefix(*verb) {
            let boundary = verb.chars().any(is_cjk)
                || stripped.chars().next().is_none_or(|c| !c.is_alphanumeric());
            if boundary {
                phrase = strip_filler(stripped.trim_start());
                break;
            }
        }
    }
    let mut out = String::new();
    let mut words = 0usize;
    let mut cjk = 0usize;
    for word in phrase.split_inclusive(char::is_whitespace) {
        let bare = word.trim();
        if bare.is_empty() {
            continue;
        }
        if matches!(
            bare,
            "for"
                | "to"
                | "in"
                | "and"
                | "but"
                | "because"
                | "so"
                | "since"
                | "as"
                | "with"
                | "instead"
                | "rather"
                | "over"
                | "here"
                | "now"
                | "then"
                | "again"
                | "from"
        ) {
            break;
        }
        let mut stop = false;
        for c in word.chars() {
            if matches!(
                c,
                '.' | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '"'
                    | '\n'
                    | '。'
                    | '，'
                    | '、'
                    | '；'
                    | '：'
                    | '！'
                    | '？'
                    | '（'
                    | '）'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
            ) {
                stop = true;
                break;
            }
            if is_cjk(c) {
                // A CJK clause marker ends the option.
                if matches!(
                    c,
                    '而' | '來' | '来' | '的' | '吧' | '了' | '嗎' | '吗' | '做' | '當' | '当'
                ) {
                    stop = true;
                    break;
                }
                cjk += 1;
                if cjk > 10 {
                    stop = true;
                    break;
                }
            }
            out.push(c);
        }
        words += 1;
        if stop || words >= 4 {
            break;
        }
    }
    let option = out
        .trim()
        .trim_matches(|c: char| c == '`' || c == '\'' || c == '*')
        .trim()
        .to_string();
    let meaningful = option.chars().filter(|c| c.is_alphanumeric()).count();
    let cjk_count = option.chars().filter(|c| is_cjk(*c)).count();
    (meaningful >= 3 || cjk_count >= 2).then_some(option)
}

/// Options adopted and turned down in one sentence. A rejected span is
/// cut out before adoptions are read, so `use X instead of Y` adopts only X.
fn read_options(sentence: &str) -> (Vec<String>, Vec<String>) {
    let lower = sentence.to_lowercase();
    let mut rejected = Vec::new();
    let mut reject_spans: Vec<(usize, usize)> = Vec::new();
    for marker in REJECT_MARKERS {
        for end in marker_positions(&lower, marker) {
            if let Some(option) = option_after(&lower, end) {
                let start = end - marker.len();
                reject_spans.push((start, end + option.len() + 1));
                if !rejected.contains(&option) {
                    rejected.push(option);
                }
            }
        }
    }
    let mut adopted = Vec::new();
    for marker in ADOPT_MARKERS {
        for end in marker_positions(&lower, marker) {
            let start = end - marker.len();
            if reject_spans.iter().any(|(s, e)| start >= *s && start < *e) {
                continue;
            }
            if let Some(option) = option_after(&lower, end) {
                if !adopted.contains(&option) && !rejected.iter().any(|r| options_match(r, &option))
                {
                    adopted.push(option);
                }
            }
        }
    }
    (adopted, rejected)
}

fn normalize_option(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether two option phrases name the same thing: one contains the other
/// (at least three letters or two CJK characters), or close bigram overlap.
pub fn options_match(a: &str, b: &str) -> bool {
    let a = normalize_option(a);
    let b = normalize_option(b);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (short, long) = if a.chars().count() <= b.chars().count() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    let short_cjk = short.chars().filter(|c| is_cjk(*c)).count();
    let long_enough = short.chars().filter(|c| c.is_alphanumeric()).count() >= 3 || short_cjk >= 2;
    if long_enough {
        let bounded = if short_cjk > 0 {
            long.contains(short.as_str())
        } else {
            long.split_whitespace()
                .collect::<Vec<_>>()
                .windows(short.split_whitespace().count().max(1))
                .any(|w| w.join(" ") == *short)
        };
        if bounded {
            return true;
        }
    }
    bigram_jaccard(&a, &b) >= 0.6
}

fn split_list(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => s
            .split([',', ';', '、', '，', '/'])
            .flat_map(|part| part.split(" or "))
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect(),
        Value::Array(items) => items.iter().flat_map(split_list).collect(),
        _ => Vec::new(),
    }
}

/// Read a decision memory: its structured fields when present, its content
/// otherwise (and always for `instead of` style rejections).
pub fn decision_terms(structured_fields: Option<&str>, content: &str) -> DecisionTerms {
    let fields: serde_json::Map<String, Value> = structured_fields
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    let decision = get_str(&fields, "decision")
        .map(str::to_string)
        .unwrap_or_else(|| content.trim().to_string());
    let context = get_str(&fields, "context").map(str::to_string);
    let mut chosen: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for text in [decision.as_str(), content] {
        let (adopted, turned_down) = read_options(text);
        for option in adopted {
            if !chosen.iter().any(|c| options_match(c, &option)) {
                chosen.push(option);
            }
        }
        for option in turned_down {
            if !rejected.iter().any(|r| options_match(r, &option)) {
                rejected.push(option);
            }
        }
    }
    if let Some(alternatives) = fields.get("alternatives_considered") {
        for option in split_list(alternatives) {
            let is_chosen = chosen.iter().any(|c| options_match(c, &option))
                || options_match(&decision, &option);
            if !is_chosen && !rejected.iter().any(|r| options_match(r, &option)) {
                rejected.push(option);
            }
        }
    }
    DecisionTerms {
        decision,
        context,
        chosen,
        rejected,
    }
}

/// Whether `sentence` goes against the decision in `terms`. `None` when it
/// agrees, restates it, or is about something else.
pub fn statement_contradicts_decision(
    terms: &DecisionTerms,
    sentence: &str,
) -> Option<DecisionConflict> {
    let (adopted, dropped) = read_options(sentence);
    let conflict = adopted
        .iter()
        .find_map(|option| {
            let restates = terms.chosen.iter().any(|c| options_match(c, option));
            let turned_down = terms.rejected.iter().any(|r| options_match(r, option));
            (turned_down && !restates).then(|| DecisionConflict::AdoptsRejected {
                option: option.clone(),
            })
        })
        .or_else(|| {
            dropped.iter().find_map(|option| {
                let drops_chosen = terms.chosen.iter().any(|c| options_match(c, option));
                drops_chosen.then(|| DecisionConflict::DropsChosen {
                    option: option.clone(),
                })
            })
        })?;
    // Final word: the same decision comparison a new decision memory gets.
    // The sentence's position becomes the new `decision`, judged in the
    // decision's own context.
    let position = match &conflict {
        DecisionConflict::AdoptsRejected { option } => option.clone(),
        DecisionConflict::DropsChosen { option } => format!("not {option}"),
    };
    let existing = serde_json::json!({
        "decision": terms.decision,
        "context": terms.context.clone().unwrap_or_else(|| terms.decision.clone()),
    });
    let incoming = serde_json::json!({
        "decision": position,
        "context": terms.context.clone().unwrap_or_else(|| terms.decision.clone()),
    });
    fields_may_contradict("decision", &existing.to_string(), &incoming.to_string())
        .then_some(conflict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(fields: Option<&str>, content: &str) -> DecisionTerms {
        decision_terms(fields, content)
    }

    #[test]
    fn reads_chosen_and_rejected_options_from_a_decision() {
        let t = terms(
            None,
            "We decided to keep libSQL instead of switching to a dedicated graph database",
        );
        assert!(t.chosen.iter().any(|c| options_match(c, "libsql")), "{t:?}");
        assert!(
            t.rejected
                .iter()
                .any(|r| options_match(r, "dedicated graph database")),
            "{t:?}"
        );
        let t = terms(
            Some(
                r#"{"decision":"Use Postgres","context":"storage","alternatives_considered":"MySQL, MongoDB"}"#,
            ),
            "Use Postgres for storage",
        );
        assert!(t.chosen.iter().any(|c| options_match(c, "postgres")));
        assert!(t.rejected.iter().any(|r| options_match(r, "mongodb")));
        assert!(t.rejected.iter().any(|r| options_match(r, "mysql")));
        assert_eq!(t.context.as_deref(), Some("storage"));
    }

    #[test]
    fn a_sentence_adopting_a_rejected_option_conflicts() {
        let t = terms(None, "Use libSQL instead of a dedicated graph database.");
        let conflict = statement_contradicts_decision(
            &t,
            "Let's switch to a dedicated graph database for the entity store.",
        );
        assert!(
            matches!(conflict, Some(DecisionConflict::AdoptsRejected { .. })),
            "{conflict:?}"
        );
    }

    #[test]
    fn a_sentence_dropping_the_chosen_option_conflicts() {
        let t = terms(None, "Use libSQL instead of a dedicated graph database.");
        let conflict =
            statement_contradicts_decision(&t, "We should stop using libSQL and drop it.");
        assert!(
            matches!(conflict, Some(DecisionConflict::DropsChosen { .. })),
            "{conflict:?}"
        );
    }

    #[test]
    fn restating_or_unrelated_sentences_do_not_conflict() {
        let t = terms(None, "Use libSQL instead of a dedicated graph database.");
        for sentence in [
            "We use libSQL instead of a dedicated graph database.",
            "Keep using libSQL.",
            "The tests use tokio for async.",
            "I updated the README.",
            "",
        ] {
            assert_eq!(
                statement_contradicts_decision(&t, sentence),
                None,
                "{sentence}"
            );
        }
    }

    #[test]
    fn chinese_decisions_are_read_too() {
        let t = terms(None, "資料庫用 libSQL，而不是 Neo4j。");
        assert!(
            t.rejected.iter().any(|r| options_match(r, "neo4j")),
            "{t:?}"
        );
        let conflict = statement_contradicts_decision(&t, "我們改用 Neo4j 來存圖。");
        assert!(
            matches!(conflict, Some(DecisionConflict::AdoptsRejected { .. })),
            "{conflict:?}"
        );
        assert_eq!(statement_contradicts_decision(&t, "繼續用 libSQL。"), None);
    }

    #[test]
    fn option_matching_respects_word_boundaries() {
        assert!(options_match(
            "graph database",
            "a dedicated graph database"
        ));
        assert!(!options_match("sql", "nosql store"));
        assert!(options_match("Neo4j", "neo4j"));
        assert!(!options_match("go", "mongo"));
    }

    #[test]
    fn test_preference_contradiction() {
        let existing = r#"{"preference":"dark mode","applies_when":"editors"}"#;
        let new = r#"{"preference":"light mode","applies_when":"editors"}"#;
        assert!(fields_may_contradict("preference", existing, new));
    }

    #[test]
    fn test_preference_no_contradiction_different_context() {
        let existing = r#"{"preference":"dark mode","applies_when":"editors"}"#;
        let new = r#"{"preference":"light mode","applies_when":"reading"}"#;
        assert!(!fields_may_contradict("preference", existing, new));
    }

    #[test]
    fn test_identity_contradiction() {
        let existing = r#"{"claim":"I am a Python developer"}"#;
        let new = r#"{"claim":"I am a Rust developer"}"#;
        assert!(fields_may_contradict("identity", existing, new));
    }

    #[test]
    fn test_fact_contradiction() {
        let existing = r#"{"claim":"libSQL uses SQLite","domain":"databases"}"#;
        let new = r#"{"claim":"libSQL uses PostgreSQL","domain":"databases"}"#;
        assert!(fields_may_contradict("fact", existing, new));
    }

    #[test]
    fn test_goal_status_update() {
        let existing = r#"{"objective":"launch Wenlan","status":"in_progress"}"#;
        let new = r#"{"objective":"launch Wenlan","status":"completed"}"#;
        assert!(fields_may_contradict("goal", existing, new));
    }

    #[test]
    fn test_invalid_json_returns_false() {
        assert!(!fields_may_contradict(
            "preference",
            "not json",
            "also not json"
        ));
    }

    #[test]
    fn test_missing_fields_returns_false() {
        let existing = r#"{"preference":"dark mode"}"#;
        let new = r#"{"other_field":"something"}"#;
        assert!(!fields_may_contradict("preference", existing, new));
    }
}
