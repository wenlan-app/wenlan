// SPDX-License-Identifier: Apache-2.0
//! The page that says how the person wants AI tools to work with them
//! (UX-HOW-1).
//!
//! It is an ordinary page with `role = instructions` (`type: page`, `role:
//! instructions` once pages are files). It is found by that role, never by
//! its title, and its title is in the person's language. When no such page
//! exists and none was ever made, it is written once from the person's
//! identity, preference, lesson and gotcha memories. It is born with the
//! review lock on, so an AI tool's change to it stages a revision card (a
//! proposal) and only the person's own edits land.

use crate::db::MemoryDB;
use crate::error::WenlanError;
use crate::pages::Page;
use crate::read_scope::ReadScope;
use std::path::Path;
use wenlan_types::chat_check::{page_link, InstructionsPage, INSTRUCTIONS_PAGE_ROLE};
use wenlan_types::MemoryItem;

/// Set once the page has been seeded, so a page the person deleted is not
/// written again.
pub const SEEDED_MARKER_KEY: &str = "instructions_page_seeded";

/// Memory types the page is seeded from, in the order they appear.
pub const SEED_MEMORY_TYPES: &[&str] = &["identity", "preference", "lesson", "gotcha"];
const SEED_PER_TYPE: usize = 25;
const SEED_LINE_CHARS: usize = 240;

/// Supported locales: `zh-Hant`, `zh-Hans`, `en`, `es`.
fn locale_key(locale: Option<&str>) -> &'static str {
    let lower = locale
        .map(|l| l.trim().to_ascii_lowercase().replace('_', "-"))
        .unwrap_or_default();
    if lower.starts_with("zh") {
        if lower.contains("hans") || lower.ends_with("-cn") || lower.ends_with("-sg") {
            "zh-Hans"
        } else {
            "zh-Hant"
        }
    } else if lower.starts_with("es") {
        "es"
    } else {
        "en"
    }
}

/// The page title in the person's language.
pub fn instructions_title(locale: Option<&str>) -> &'static str {
    match locale_key(locale) {
        "zh-Hant" => "你希望 AI 怎麼跟你做事",
        "zh-Hans" => "你希望 AI 怎么跟你做事",
        "es" => "Cómo quieres que la IA trabaje contigo",
        _ => "How you want AI to work with you",
    }
}

/// Guess the person's language from their own text: mostly Han characters →
/// Chinese (simplified when simplified-only characters outnumber
/// traditional-only ones), Spanish when its marks show up, else English.
pub fn infer_locale<'a>(texts: impl IntoIterator<Item = &'a str>) -> &'static str {
    const SIMPLIFIED: &str = "么这说们让对时为过经发见门问边开关长东车书";
    const TRADITIONAL: &str = "麼這說們讓對時為過經發見門問邊開關長東車書";
    let (mut han, mut latin, mut simplified, mut traditional, mut spanish) = (0, 0, 0, 0, 0);
    for text in texts {
        for c in text.chars() {
            if matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF) {
                han += 1;
                if SIMPLIFIED.contains(c) && !TRADITIONAL.contains(c) {
                    simplified += 1;
                }
                if TRADITIONAL.contains(c) && !SIMPLIFIED.contains(c) {
                    traditional += 1;
                }
            } else if c.is_alphabetic() {
                latin += 1;
                if matches!(c, 'ñ' | '¿' | '¡' | 'á' | 'é' | 'í' | 'ó' | 'ú') {
                    spanish += 1;
                }
            }
        }
    }
    if han > 0 && han * 2 >= latin / 3 {
        if simplified > traditional {
            "zh-Hans"
        } else {
            "zh-Hant"
        }
    } else if spanish >= 2 {
        "es"
    } else {
        "en"
    }
}

struct Copy {
    opening: &'static str,
    identity: &'static str,
    preference: &'static str,
    lesson: &'static str,
    gotcha: &'static str,
}

fn copy(locale: &str) -> Copy {
    match locale {
        "zh-Hant" => Copy {
            opening: "這一頁寫你希望 AI 工具怎麼跟你做事。每個 AI 工具開工前都會先讀它；你改了，下一次就照新的，AI 只能提議修改。",
            identity: "關於你",
            preference: "你的偏好",
            lesson: "學到的事",
            gotcha: "要小心的地方",
        },
        "zh-Hans" => Copy {
            opening: "这一页写你希望 AI 工具怎么跟你做事。每个 AI 工具开工前都会先读它；你改了，下一次就照新的，AI 只能提议修改。",
            identity: "关于你",
            preference: "你的偏好",
            lesson: "学到的事",
            gotcha: "要小心的地方",
        },
        "es" => Copy {
            opening: "Esta página dice cómo quieres que las herramientas de IA trabajen contigo. Cada herramienta la lee antes de empezar; si la cambias, la próxima vez sigue lo nuevo, y la IA solo puede proponer cambios.",
            identity: "Sobre ti",
            preference: "Tus preferencias",
            lesson: "Lo aprendido",
            gotcha: "Cuidado con",
        },
        _ => Copy {
            opening: "This page says how you want AI tools to work with you. Every AI tool reads it before it starts; when you change it, the next session follows the new version, and AI tools can only propose changes.",
            identity: "About you",
            preference: "Your preferences",
            lesson: "What was learned",
            gotcha: "Watch out for",
        },
    }
}

