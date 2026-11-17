// SPDX-License-Identifier: Apache-2.0
//! Rules the person asked to be saved (UX-SKILL-14): "save this as a rule",
//! said to an AI tool or typed as `wenlan rules add`.
//!
//! Such a rule counts at once, like one the person wrote on a page. The
//! daemon cannot prove a person spoke, so what it enforces is what it can:
//! every list says where the save came from and never "written by you"; one
//! short rule per save, with the person's words kept in the page's history;
//! a Space unless every Space is asked for by name; a count per tool per
//! day; removal in one step. The rules live on an ordinary page per scope,
//! "Rules I saved", whose id the skills ledger keeps.

use chrono::TimeZone;
use wenlan_core::db::skill_grow::{SavedRule, SavedRulesPage};
use wenlan_core::post_write::{PageWrite, WriteOutcome};
use wenlan_types::{
    RemoveRuleRequest, RemoveRuleResponse, SaveRuleRequest, SaveRuleResponse, SavedRuleStatus,
    SkillLineKind, SAVED_FROM_TERMINAL, SAVED_RULES_PER_TOOL_PER_DAY,
};

use super::*;

/// The agent recorded as the page's maker, as for a "How I work" page.
const PAGE_MAKER: &str = "wenlan-skills";

/// Every refusal of these two routes opens with this code and goes on with
/// a fixed sentence that holds no page text, so an AI tool's connection can
/// pass it on whole.
const REFUSED: &str = "rule_save_refused";

fn refused(detail: impl std::fmt::Display) -> String {
    format!("{REFUSED}: {detail}")
}

pub(super) const TEAM_MEMBERS_CANNOT_SAVE_RULES: &str =
    "a rule saved this way goes to the AI tools of the person at this computer, so a team \
     member cannot save or remove one";
pub(super) const RULES_NEED_THIS_COMPUTER: &str =
    "a rule can be saved or removed only while Wenlan listens on this computer alone";
pub(super) const RULE_NEEDS_A_SPACE: &str =
    "say which Space this rule is for, or ask for every Space (every_space, \
     `--every-space`); nothing was saved";

/// Where a save came from.
struct Saver {
    /// What the day's count is kept by: the caller's own name for itself.
    id: String,
    /// What rules lists show after "saved from".
    label: String,
}

/// A caller that names an AI tool is that tool. One that names none is a
/// command someone ran here: the terminal.
async fn saver(db: &MemoryDB, headers: &HeaderMap) -> Result<Saver, ServerError> {
    let named = headers
        .get("x-agent-name")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let Some(name) = named else {
        return Ok(Saver {
            id: SAVED_FROM_TERMINAL.to_string(),
            label: SAVED_FROM_TERMINAL.to_string(),
        });
    };
    let display = db
        .list_agents()
        .await?
        .into_iter()
        .find(|agent| agent.name.eq_ignore_ascii_case(name))
        .and_then(|agent| agent.display_name)
        .or_else(|| wenlan_core::db::known_client_display_name(name).map(str::to_string))
        .unwrap_or_else(|| name.to_string());
    // The name goes into files every AI tool reads: letters, digits and a
    // few marks only, and short.
    let mut label: String = display
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '/'))
        .take(40)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if label.is_empty() {
        label = "an AI tool".to_string();
    }
    if label.eq_ignore_ascii_case(SAVED_FROM_TERMINAL) {
        label = format!("{label} (AI tool)");
    }
    Ok(Saver {
        id: name.to_string(),
        label,
    })
}

fn refuse_who_cannot(
    member: &Option<crate::team_actor::ActingMember>,
    local_only: bool,
) -> Result<(), ServerError> {
    if member.is_some() {
        return Err(ServerError::AgentDisabled(refused(
            TEAM_MEMBERS_CANNOT_SAVE_RULES,
        )));
    }
    if !local_only {
        return Err(ServerError::ValidationError(refused(
            RULES_NEED_THIS_COMPUTER,
        )));
    }
    Ok(())
}

fn persons_words(quote: &str) -> Result<String, ServerError> {
    wenlan_core::post_write::validate_person_asked(quote, Some(0)).map_err(|_| {
        ServerError::ValidationError(refused(format!(
            "person_asked must quote the person's own words asking for this: not blank, at \
             most {} characters",
            wenlan_core::post_write::PERSON_ASKED_MAX_CHARS
        )))
    })
}

fn local_only_now() -> bool {
    matches!(
        crate::security::bind_scope_from_env(),
        crate::security::BindScope::Unset | crate::security::BindScope::Loopback
    )
}

