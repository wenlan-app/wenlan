// SPDX-License-Identifier: Apache-2.0
//! Dry run of "what you're working on" from AI-tool transcripts: the same
//! scan, rules, prompts and parsing the daemon uses
//! (`wenlan_core::conversation_intake`), with no database. Reads the
//! transcripts on this computer and writes only `$THINGS_OUT/things.json`.
//!
//! ```sh
//! LABEL_URL=http://127.0.0.1:11434/v1 LABEL_MODEL=qwen2.5:14b \
//! SPACES=wenlan,writing UI_LANGUAGE=zh-Hant THINGS_OUT=/tmp/things \
//! cargo run -p wenlan-core --example things_intake_dry_run
//! ```
//!
//! Grouping uses the first of Claude Code and Codex installed, as the daemon
//! does when no cloud model is set. `COMPARE=1` also groups the same labels
//! a second time with what each conversation worked on hidden, into
//! `things-titles-only.json`, to compare with titles alone.

use std::collections::BTreeSet;
use std::sync::Arc;

use wenlan_core::agent_cli_llm::AgentCliProvider;
use wenlan_core::conversation_intake::{self as intake, OpenThing, Pending};
use wenlan_core::llm_provider::{LlmProvider, LlmRequest, OpenAICompatibleProvider};

