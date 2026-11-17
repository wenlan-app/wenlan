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

use wenlan_core::db::revision_cards::PageCardState;
use wenlan_core::db::skill_grow::{SavedRule, SavedRulesPage};
use wenlan_core::post_write::{PageWrite, WriteOutcome};
use wenlan_core::saved_rules::{self as decide, SaveScope, Saver};
use wenlan_types::{
    RemoveRuleRequest, RemoveRuleResponse, SaveRuleRequest, SaveRuleResponse, SavedRuleStatus,
    SkillLineKind,
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

fn refused_with(status: axum::http::StatusCode, detail: impl std::fmt::Display) -> ServerError {
    ServerError::Structured {
        status,
        body: serde_json::json!({ "error": refused(detail) }),
    }
}

pub(super) const TEAM_MEMBERS_CANNOT_SAVE_RULES: &str =
    "a rule saved this way goes to the AI tools of the person at this computer, so a team \
     member cannot save or remove one";
pub(super) const RULES_NEED_THIS_COMPUTER: &str =
    "a rule can be saved or removed only while Wenlan listens on this computer alone";
#[cfg(test)]
pub(super) const RULE_NEEDS_A_SPACE: &str = decide::RULE_NEEDS_A_SPACE;

/// Who a request is recorded as coming from (`wenlan_core::saved_rules`
/// decides; this looks up the name Wenlan lists the tool under).
async fn saver(db: &MemoryDB, headers: &HeaderMap) -> Result<Saver, ServerError> {
    let named = headers
        .get("x-agent-name")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let Some(name) = named else {
        return Ok(decide::saver(None, None));
    };
    let id = decide::tool_id(name);
    let display = db
        .list_agents()
        .await?
        .into_iter()
        .find(|agent| decide::tool_id(&agent.name) == id)
        .and_then(|agent| agent.display_name)
        .or_else(|| wenlan_core::db::known_client_display_name(&id).map(str::to_string));
    Ok(decide::saver(Some(name), display.as_deref()))
}

/// The Space this connection is limited to: the Space header.
fn locked_space(headers: &HeaderMap) -> Result<Option<String>, ServerError> {
    Ok(headers
        .get(crate::space_header::HEADER_NAME)
        .or_else(|| headers.get(crate::space_header::HEADER_NAME_LEGACY))
        .map(|value| {
            value
                .to_str()
                .map(str::trim)
                .map(str::to_string)
                .map_err(|_| ServerError::BadRequest("invalid Space header".into()))
        })
        .transpose()?
        .filter(|value| !value.is_empty()))
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

/// Drop the saves of `entry` that still wait though their card is no longer
/// open (the accept, the dismissal and the replacement of a card settle its
/// save themselves; this is for a card that went another way). Returns the
/// entry as it stands, and whether a save still waits on an open card.
async fn settle_waiting(
    db: &MemoryDB,
    entry: SavedRulesPage,
) -> Result<(SavedRulesPage, bool), ServerError> {
    let mut gone = Vec::new();
    let mut waits = false;
    for save in entry.saves.iter().filter(|save| save.version.is_none()) {
        let open = match save.card.as_deref() {
            Some(card) => db.page_card_state(card).await? == PageCardState::Open,
            None => false,
        };
        if open {
            waits = true;
        } else {
            gone.push(save.id.clone());
        }
    }
    if gone.is_empty() {
        return Ok((entry, waits));
    }
    let (_, entry) = db
        .edit_saved_rules_page(&entry.space_id, |entry| {
            entry
                .saves
                .retain(|save| save.version.is_some() || !gone.contains(&save.id));
        })
        .await?;
    Ok((entry, waits))
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
    // Never general unless asked by name, and never over a connection
    // limited to one Space.
    let locked = locked_space(headers)?;
    let scope = decide::save_scope(locked.as_deref(), req.space.as_deref(), req.every_space)
        .map_err(|reason| ServerError::ValidationError(refused(reason)))?;
    let space = match &scope {
        SaveScope::EverySpace => None,
        SaveScope::Space(name) => Some(resolved_space(&db, headers, Some(name)).await?),
    };
    let saver = saver(&db, headers).await?;

    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    let ledger = db.saved_rules_pages().await?;
    let now = chrono::Utc::now().timestamp();
    if let Some(reason) =
        decide::day_limit_refusal(ledger.iter().flat_map(|page| &page.saves), &saver, now)
    {
        return Err(refused_with(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            reason,
        ));
    }

    let space_id = space
        .as_ref()
        .map(|space| space.id.clone())
        .unwrap_or_default();
    let entry = ledger
        .into_iter()
        .find(|page| page.space_id == space_id)
        .unwrap_or_else(|| SavedRulesPage {
            space_id: space_id.clone(),
            page_id: String::new(),
            saves: Vec::new(),
        });
    let (entry, waits) = settle_waiting(&db, entry).await?;
    if waits {
        // A second card would replace the first, and its rule with it.
        return Err(ServerError::Conflict(refused(
            "a rule already waits as a card on the locked saved-rules page; nothing was saved. \
             Once the person accepts or dismisses that card in the Wenlan app, save this one",
        )));
    }
    let page = match page_of(&db, &entry).await? {
        Some(page) => page,
        None => make_page(&db, page_root, space.as_ref()).await?,
    };
    let content = match skill_grow::page_with_saved_rule(&page.content, kind, &text) {
        Ok(content) => content,
        Err(skill_grow::SavedRuleRefusal::NotARule(reason)) => {
            return Err(ServerError::ValidationError(refused(reason)));
        }
        Err(skill_grow::SavedRuleRefusal::AlreadyOnPage) => {
            let counts = lines_of(&db, page_root, &page, &entry)
                .await?
                .iter()
                .any(|line| {
                    skill_grow::rule_text_key(&line.text) == skill_grow::rule_text_key(&text)
                });
            return Err(ServerError::Conflict(refused(if counts {
                "this is already a rule on the saved-rules page; nothing was saved"
            } else {
                // A line that never counted, or one changed after it was
                // saved: it has to go before the same words can be saved.
                "the saved-rules page already has this sentence as a line that is not a rule \
                 (it was put there or changed by an edit, not by a save); nothing was saved. \
                 Delete that line on the page, then save the rule again"
            })));
        }
        Err(skill_grow::SavedRuleRefusal::PageCannotTakeIt) => {
            return Err(ServerError::Conflict(refused(
                "the saved-rules page cannot take a new rule as it stands (an open code block \
                 at its end?); nothing was saved",
            )));
        }
    };

    // The record comes first, so no reader ever sees the new version
    // without it. It names the version this write will make.
    let record = SavedRule {
        id: uuid::Uuid::new_v4().simple().to_string(),
        text: text.clone(),
        tool: saver.label.clone(),
        tool_id: saver.id.clone(),
        person_asked: person_asked.clone(),
        at: now,
        version: Some(page.version + 1),
        card: None,
    };
    let record_id = record.id.clone();
    db.edit_saved_rules_page(&space_id, |entry| {
        entry.page_id.clone_from(&page.id);
        entry.saves.push(record);
    })
    .await?;
    let landed = match write_rule_change(
        &db,
        page_root,
        &page,
        content.clone(),
        &saver,
        &person_asked,
    )
    .await
    {
        Ok(landed) => landed,
        Err(error) => {
            // The write may have landed and only its answer been lost. The
            // record stays exactly when the page now holds this save as its
            // next version, so no bullet is left on the page that can never
            // count; otherwise the record goes and the page is as it was.
            let now_page = db.get_page(&page.id).await.ok().flatten();
            match now_page {
                Some(written)
                    if written.version == page.version + 1 && written.content == content =>
                {
                    Landed::Wrote(Box::new(written))
                }
                _ => {
                    db.edit_saved_rules_page(&space_id, |entry| {
                        entry.saves.retain(|save| save.id != record_id);
                    })
                    .await?;
                    return Err(error);
                }
            }
        }
    };
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
        Landed::Card(None) => {
            // Held back with no card to accept: nothing can bring it in.
            db.edit_saved_rules_page(&space_id, |entry| {
                entry.saves.retain(|save| save.id != record_id);
            })
            .await?;
            Err(ServerError::Conflict(refused(
                "the saved-rules page did not take the rule and no card was made for it; \
                 nothing was saved",
            )))
        }
        Landed::Card(Some(card)) => {
            // Only the accept of this very card brings the rule in.
            db.edit_saved_rules_page(&space_id, |entry| {
                for save in entry.saves.iter_mut().filter(|save| save.id == record_id) {
                    save.version = None;
                    save.card = Some(card.clone());
                }
            })
            .await?;
            Ok(SaveRuleResponse {
                status: SavedRuleStatus::Waiting,
                rule_id: None,
                space_name,
                page_id: page.id.clone(),
                message: format!(
                    "Not saved yet: the page {:?} is locked, so the rule waits as a card. Once \
                     the person accepts that card in the Wenlan app and unlocks the page, it \
                     counts for {whose}. If they dismiss it, it is dropped.",
                    page.title
                ),
                page_title: page.title,
                saved_from: saver.label,
                revision_card_id: Some(card),
            })
        }
        Landed::Wrote(written) => {
            db.edit_saved_rules_page(&space_id, |entry| {
                for save in entry.saves.iter_mut().filter(|save| save.id == record_id) {
                    save.version = Some(written.version);
                }
            })
            .await?;
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

    let locked = locked_space(headers)?;
    let locked_id = match locked.as_deref() {
        Some(name) => Some(resolved_space(&db, headers, Some(name)).await?.id),
        None => None,
    };

    let _serial = SKILLS_MUTATION_LOCK.lock().await;
    let mut found = None;
    for entry in db.saved_rules_pages().await? {
        let Some(page) = page_of(&db, &entry).await? else {
            continue;
        };
        let line = lines_of(&db, page_root, &page, &entry)
            .await?
            .into_iter()
            .find(|line| line.id == id && line.saved_from.is_some());
        if let Some(line) = line {
            found = Some((entry, page, line));
            break;
        }
    }
    let Some((entry, page, line)) = found else {
        return Err(ServerError::NotFound(refused(
            "no saved rule has that id; nothing was removed. `wenlan rules` and \
             read_space_rules list each saved rule with its id. A rule written on a page by \
             hand is removed by editing that page",
        )));
    };
    // A connection limited to one Space removes only that Space's rules.
    if locked_id
        .as_deref()
        .is_some_and(|locked| locked != entry.space_id)
    {
        return Err(refused_with(
            axum::http::StatusCode::FORBIDDEN,
            "this connection is limited to one Space, and this rule is not that Space's own; \
             nothing was removed",
        ));
    }
    // An AI tool removes only what was saved under its own name.
    let key = skill_grow::rule_text_key(&line.text);
    let record = entry
        .saves
        .iter()
        .rev()
        .find(|save| save.version.is_some() && skill_grow::rule_text_key(&save.text) == key);
    let may = match record {
        Some(record) => decide::removal_refusal(&saver, record),
        // A saved rule always has its record; without one only the
        // terminal may take it out.
        None => (!saver.is_terminal()).then(|| {
            "only the person can remove this rule, with `wenlan rules remove <id>` in a \
             terminal; nothing was removed"
                .to_string()
        }),
    };
    if let Some(reason) = may {
        return Err(refused_with(axum::http::StatusCode::FORBIDDEN, reason));
    }
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