fn same_local_day(left: i64, right: i64) -> bool {
    chrono::Local
        .timestamp_opt(left, 0)
        .single()
        .zip(chrono::Local.timestamp_opt(right, 0).single())
        .is_some_and(|(left, right)| left.date_naive() == right.date_naive())
}

fn no_space(page: &wenlan_types::Page) -> bool {
    page.space
        .as_deref()
        .map(str::trim)
        .is_none_or(|name| name.is_empty() || name == "uncategorized")
}

/// The ledger's page for a scope while it is still that scope's: active,
/// and in the Space (or, for every Space, in none).
pub(super) async fn page_of(
    db: &MemoryDB,
    entry: &SavedRulesPage,
) -> Result<Option<wenlan_types::Page>, ServerError> {
    let Some(page) = db.get_page(&entry.page_id).await? else {
        return Ok(None);
    };
    let in_scope = if entry.space_id.is_empty() {
        no_space(&page)
    } else {
        page_belongs_to_space(db, &page, &entry.space_id).await?
    };
    Ok((page.status == "active" && in_scope).then_some(page))
}

/// The rules on a "Rules I saved" page that count, each with where it came
/// from. A bullet the person typed there themselves counts as hand-written.
pub(super) async fn lines_of(
    db: &MemoryDB,
    root: Option<&Path>,
    page: &wenlan_types::Page,
    entry: &SavedRulesPage,
) -> Result<Vec<SkillLine>, ServerError> {
    let mut lines = person_lines_of(db, root, page, false, Some(entry)).await?;
    for line in &mut lines {
        line.general = entry.space_id.is_empty();
    }
    Ok(lines)
}

async fn make_page(
    db: &MemoryDB,
    page_root: Option<&Path>,
    space: Option<&wenlan_types::Space>,
) -> Result<wenlan_types::Page, ServerError> {
    let name = space.map(|space| space.name.as_str());
    let title = skill_grow::saved_rules_title(name);
    let result = wenlan_core::post_write::create_page(
        db,
        CreateConceptRequest {
            title: title.clone(),
            content: skill_grow::saved_rules_page_body(name),
            summary: None,
            entity_id: None,
            space: match name {
                Some(name) => WriteSpaceTarget::Named(name.to_string()),
                None => WriteSpaceTarget::Uncategorized,
            },
            source_memory_ids: Vec::new(),
            creation_kind: Some("authored".into()),
            workspace: name.map(str::to_string),
        },
        PAGE_MAKER,
        page_root,
    )
    .await?;
    if result.attached_to.is_some() || !result.wrote {
        return Err(ServerError::Conflict(refused(
            "another page already has the title Wenlan keeps saved rules under (\"Rules I \
             saved\"); rename that page, then save the rule again",
        )));
    }
    db.get_page(&result.id)
        .await?
        .ok_or_else(|| ServerError::Internal("the saved-rules page disappeared".into()))
}

/// What the write of one rule change came to.
enum Landed {
    Wrote(Box<wenlan_types::Page>),
    /// The page is locked: the change waits as this card.
    Card(Option<String>),
}

async fn write_rule_change(
    db: &MemoryDB,
    page_root: Option<&Path>,
    page: &wenlan_types::Page,
    content: String,
    saver: &Saver,
    person_asked: &str,
) -> Result<Landed, ServerError> {
    let result = wenlan_core::post_write::page_write(
        db,
        PageWrite::RuleSave {
            page_id: &page.id,
            content,
            source_memory_ids: page.source_memory_ids.clone(),
            expected_version: page.version,
            agent: &saver.id,
            person_asked: person_asked.to_string(),
            knowledge_path: page_root,
        },
    )
    .await?;
    if result.gated || result.revision_card_id.is_some() {
        return Ok(Landed::Card(result.revision_card_id));
    }
    if !result.wrote || result.outcome != WriteOutcome::Wrote {
        return Err(ServerError::Conflict(refused(
            "the saved-rules page changed while this was being done; nothing was changed, try \
             again",
        )));
    }
    let written = db
        .get_page(&page.id)
        .await?
        .ok_or_else(|| ServerError::Internal("the saved-rules page disappeared".into()))?;
    Ok(Landed::Wrote(Box::new(written)))
}

