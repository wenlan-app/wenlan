// SPDX-License-Identifier: Apache-2.0
//! What a night does with the notes a conversation read but did not cite.
//!
//! A note that cites a memory the night wrote already follows it (the page
//! refresh). A note that was only read in the conversation, by `read_page` or
//! because a recall returned it, is a candidate and nothing more: the night
//! changes one of its sentences only when a new point clearly contradicts that
//! sentence, and touches nothing else (design v5 F6, decision D15).
//!
//! Who wrote the sentence decides how it changes, by the rule the person
//! chose: the AI's own sentence on an unlocked note changes in place, through
//! the same machine write the automatic refresh uses; a sentence the person
//! wrote (or of unknown origin, or imported), and any sentence on a locked
//! note, only gets a proposal the person decides.

use std::collections::HashSet;

use wenlan_core::db::MemoryDB;
use wenlan_core::llm_provider::{LlmProvider, LlmRequest};
use wenlan_core::pages::Page;
use wenlan_types::authorship::{PageSentence, SentenceAuthor, SentenceProposeRequest};
use wenlan_types::requests::UpdatePageRequest;

/// Most read notes one Space group's night sends to the model. A cost guard
/// (one model call per note), not a measured number.
pub(crate) const READ_PAGES_CHECKED: usize = 8;

/// Most sentences of one note shown to the model, and the longest of them in
/// characters. Also a cost guard, not a measured number.
const SENTENCES_SHOWN: usize = 120;
const SENTENCE_CHARS: usize = 400;

/// The name the night's sentence proposals are filed under.
const NIGHT_AGENT: &str = "wenlan";

/// A point the night just kept, as the contradiction check sees it.
pub(crate) struct NewPoint {
    pub memory_id: String,
    pub text: String,
    /// The Space the point was kept in; a note is checked only against the
    /// points of its own Space.
    pub space: Option<String>,
    /// The conversation (its session id) the point came from. A note is
    /// checked only against the points of conversations that read it.
    pub session_id: String,
}

/// One sentence a point clearly contradicts, after validation.
#[derive(Debug, PartialEq)]
struct Contradiction {
    sentence: usize,
    point: usize,
    /// The replacement sentence; empty deletes it.
    text: String,
}

/// What came of one group's check.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ReadPagesReport {
    pub changed: usize,
    pub proposed: usize,
    /// Whole-page revision cards the write path staged instead of landing.
    pub staged: usize,
}

/// Check the notes `session_ids` read against `points`. `already_following`
/// holds the memories the group wrote or replaced: a note citing one of them
/// is not a candidate, the refresh follows it. A note is checked only against
/// the points of the conversations that read it, in its own Space.
/// `knowledge_path` is the pages folder the write projects into. The caller
/// has already ruled out a computer that may not write page text. A failure
/// on one note is logged and the next is tried; nothing here fails the night.
pub(crate) async fn check_read_pages(
    db: &MemoryDB,
    session_ids: &[&str],
    points: &[NewPoint],
    already_following: &HashSet<String>,
    model: &dyn LlmProvider,
    knowledge_path: Option<&std::path::Path>,
) -> ReadPagesReport {
    let mut report = ReadPagesReport::default();
    if points.is_empty() {
        return report;
    }
    // Each note with the conversations that read it, in the order first read.
    let mut readers: Vec<(String, HashSet<&str>)> = Vec::new();
    for session in session_ids {
        match db.session_pages_seen(session).await {
            Ok(ids) => {
                for id in ids {
                    match readers.iter_mut().find(|(seen, _)| *seen == id) {
                        Some((_, sessions)) => {
                            sessions.insert(session);
                        }
                        None => readers.push((id, HashSet::from([*session]))),
                    }
                }
            }
            Err(error) => tracing::warn!("[things] the notes {session} read: {error}"),
        }
    }
    let mut asked = 0;
    for (page_id, sessions) in readers {
        if asked >= READ_PAGES_CHECKED {
            break;
        }
        let page = match candidate(db, &page_id, already_following).await {
            Ok(Some(page)) => page,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!("[things] note {page_id} not checked: {error}");
                continue;
            }
        };
        let mut own: Vec<&NewPoint> = Vec::new();
        for point in points {
            if sessions.contains(point.session_id.as_str())
                && wenlan_core::sentences::same_space(
                    db,
                    point.space.as_deref(),
                    page.space.as_deref(),
                )
                .await
            {
                own.push(point);
            }
        }
        if own.is_empty() {
            continue;
        }
        asked += 1;
        match check_page(db, &page, &own, model, knowledge_path).await {
            Ok(done) => {
                report.changed += done.changed;
                report.proposed += done.proposed;
                report.staged += done.staged;
            }
            Err(error) => tracing::warn!("[things] note {page_id} left as it is: {error}"),
        }
    }
    report
}

