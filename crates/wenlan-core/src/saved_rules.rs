// SPDX-License-Identifier: Apache-2.0
//! The decisions behind "save this as a rule" (UX-SKILL-14) that need no
//! database: who a save is recorded as coming from, how many a day, which
//! Space it may be for, and who may remove it. The daemon's routes
//! (`wenlan-server/src/skill_grow/saved_rules.rs`) do the reading and the
//! writing around them.
//!
//! Nothing here can prove a person spoke. A tool's name is what the tool
//! reported about itself, and "the terminal" only means the `wenlan` command
//! was run with no tool name: an AI tool with a shell can do that too. So
//! these checks bound what any one caller can do; they never vouch for it.

use chrono::TimeZone;

use crate::db::skill_grow::SavedRule;
use wenlan_types::{
    RULE_REMOVED_IN_APP, SAVED_FROM_TERMINAL, SAVED_RULES_PER_DAY, SAVED_RULES_PER_TOOL_PER_DAY,
};

/// Where a save came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saver {
    /// What the day's count and the right to remove are kept by: the name
    /// the caller reported, trimmed and lower-cased, so `Codex`, `codex`
    /// and ` CODEX ` are one tool. [`SAVED_FROM_TERMINAL`] for no name.
    pub id: String,
    /// What rules lists show after "saved from".
    pub label: String,
}

impl Saver {
    pub fn is_terminal(&self) -> bool {
        self.id == SAVED_FROM_TERMINAL
    }

    /// The person pressing Remove in the Wenlan app themselves. The daemon
    /// gives this only to a request that carries the app's person marker
    /// and no tool name, while it listens on this computer alone.
    pub fn person_in_app() -> Self {
        Saver {
            id: RULE_REMOVED_IN_APP.to_string(),
            label: RULE_REMOVED_IN_APP.to_string(),
        }
    }

    pub fn is_person_in_app(&self) -> bool {
        self.id == RULE_REMOVED_IN_APP
    }
}

/// A reported tool name as the count is kept by.
pub fn tool_id(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A name with its case, spaces, hyphens, underscores and every other mark
/// taken out, so `Wenlan App`, `wenlan-app`, `wenlan_app` and `WenlanApp`
/// compare as one.
fn squashed(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Names no AI tool may be listed under: they read as the person, the
/// terminal or Wenlan itself.
fn reads_as_someone_else(label: &str) -> bool {
    matches!(
        squashed(label).as_str(),
        "terminal"
            | "theterminal"
            | "aterminal"
            | "cli"
            | "thecli"
            | "wenlan"
            | "wenlancli"
            | "wenlanapp"
            | "thewenlanapp"
            | "you"
            | "me"
            | "i"
            | "myself"
            | "yourself"
            | "user"
            | "theuser"
            | "person"
            | "theperson"
            | "owner"
            | "theowner"
            | "human"
    )
}

/// The start of the id every such tool is kept under, so its saves, its
/// count and its line in a page's history are never the terminal's, the
/// person's or the app's own, however it spells the name.
pub const SELF_NAMED_TOOL_ID_PREFIX: &str = "tool:";

/// The saver for a caller. `agent_name` is the `x-agent-name` the request
/// carried, if any; `display_name` is the name Wenlan lists that tool under,
/// if it knows one. A caller that names an AI tool is listed as that tool.
/// One that names none is listed as the terminal.
pub fn saver(agent_name: Option<&str>, display_name: Option<&str>) -> Saver {
    let named = agent_name.map(tool_id).filter(|name| !name.is_empty());
    let Some(id) = named else {
        return Saver {
            id: SAVED_FROM_TERMINAL.to_string(),
            label: SAVED_FROM_TERMINAL.to_string(),
        };
    };
    // The name goes into files every AI tool reads: letters, digits and a
    // few marks only, and short.
    let clean = |name: &str| -> String {
        name.chars()
            .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '/'))
            .take(40)
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let reported = clean(agent_name.unwrap_or_default());
    let shown = display_name
        .map(clean)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| reported.clone());
    let label = if shown.is_empty() {
        wenlan_types::SAVED_FROM_UNNAMED_TOOL.to_string()
    } else if reads_as_someone_else(&shown) || reads_as_someone_else(&reported) {
        // Never the terminal's label, the person's or Wenlan's: say it is an
        // AI tool, and give the name it reported in quotes.
        let name = if reported.is_empty() { shown } else { reported };
        wenlan_types::saved_from_self_named(&name)
    } else {
        shown
    };
    // A tool cannot take the terminal's count either, nor the name the
    // app's own Remove is recorded under, nor any near spelling of them
    // (`Wenlan App`, `wenlan_app`, `The Terminal`, `you`). A name that
    // already opens with the prefix gets it again, so no tool can pick
    // another's prefixed id.
    let raw = agent_name.unwrap_or_default();
    let id = if reads_as_someone_else(raw) || id.starts_with(SELF_NAMED_TOOL_ID_PREFIX) {
        format!("{SELF_NAMED_TOOL_ID_PREFIX}{id}")
    } else {
        id
    };
    Saver { id, label }
}