pub(super) async fn save_rule_in(
    state: &SharedState,
    headers: &HeaderMap,
    member: Option<crate::team_actor::ActingMember>,
    req: SaveRuleRequest,
    dirs: SkillDirs,
    local_only: bool,
) -> Result<SaveRuleResponse, ServerError> {
    refuse_who_cannot(&member, local_only)?;
    let kind = req.kind.unwrap_or(SkillLineKind::Rule);
    let text = skill_grow::saved_rule_text(kind, &req.text)
        .map_err(|reason| ServerError::ValidationError(refused(reason)))?;
    let person_asked = persons_words(&req.person_asked)?;
    let (db, page_root) = db_and_page_root(state).await?;
    let page_root = page_root.as_deref();
    // Never general unless asked by name: no Space and no flag is refused.
    let space = if req.every_space {
        None
    } else if no_space_named(headers, req.space.as_deref()) {
        return Err(ServerError::ValidationError(refused(RULE_NEEDS_A_SPACE)));
    } else {
        Some(resolved_space(&db, headers, req.space.as_deref()).await?)
    };
    let saver = saver(&db, headers).await?;

    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    let ledger = db.saved_rules_pages().await?;
    let now = chrono::Utc::now().timestamp();
    let today = ledger
        .iter()
        .flat_map(|page| page.saves.iter())
        .filter(|save| save.tool_id == saver.id && same_local_day(save.at, now))
        .count();
    if today >= SAVED_RULES_PER_TOOL_PER_DAY {
        return Err(ServerError::Structured {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            body: serde_json::json!({
                "error": refused(format!(
                    "{SAVED_RULES_PER_TOOL_PER_DAY} rules were already saved from here today, \
                     which is the most for one day; nothing was saved. The person can write \
                     more on a page in a folder named Rules, or save again tomorrow"
                )),
            }),
        });
    }

    let space_id = space
        .as_ref()
        .map(|space| space.id.clone())
        .unwrap_or_default();
    let mut entry = ledger
        .into_iter()
        .find(|page| page.space_id == space_id)
        .unwrap_or_else(|| SavedRulesPage {
            space_id: space_id.clone(),
            page_id: String::new(),
            saves: Vec::new(),
        });
    let page = match page_of(&db, &entry).await? {
        Some(page) => page,
        None => {
            let page = make_page(&db, page_root, space.as_ref()).await?;
            entry.page_id.clone_from(&page.id);
            page
        }
    };
    let content =
        skill_grow::page_with_saved_rule(&page.content, kind, &text).map_err(|refusal| {
            match refusal {
                skill_grow::SavedRuleRefusal::NotARule(reason) => {
                    ServerError::ValidationError(refused(reason))
                }
                skill_grow::SavedRuleRefusal::AlreadyOnPage => ServerError::Conflict(refused(
                    "the saved-rules page already has this sentence; nothing was saved",
                )),
                skill_grow::SavedRuleRefusal::PageCannotTakeIt => ServerError::Conflict(refused(
                    "the saved-rules page cannot take a new rule as it stands (an open code \
                     block at its end?); nothing was saved",
                )),
            }
        })?;

    // The record comes first, so no reader ever sees the new version
    // without it. It names the version this write will make.
    entry.saves.push(SavedRule {
        text: text.clone(),
        tool: saver.label.clone(),
        tool_id: saver.id.clone(),
        person_asked: person_asked.clone(),
        at: now,
        version: Some(page.version + 1),
    });
    db.set_saved_rules_page(&entry).await?;
    let landed = write_rule_change(&db, page_root, &page, content, &saver, &person_asked).await;
    let space_name = space
        .as_ref()
        .map(|space| space.name.clone())
        .unwrap_or_default();
    let whose = if space_name.is_empty() {
        "every Space".to_string()
    } else {
        format!("the Space {space_name:?}")
    };
    let from = wenlan_types::saved_from_label(&saver.label);
    match landed {
        Err(error) => {
            entry.saves.pop();
            db.set_saved_rules_page(&entry).await?;
            Err(error)
        }
        Ok(Landed::Card(card)) => {
            // The card's accept brings the rule in; the record waits for it.
            if let Some(save) = entry.saves.last_mut() {
                save.version = None;
            }
            db.set_saved_rules_page(&entry).await?;
            Ok(SaveRuleResponse {
                status: SavedRuleStatus::Waiting,
                rule_id: None,
                space_name,
                page_id: page.id.clone(),
                message: format!(
                    "Not saved yet: the page {:?} is locked, so the rule waits as a card. Once \
                     the person accepts it in the Wenlan app and unlocks the page, it counts \
                     for {whose}.",
                    page.title
                ),
                page_title: page.title,
                saved_from: saver.label,
                revision_card_id: card,
            })
        }
        Ok(Landed::Wrote(written)) => {
            if let Some(save) = entry.saves.last_mut() {
                save.version = Some(written.version);
            }
            db.set_saved_rules_page(&entry).await?;
            sync_rules_files_locked(&db, page_root, &dirs).await;
            let rule_id = skill_grow::line_id(kind, &written.id, 0, &text);
            let silenced = skill_grow::page_is_not_for_ai_tools(&written.content);
            let message = if silenced {
                format!(
                    "Saved on the page {:?}, but that page is marked #not-for-ai-tools, so AI \
                     tools are not told its rules until the mark is taken off.",
                    written.title
                )
            } else {
                format!(
                    "Saved as a rule for {whose}. It counts now and AI tools are told it. It \
                     is kept on the page {:?} and listed as {from}. To remove it: `wenlan rules \
                     remove {rule_id}`.",
                    written.title
                )
            };
            Ok(SaveRuleResponse {
                status: SavedRuleStatus::Saved,
                rule_id: Some(rule_id),
                space_name,
                page_id: written.id,
                page_title: written.title,
                saved_from: saver.label,
                revision_card_id: None,
                message,
            })
        }
    }
}