/// The note, when it may be checked at all: it stands, an automatic reader
/// may read it, the person did not keep it from AI tools, and it does not
/// already cite a memory the group wrote.
async fn candidate(
    db: &MemoryDB,
    page_id: &str,
    already_following: &HashSet<String>,
) -> Result<Option<Page>, String> {
    let Some(page) = db.get_page(page_id).await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let Some(page) = wenlan_core::truth_adapter::filter_page(
        db,
        &wenlan_core::truth_contract::TruthGrant::Automatic,
        Some(page),
    )
    .await
    .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    if page.status != "active"
        || page.kind == "entity"
        || wenlan_core::skill_grow::page_is_not_for_ai_tools(&page.content)
        || page
            .source_memory_ids
            .iter()
            .any(|id| already_following.contains(id))
    {
        return Ok(None);
    }
    Ok(Some(page))
}

async fn check_page(
    db: &MemoryDB,
    page: &Page,
    points: &[&NewPoint],
    model: &dyn LlmProvider,
    knowledge_path: Option<&std::path::Path>,
) -> Result<ReadPagesReport, String> {
    let sentences = wenlan_core::sentences::page_sentences(db, &page.id)
        .await
        .map_err(|e| e.to_string())?;
    // What the model is shown, and the only thing its answer is checked
    // against. A sentence longer than the limit is left out whole rather than
    // cut, so a replacement never drops text the model did not see.
    let shown: Vec<&PageSentence> = sentences
        .sentences
        .iter()
        .filter(|s| !s.text.starts_with('#') && s.text.chars().count() <= SENTENCE_CHARS)
        .take(SENTENCES_SHOWN)
        .collect();
    if shown.is_empty() {
        return Ok(ReadPagesReport::default());
    }
    let answer = model
        .generate(LlmRequest {
            system_prompt: Some(
                "You check whether new points contradict sentences of a note. Answer with JSON only."
                    .into(),
            ),
            user_prompt: prompt(&shown, points),
            max_tokens: 1_000,
            temperature: 0.0,
            label: Some("things_read_page".into()),
            timeout_secs: Some(300),
        })
        .await
        .map_err(|e| e.to_string())?;
    let found = parse(&answer, &shown, points.len());
    let mut report = ReadPagesReport::default();
    // Before either path: a point that no longer stands (replaced or hidden
    // since it was kept) contradicts nothing.
    let mut standing: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    // Proposals first: they leave the page's version alone, so the version the
    // note was read at still holds for each of them.
    let mut in_place = Vec::new();
    for item in found {
        let sentence = shown[item.sentence];
        let point = points[item.point];
        let stands = match standing.get(point.memory_id.as_str()) {
            Some(stands) => *stands,
            None => {
                let stands = db.memory_stands(&point.memory_id).await.unwrap_or(false);
                standing.insert(point.memory_id.as_str(), stands);
                stands
            }
        };
        if !stands {
            tracing::warn!(
                "[things] point {} no longer stands; note {} is left as it is",
                point.memory_id,
                page.id
            );
            continue;
        }
        let ai_unlocked =
            sentence.author == SentenceAuthor::Ai && !sentence.protected && !page.review_required;
        if ai_unlocked {
            in_place.push((sentence, point, item.text));
            continue;
        }
        let request = SentenceProposeRequest {
            sentence_hash: sentence.hash.clone(),
            text: item.text,
            why: None,
            reason: None,
            expected_version: Some(sentences.version),
        };
        match wenlan_core::sentences::propose_contradicted_sentence(
            db,
            &page.id,
            &request,
            NIGHT_AGENT,
            &point.memory_id,
        )
        .await
        {
            Ok(Some(_)) => report.proposed += 1,
            Ok(None) => tracing::info!(
                "[things] a proposal for {} is already waiting or was refused; not filed again",
                page.id
            ),
            Err(error) => tracing::warn!("[things] a proposal for {} not filed: {error}", page.id),
        }
    }
    // The AI's own sentences, one write each, each fenced on the version the
    // note had just before it. A note that moved under us is left alone.
    let mut version = sentences.version;
    for (sentence, point, text) in in_place {
        match change_in_place(
            db,
            &page.id,
            version,
            &sentence.hash,
            &text,
            &point.memory_id,
            knowledge_path,
        )
        .await
        {
            Ok(InPlace::Wrote(next)) => {
                version = next;
                report.changed += 1;
            }
            Ok(InPlace::Moved) => {
                tracing::warn!(
                    "[things] note {} moved on; its sentence is left as it is",
                    page.id
                );
                break;
            }
            Ok(InPlace::Staged {
                cards,
                proposals,
                what,
            }) => {
                tracing::info!("[things] note {}: the write {what}", page.id);
                report.staged += cards;
                report.proposed += proposals;
                break;
            }
            Err(error) => {
                tracing::warn!("[things] note {} not changed: {error}", page.id);
                break;
            }
        }
    }
    Ok(report)
}