fn same_local_day(left: i64, right: i64) -> bool {
    chrono::Local
        .timestamp_opt(left, 0)
        .single()
        .zip(chrono::Local.timestamp_opt(right, 0).single())
        .is_some_and(|(left, right)| left.date_naive() == right.date_naive())
}

/// Why `saver` may not save one more rule today, if it may not. `saves` is
/// every save the ledger holds, removed rules and waiting ones included: a
/// removal does not give a save back.
pub fn day_limit_refusal<'a>(
    saves: impl IntoIterator<Item = &'a SavedRule>,
    saver: &Saver,
    now: i64,
) -> Option<String> {
    let (mut own, mut all) = (0usize, 0usize);
    for save in saves {
        if !same_local_day(save.at, now) {
            continue;
        }
        all += 1;
        if tool_id(&save.tool_id) == saver.id {
            own += 1;
        }
    }
    if own >= SAVED_RULES_PER_TOOL_PER_DAY {
        return Some(format!(
            "{SAVED_RULES_PER_TOOL_PER_DAY} rules were already saved from here today, which is \
             the most for one day; nothing was saved. The person can write more on a page in a \
             folder named Rules, or save again tomorrow"
        ));
    }
    if all >= SAVED_RULES_PER_DAY {
        return Some(format!(
            "{SAVED_RULES_PER_DAY} rules were already saved today from all tools together, \
             which is the most for one day; nothing was saved. The person can write more on a \
             page in a folder named Rules, or save again tomorrow"
        ));
    }
    None
}

/// Which Space a save is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveScope {
    EverySpace,
    /// The Space named this way; the daemon resolves the name.
    Space(String),
}

pub const RULE_NEEDS_A_SPACE: &str =
    "say which Space this rule is for, or ask for every Space (every_space, `--every-space`); \
     nothing was saved";

/// Decide a save's Space. `locked` is the Space the connection is limited
/// to (the `x-wenlan-space` header), `asked` the Space the request names.
/// A rule is for every Space only when asked for by name, and never over a
/// connection limited to one Space; with no Space and no such ask it is
/// refused.
pub fn save_scope(
    locked: Option<&str>,
    asked: Option<&str>,
    every_space: bool,
) -> Result<SaveScope, String> {
    fn name(value: Option<&str>) -> Option<&str> {
        value.map(str::trim).filter(|value| !value.is_empty())
    }
    let (locked, asked) = (name(locked), name(asked));
    if let Some(locked) = locked {
        if every_space {
            return Err(format!(
                "this connection is limited to the Space {locked:?}, so it cannot save a rule \
                 for every Space; nothing was saved. Save it for that Space, or ask the person \
                 to run `wenlan rules add \"<rule>\" --every-space` themselves"
            ));
        }
        if asked.is_some_and(|asked| asked != locked) {
            return Err(format!(
                "this connection is limited to the Space {locked:?}, so it cannot save a rule \
                 for another Space; nothing was saved"
            ));
        }
        return Ok(SaveScope::Space(locked.to_string()));
    }
    match (every_space, asked) {
        (true, Some(_)) => Err(
            "a rule is for one Space or for every Space, not both; nothing was saved".to_string(),
        ),
        (true, None) => Ok(SaveScope::EverySpace),
        (false, Some(asked)) => Ok(SaveScope::Space(asked.to_string())),
        (false, None) => Err(RULE_NEEDS_A_SPACE.to_string()),
    }
}