pub(super) async fn remove_rule_in(
    state: &SharedState,
    headers: &HeaderMap,
    member: Option<crate::team_actor::ActingMember>,
    id: &str,
    req: RemoveRuleRequest,
    dirs: SkillDirs,
    local_only: bool,
) -> Result<RemoveRuleResponse, ServerError> {
    refuse_who_cannot(&member, local_only)?;
    let person_asked = persons_words(&req.person_asked)?;
    let (db, page_root) = db_and_page_root(state).await?;
    let page_root = page_root.as_deref();
    let saver = saver(&db, headers).await?;

    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    let mut found = None;
    for entry in db.saved_rules_pages().await? {
        let Some(page) = page_of(&db, &entry).await? else {
            continue;
        };
        let saved = lines_of(&db, page_root, &page, &entry)
            .await?
            .into_iter()
            .any(|line| line.id == id && line.saved_from.is_some());
        if saved {
            found = Some(page);
            break;
        }
    }
    let Some(page) = found else {
        return Err(ServerError::NotFound(refused(
            "no saved rule has that id; nothing was removed. `wenlan rules` and \
             read_space_rules list each saved rule with its id. A rule written on a page by \
             hand is removed by editing that page",
        )));
    };
    let (content, _) = skill_grow::page_without_rule(
        &SkillPageInput {
            id: page.id.clone(),
            title: page.title.clone(),
            revision: page.version,
            revision_at: 0,
            text: page.content.clone(),
        },
        id,
    )
    .ok_or_else(|| {
        ServerError::Conflict(refused(
            "the rule could not be taken off its page on its own; edit the page instead",
        ))
    })?;
    match write_rule_change(&db, page_root, &page, content, &saver, &person_asked).await? {
        Landed::Card(card) => Ok(RemoveRuleResponse {
            status: SavedRuleStatus::Waiting,
            id: id.to_string(),
            page_id: page.id.clone(),
            message: format!(
                "Not removed yet: the page {:?} is locked, so the removal waits as a card for \
                 the person to accept in the Wenlan app.",
                page.title
            ),
            page_title: page.title,
            revision_card_id: card,
        }),
        Landed::Wrote(written) => {
            sync_rules_files_locked(&db, page_root, &dirs).await;
            Ok(RemoveRuleResponse {
                status: SavedRuleStatus::Removed,
                id: id.to_string(),
                page_id: written.id,
                message: format!(
                    "Removed. AI tools are no longer told this rule; it was taken off the page \
                     {:?}, whose history keeps it.",
                    written.title
                ),
                page_title: written.title,
                revision_card_id: None,
            })
        }
    }
}

/// `POST /api/skills/rules`.
pub async fn handle_save_rule(
    State(state): State<SharedState>,
    headers: HeaderMap,
    crate::team_actor::TeamActor(member): crate::team_actor::TeamActor,
    Json(req): Json<SaveRuleRequest>,
) -> Result<Json<SaveRuleResponse>, ServerError> {
    save_rule_in(
        &state,
        &headers,
        member,
        req,
        SkillDirs::system(),
        local_only_now(),
    )
    .await
    .map(Json)
}

/// `POST /api/skills/rules/{id}/remove`.
pub async fn handle_remove_rule(
    State(state): State<SharedState>,
    headers: HeaderMap,
    crate::team_actor::TeamActor(member): crate::team_actor::TeamActor,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<RemoveRuleRequest>,
) -> Result<Json<RemoveRuleResponse>, ServerError> {
    remove_rule_in(
        &state,
        &headers,
        member,
        &id,
        req,
        SkillDirs::system(),
        local_only_now(),
    )
    .await
    .map(Json)
}