/// How a machine write of one sentence came out.
enum InPlace {
    /// It landed; the note is now at this version.
    Wrote(i64),
    /// The note was no longer at the version it was read at.
    Moved,
    /// The write path did not land it and staged something for the person
    /// instead (or nothing); `what` says which, from the write's own result.
    Staged {
        cards: usize,
        proposals: usize,
        what: String,
    },
}

/// Replace one sentence through the machine write the automatic refresh uses
/// (`re_distill`), so authorship, history and the sentence gate treat it as
/// any other refresh. The new point's memory joins the note's sources.
async fn change_in_place(
    db: &MemoryDB,
    page_id: &str,
    expected_version: i64,
    hash: &str,
    text: &str,
    memory_id: &str,
    knowledge_path: Option<&std::path::Path>,
) -> Result<InPlace, String> {
    let Some(current) = db.get_page(page_id).await.map_err(|e| e.to_string())? else {
        return Ok(InPlace::Moved);
    };
    if current.version != expected_version {
        return Ok(InPlace::Moved);
    }
    let (content, citations) =
        wenlan_core::sentences::body_with_replaced_sentence(&current, hash, text)
            .map_err(|e| e.to_string())?;
    let mut sources = current.source_memory_ids.clone();
    if !sources.iter().any(|id| id == memory_id) {
        sources.push(memory_id.to_string());
    }
    let result = wenlan_core::post_write::update_page(
        db,
        page_id,
        UpdatePageRequest {
            content,
            source_memory_ids: sources,
            expected_version: Some(expected_version),
            caller_id: None,
            operation_id: None,
        },
        "re_distill",
        false,
        knowledge_path,
        citations,
    )
    .await
    .map_err(|e| e.to_string())?;
    if result.wrote {
        let now = db
            .get_page(page_id)
            .await
            .map_err(|e| e.to_string())?
            .map_or(expected_version + 1, |page| page.version);
        return Ok(InPlace::Wrote(now));
    }
    if let Some(card) = result.revision_card_id {
        return Ok(InPlace::Staged {
            cards: 1,
            proposals: 0,
            what: format!("was staged as revision card {card} for the person"),
        });
    }
    if !result.sentence_proposals.is_empty() {
        return Ok(InPlace::Staged {
            cards: 0,
            proposals: result.sentence_proposals.len(),
            what: format!(
                "filed {} sentence proposal(s) instead of landing",
                result.sentence_proposals.len()
            ),
        });
    }
    Ok(InPlace::Staged {
        cards: 0,
        proposals: 0,
        what: format!(
            "did not land ({:?}){}",
            result.outcome,
            if result.warnings.is_empty() {
                String::new()
            } else {
                format!(": {}", result.warnings.join("; "))
            }
        ),
    })
}