/// Why `saver` may not remove the rule `record` saved, if it may not. The
/// terminal removes any saved rule, and so does the person in the app. An
/// AI tool removes only what was saved under its own name.
pub fn removal_refusal(saver: &Saver, record: &SavedRule) -> Option<String> {
    if saver.is_terminal() || saver.is_person_in_app() || tool_id(&record.tool_id) == saver.id {
        return None;
    }
    Some(format!(
        "this rule was {}, and an AI tool removes only the rules saved under its own name; \
         nothing was removed. The person can remove it with `wenlan rules remove <id>` in a \
         terminal, or by deleting its line on the page",
        wenlan_types::saved_from_label(&record.tool)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(tool_id: &str, at: i64) -> SavedRule {
        SavedRule {
            id: format!("{tool_id}-{at}"),
            text: "x".into(),
            tool: tool_id.into(),
            tool_id: tool_id.into(),
            person_asked: "save it".into(),
            at,
            version: Some(2),
            card: None,
        }
    }

    /// UX-SKILL-14: a tool's count is kept by its name without case or
    /// padding, so a tool cannot get a new allowance by respelling itself.
    #[test]
    fn ux_skill_14_case_variants_of_a_tool_name_share_one_count() {
        let now = 1_794_000_000;
        let saves: Vec<SavedRule> = (0..10)
            .map(|n| record(["Codex", "codex", " CODEX "][n % 3], now))
            .collect();
        for name in ["codex", "CODEX", "  Codex "] {
            let saver = saver(Some(name), None);
            assert_eq!(saver.id, "codex");
            assert!(day_limit_refusal(&saves, &saver, now).is_some(), "{name}");
        }
        // Another tool, and the terminal, still have their own.
        assert!(day_limit_refusal(&saves, &saver(Some("cursor"), None), now).is_none());
        assert!(day_limit_refusal(&saves, &saver(None, None), now).is_none());
        // Yesterday's saves do not count.
        let old: Vec<SavedRule> = (0..10).map(|_| record("codex", now - 3 * 86_400)).collect();
        assert!(day_limit_refusal(&old, &saver(Some("codex"), None), now).is_none());
    }

    /// UX-SKILL-14: all callers together have a day's limit too, so many
    /// made-up tool names do not add up to many rules.
    #[test]
    fn ux_skill_14_all_tools_together_have_a_days_limit() {
        let now = 1_794_000_000;
        let mut saves: Vec<SavedRule> = (0..SAVED_RULES_PER_DAY - 1)
            .map(|n| record(&format!("tool-{n}"), now))
            .collect();
        let fresh = saver(Some("brand-new"), None);
        assert!(day_limit_refusal(&saves, &fresh, now).is_none());
        saves.push(record("one-more", now));
        let refusal = day_limit_refusal(&saves, &fresh, now).expect("the day is full");
        assert!(refusal.contains("all tools together"), "{refusal}");
        assert!(day_limit_refusal(&saves, &saver(None, None), now).is_some());
    }

    /// UX-SKILL-14: no AI tool is listed as the terminal, the person or
    /// Wenlan, whatever it calls itself, and it does not share their count.
    #[test]
    fn ux_skill_14_a_tool_cannot_take_the_terminals_label() {
        assert_eq!(
            saver(None, None),
            Saver {
                id: "terminal".into(),
                label: "terminal".into()
            }
        );
        for name in [
            "terminal",
            "Terminal",
            "the terminal",
            " The  Terminal ",
            "you",
            "Wenlan",
            "the-terminal",
            "the user",
        ] {
            let tool = saver(Some(name), None);
            assert!(!tool.is_terminal(), "{name}");
            assert_ne!(tool.label, SAVED_FROM_TERMINAL, "{name}");
            assert!(
                tool.label.starts_with("an AI tool that calls itself \""),
                "{name}: {}",
                tool.label
            );
            assert!(
                !wenlan_types::saved_from_label(&tool.label).contains("saved from the terminal"),
                "{name}"
            );
        }
        // Its saves, its count and its history line are kept apart too.
        for name in [
            "terminal",
            "Terminal",
            "The-Terminal",
            "the_terminal",
            "you",
            "YOU",
            "wenlan-app",
            "Wenlan App",
            "wenlan app",
            "wenlan_app",
            "WenlanApp",
            " WENLAN  APP ",
        ] {
            let tool = saver(Some(name), None);
            assert_eq!(
                tool.id,
                format!("tool:{}", tool_id(name)),
                "{name}: a near spelling of a reserved name is kept as a tool's"
            );
            assert!(!tool.is_terminal() && !tool.is_person_in_app(), "{name}");
            assert!(
                tool.label.starts_with("an AI tool that calls itself \""),
                "{name}: {}",
                tool.label
            );
        }
        // An ordinary tool keeps its plain id, and no tool can name itself
        // into another's prefixed one.
        assert_eq!(saver(Some("Codex"), None).id, "codex");
        assert_eq!(saver(Some("tool:terminal"), None).id, "tool:tool:terminal");
        // A name in another script is a tool's name like any other.
        let named = saver(Some("文瀾助手"), None);
        assert_eq!(
            (named.id.as_str(), named.label.as_str()),
            ("文瀾助手", "文瀾助手")
        );
        assert!(!named.is_terminal());
        // A name with nothing to show is still an AI tool, never the terminal.
        let marks = saver(Some("\u{fffd}\u{fffd}"), None);
        assert_eq!(marks.label, "an AI tool");
        assert!(!marks.is_terminal());
        // A display name does not get it there either.
        let dressed = saver(Some("helper"), Some("the terminal"));
        assert_eq!(dressed.label, "an AI tool that calls itself \"helper\"");
        // An ordinary tool is listed by the name Wenlan knows it under.
        assert_eq!(
            saver(Some("codex"), Some("ChatGPT / Codex")).label,
            "ChatGPT / Codex"
        );
        // What goes into files is plain: no marks a tool's file reads.
        assert_eq!(
            saver(Some("x<!--@y`"), None).label,
            "x--y",
            "only letters, digits and a few marks"
        );
    }

    /// UX-SKILL-14: every Space only when asked for by name, never over a
    /// connection limited to one Space, and never by leaving the Space out.
    #[test]
    fn ux_skill_14_a_connection_limited_to_one_space_saves_only_there() {
        assert_eq!(save_scope(None, None, true), Ok(SaveScope::EverySpace));
        assert_eq!(
            save_scope(None, Some("Clinic"), false),
            Ok(SaveScope::Space("Clinic".into()))
        );
        assert_eq!(
            save_scope(Some("Clinic"), None, false),
            Ok(SaveScope::Space("Clinic".into()))
        );
        assert_eq!(
            save_scope(Some("Clinic"), Some("Clinic"), false),
            Ok(SaveScope::Space("Clinic".into()))
        );
        assert_eq!(
            save_scope(None, None, false),
            Err(RULE_NEEDS_A_SPACE.to_string())
        );
        assert!(save_scope(Some("Clinic"), None, true)
            .unwrap_err()
            .contains("limited to the Space \"Clinic\""));
        assert!(save_scope(Some("Clinic"), Some("Other"), false)
            .unwrap_err()
            .contains("another Space"));
        assert!(save_scope(None, Some("Clinic"), true).is_err());
    }

    /// UX-SKILL-14: an AI tool removes only what was saved under its own
    /// name; the terminal removes any saved rule.
    #[test]
    fn ux_skill_14_a_tool_removes_only_its_own_saves() {
        let by_codex = record("Codex", 1);
        let by_terminal = record("terminal", 1);
        assert!(removal_refusal(&saver(Some("codex"), None), &by_codex).is_none());
        assert!(removal_refusal(&saver(None, None), &by_codex).is_none());
        assert!(removal_refusal(&saver(None, None), &by_terminal).is_none());
        assert!(removal_refusal(&saver(Some("cursor"), None), &by_codex).is_some());
        for name in ["codex", "terminal", "Terminal"] {
            let refusal = removal_refusal(&saver(Some(name), None), &by_terminal);
            assert!(
                refusal.is_some_and(|text| text.contains("saved from the terminal")),
                "{name}"
            );
        }
    }

    /// UX-SKILL-14: the person pressing Remove in the app takes out any
    /// saved rule, and no AI tool can be recorded under the app's name.
    #[test]
    fn ux_skill_14_the_person_in_the_app_removes_any_saved_rule() {
        let person = Saver::person_in_app();
        assert!(removal_refusal(&person, &record("Codex", 1)).is_none());
        assert!(removal_refusal(&person, &record("terminal", 1)).is_none());
        for name in [
            "wenlan-app",
            "Wenlan-App",
            " WENLAN-APP ",
            "Wenlan App",
            "wenlan_app",
        ] {
            let tool = saver(Some(name), None);
            assert!(!tool.is_person_in_app(), "{name}");
            assert!(!tool.is_terminal(), "{name}");
            assert!(
                removal_refusal(&tool, &record("Codex", 1)).is_some(),
                "{name}"
            );
        }
    }
}