fn line_of(memory: &MemoryItem) -> Option<String> {
    let text = if memory.content.trim().is_empty() {
        memory.title.as_str()
    } else {
        memory.content.as_str()
    };
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    if flat.chars().count() <= SEED_LINE_CHARS {
        return Some(flat);
    }
    let mut cut: String = flat.chars().take(SEED_LINE_CHARS - 1).collect();
    cut.push('…');
    Some(cut)
}

/// The seeded body: the plain opening for people, then one section per
/// memory type that has memories. `memories` is `(memory_type, memory)`.
pub fn seed_body(memories: &[(&str, MemoryItem)], locale: &str) -> String {
    let copy = copy(locale);
    let mut body = String::from(copy.opening);
    for kind in SEED_MEMORY_TYPES {
        let lines: Vec<String> = memories
            .iter()
            .filter(|(t, _)| t == kind)
            .filter_map(|(_, m)| line_of(m))
            .collect();
        if lines.is_empty() {
            continue;
        }
        let heading = match *kind {
            "identity" => copy.identity,
            "preference" => copy.preference,
            "lesson" => copy.lesson,
            _ => copy.gotcha,
        };
        body.push_str(&format!("\n\n## {heading}\n\n"));
        for line in lines {
            body.push_str(&format!("- {line}\n"));
        }
        body.truncate(body.trim_end().len());
    }
    body.push('\n');
    body
}

/// The page with `role = instructions`, when there is one.
pub async fn find_instructions_page(db: &MemoryDB) -> Result<Option<Page>, WenlanError> {
    match db.find_page_id_by_role(INSTRUCTIONS_PAGE_ROLE).await? {
        Some(id) => db.get_page(&id).await,
        None => Ok(None),
    }
}

/// The wire shape handed to AI tools.
pub fn instructions_payload(page: &Page) -> InstructionsPage {
    InstructionsPage {
        page_id: page.id.clone(),
        title: page.title.clone(),
        version: page.version,
        content: page.content.clone(),
        role: INSTRUCTIONS_PAGE_ROLE.to_string(),
        link: page_link(&page.id),
    }
}

/// The instructions page, seeding it first when it never existed. Returns the
/// page (`None` when there is nothing to seed from yet) and whether this call
/// created it. `locale` picks the title; without it, the language of the
/// seed memories does.
pub async fn ensure_instructions_page(
    db: &MemoryDB,
    locale: Option<&str>,
    knowledge_path: Option<&Path>,
) -> Result<(Option<Page>, bool), WenlanError> {
    if let Some(page) = find_instructions_page(db).await? {
        return Ok((Some(page), false));
    }
    // One seeding at a time: two sessions starting together write one page.
    static SEEDING: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _seeding = SEEDING
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    if let Some(page) = find_instructions_page(db).await? {
        return Ok((Some(page), false));
    }
    if db.get_app_metadata(SEEDED_MARKER_KEY).await?.is_some() {
        return Ok((None, false));
    }
    let mut memories: Vec<(&str, MemoryItem)> = Vec::new();
    for kind in SEED_MEMORY_TYPES {
        for memory in db
            .list_memories_scoped(
                &ReadScope::Global,
                Some(kind),
                Some(true),
                None,
                SEED_PER_TYPE,
            )
            .await?
        {
            memories.push((kind, memory));
        }
    }
    if memories.is_empty() {
        return Ok((None, false));
    }
    let locale = match locale.map(str::trim).filter(|l| !l.is_empty()) {
        Some(locale) => locale_key(Some(locale)),
        None => infer_locale(memories.iter().map(|(_, m)| m.content.as_str())),
    };
    let request = wenlan_types::requests::CreateConceptRequest {
        title: instructions_title(Some(locale)).to_string(),
        content: seed_body(&memories, locale),
        summary: None,
        entity_id: None,
        space: wenlan_types::WriteSpaceTarget::Uncategorized,
        source_memory_ids: Vec::new(),
        creation_kind: Some("authored".to_string()),
        workspace: None,
    };
    let result = crate::post_write::create_page(db, request, "wenlan", knowledge_path).await?;
    let page_id = result.attached_to.unwrap_or(result.id);
    db.set_page_role(&page_id, Some(INSTRUCTIONS_PAGE_ROLE))
        .await?;
    db.set_page_review_required(&page_id, true).await?;
    db.set_app_metadata(SEEDED_MARKER_KEY, &chrono::Utc::now().to_rfc3339())
        .await?;
    Ok((db.get_page(&page_id).await?, true))
}

#[cfg(test)]
#[path = "instructions_page_test.rs"]
mod tests;