struct Thing {
    title: String,
    space: String,
    about: String,
    status: String,
    line: Option<String>,
    last: i64,
    tools: BTreeSet<String>,
    parts: Vec<String>,
    touched: Vec<String>,
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

#[tokio::main]
async fn main() {
    let things_out = std::path::PathBuf::from(env("THINGS_OUT"));
    let spaces: Vec<String> = env("SPACES")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let compare = std::env::var("COMPARE").is_ok_and(|v| v == "1");
    let language = intake::language_name(std::env::var("UI_LANGUAGE").ok().as_deref());
    let label: Arc<dyn LlmProvider> = Arc::new(OpenAICompatibleProvider::new_with_key(
        env("LABEL_URL"),
        env("LABEL_MODEL"),
        None,
    ));
    let group: Arc<dyn LlmProvider> =
        Arc::new(AgentCliProvider::find().expect("Claude Code or Codex is not installed"));

    let roots = wenlan_types::session_context::TranscriptRoots::from_env(dirs::home_dir());
    let now = chrono::Utc::now().timestamp();
    let found = intake::scan(&roots, now, intake::INTAKE_WINDOW_SECS);
    let found_count = found.len();
    let (kept, dropped) = intake::keep_work(found);
    eprintln!(
        "found {found_count}, kept {}, dropped {dropped:?}",
        kept.len()
    );

    let mut work: Vec<(Pending, intake::Label, i64, String)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (i, conv) in kept.iter().enumerate() {
        let answer = label
            .generate(LlmRequest {
                system_prompt: Some("You describe a person's conversations with AI tools. Answer with one line of JSON only.".into()),
                user_prompt: intake::label_prompt(conv, &spaces, language),
                max_tokens: 400,
                temperature: 0.0,
                label: Some("things_label".into()),
                timeout_secs: Some(120),
            })
            .await;
        eprintln!("labelled {}/{}", i + 1, kept.len());
        match answer
            .ok()
            .and_then(|text| intake::parse_label(&text, &spaces))
        {
            Some(l) if l.work => work.push((
                Pending {
                    session_id: format!("{}:{}", conv.tool, conv.external_id),
                    title: l.title.clone(),
                    about: l.about.clone(),
                    space: l.space.clone(),
                    status: l.status.clone(),
                    touched: conv.touched.clone(),
                },
                l,
                conv.last_at,
                conv.tool.clone(),
            )),
            Some(l) => skipped.push(l.title),
            None => skipped.push("(model answer unreadable)".into()),
        }
    }

    work.sort_by_key(|w| w.2);
    let runs: &[(&str, bool)] = if compare {
        &[("things.json", false), ("things-titles-only.json", true)]
    } else {
        &[("things.json", false)]
    };
    std::fs::create_dir_all(&things_out).unwrap();
    for &(file, titles_only) in runs {
        let mut work = work.clone();
        if titles_only {
            for w in &mut work {
                w.0.touched.clear();
            }
        }
        let (things, left_out) = group_all(group.as_ref(), &work, language).await;
        write_things(
            &things_out.join(file),
            found_count,
            &kept,
            &dropped,
            &work,
            things,
            left_out,
            &skipped,
        );
    }
}

type Work = (Pending, intake::Label, i64, String);

/// Oldest first, in the daemon's batches, each seeing the things so far.
async fn group_all(
    group: &dyn LlmProvider,
    work: &[Work],
    language: &str,
) -> (Vec<Thing>, Vec<String>) {
    let mut things: Vec<Thing> = Vec::new();
    let mut left_out: Vec<String> = Vec::new();
    for batch in work.chunks(60) {
        let pending: Vec<Pending> = batch.iter().map(|w| w.0.clone()).collect();
        let open: Vec<OpenThing> = things
            .iter()
            .enumerate()
            .map(|(i, t)| OpenThing {
                id: i.to_string(),
                title: t.title.clone(),
                about: t.about.clone(),
                space: t.space.clone(),
                news: Vec::new(),
                touched: t.touched.iter().take(12).cloned().collect(),
            })
            .collect();
        let answer = group
            .generate(LlmRequest {
                system_prompt: Some("You sort a person's conversations into the pieces of work they belong to. Answer with JSON only.".into()),
                user_prompt: intake::group_prompt(&pending, &open, &[], language),
                max_tokens: 4_000,
                temperature: 0.0,
                label: Some("things_group".into()),
                timeout_secs: Some(300),
            })
            .await
            .expect("grouping");
        let grouping =
            intake::parse_grouping(&answer, &pending, open.len()).expect("grouping answer");
        let add = |things: &mut Vec<Thing>, thing: usize, c: usize| {
            let (p, l, at, tool) = &batch[c];
            let t = &mut things[thing];
            t.tools.insert(tool.clone());
            t.parts.push(p.title.clone());
            for name in &p.touched {
                if !t.touched.contains(name) {
                    t.touched.push(name.clone());
                }
            }
            if *at >= t.last {
                t.last = *at;
                t.about = l.about.clone();
                t.status = l.status.clone();
                if !l.space.is_empty() {
                    t.space = l.space.clone();
                }
            }
        };
        for &(c, t) in &grouping.joins {
            add(&mut things, t, c);
        }
        let first_new = things.len();
        for (title, members) in &grouping.new {
            things.push(Thing {
                title: title.clone(),
                space: String::new(),
                about: String::new(),
                status: String::new(),
                line: None,
                last: 0,
                tools: BTreeSet::new(),
                parts: Vec::new(),
                touched: Vec::new(),
            });
            let i = things.len() - 1;
            for &c in members {
                add(&mut things, i, c);
            }
        }
        for (piece, line) in &grouping.lines {
            let i = match *piece {
                intake::Piece::Open(t) => t,
                intake::Piece::New(n) => first_new + n,
            };
            if let Some(t) = things.get_mut(i) {
                t.line = Some(line.clone());
            }
        }
        left_out.extend(grouping.left_out.iter().map(|&c| batch[c].0.title.clone()));
        eprintln!("grouped {} conversations", pending.len());
    }

    (things, left_out)
}

#[allow(clippy::too_many_arguments)]
fn write_things(
    path: &std::path::Path,
    found_count: usize,
    kept: &[intake::FoundConversation],
    dropped: &intake::Dropped,
    work: &[Work],
    mut things: Vec<Thing>,
    left_out: Vec<String>,
    skipped: &[String],
) {
    things.sort_by_key(|t| std::cmp::Reverse(t.last));
    let json = serde_json::json!({
        "found": found_count,
        "kept": kept.len(),
        "dropped": format!("{dropped:?}"),
        "work": work.len(),
        "things": things.iter().map(|t| serde_json::json!({
            "title": t.title, "space": t.space, "status": t.line.as_ref().unwrap_or(&t.status),
            "last_status": t.status, "about": t.about,
            "last": chrono::DateTime::from_timestamp(t.last, 0).map(|d| d.to_rfc3339()),
            "tools": t.tools, "conversations": t.parts.len(), "parts": t.parts,
        })).collect::<Vec<_>>(),
        "left_out": left_out,
        "skipped": skipped,
    });
    std::fs::write(path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    eprintln!("{} things; wrote {}", things.len(), path.display());
}