fn prompt(sentences: &[&PageSentence], points: &[&NewPoint]) -> String {
    let mut out = String::from(
        "A conversation just settled the new points below. Here is a note the conversation \
         read. List only the sentences of the note that a new point clearly contradicts: the \
         point says the opposite, or a different value for the same thing. Do not list a \
         sentence a point merely adds to, rewords, or does not mention. When unsure, list \
         nothing.\n\nFor each, give the sentence number, the point number, and the sentence \
         as it should now read: one sentence, in the language of the original, with no \
         [1]-style markers. Use an empty string when the sentence is simply no longer true \
         and should go.\n\nAnswer with JSON only: \
         {\"contradicted\": [{\"sentence\": 0, \"point\": 0, \"text\": \"...\"}]}. \
         Answer {\"contradicted\": []} when nothing is contradicted.\n\nNew points:\n",
    );
    for (i, point) in points.iter().enumerate() {
        out.push_str(&format!("P{i}: {}\n", point.text));
    }
    out.push_str("\nSentences of the note:\n");
    for sentence in sentences {
        out.push_str(&format!("S{}: {}\n", sentence.idx, sentence.text));
    }
    out
}

/// The contradictions the answer names that hold up. Dropped: a sentence or
/// point that was not shown to the model, a heading, a replacement that reads as the
/// sentence already does, and a second change to the same sentence. A
/// replacement carrying a citation marker is dropped too: nothing the note
/// cites backs it.
fn parse(answer: &str, shown: &[&PageSentence], points: usize) -> Vec<Contradiction> {
    #[derive(serde::Deserialize)]
    struct Item {
        sentence: serde_json::Value,
        point: serde_json::Value,
        #[serde(default)]
        text: String,
    }
    #[derive(serde::Deserialize)]
    struct Answer {
        #[serde(default)]
        contradicted: Vec<Item>,
    }
    let Some(json) = wenlan_core::engine::extract_json(answer) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Answer>(json) else {
        return Vec::new();
    };
    // Accept 3, "3", "S3" and "P1" alike; the prompt names them so.
    let number = |value: &serde_json::Value| -> Option<usize> {
        match value {
            serde_json::Value::Number(n) => n.as_u64().map(|n| n as usize),
            serde_json::Value::String(s) => s.trim().trim_start_matches(['S', 'P']).parse().ok(),
            _ => None,
        }
    };
    let mut taken = HashSet::new();
    let mut out = Vec::new();
    for item in parsed.contradicted {
        let (Some(wanted), Some(point)) = (number(&item.sentence), number(&item.point)) else {
            continue;
        };
        // The number is the sentence's own `idx`, as the prompt printed it,
        // and only a sentence the prompt actually showed counts.
        let Some(position) = shown.iter().position(|s| s.idx as usize == wanted) else {
            continue;
        };
        let sentence = shown[position];
        let text = item.text.trim();
        if point >= points
            || sentence.text.starts_with('#')
            || wenlan_core::citations::strip_markers(text) != text
            || wenlan_core::authorship::normalize_sentence(text)
                == wenlan_core::authorship::normalize_sentence(&sentence.text)
            || !taken.insert(position)
        {
            continue;
        }
        out.push(Contradiction {
            sentence: position,
            point,
            text: text.to_string(),
        });
    }
    out
}
