// SPDX-License-Identifier: Apache-2.0
//! Reviewing together: an AI tool's change to a locked page of a shared
//! scope waits as a card every member sees, and whoever accepts or rejects
//! it first decides it for everyone.
//!
//! A card staged here by an AI tool is written as a proposal file
//! (`members/<author>/proposals/<page>/<sha>.wlv`, a whole page like a
//! version, with `proposal: true`). Another member's open proposal is staged
//! here as a card naming that member's AI tool. When a card is accepted or
//! dismissed here, a decision file (`members/<me>/decisions/<page>/<sha>.wlv`,
//! `decides: <proposal sha>`) closes the proposal; a decision found in the
//! folder takes the card away here. An accepted change reaches the others as
//! an ordinary version, written by the page step of the next pass.
//!
//! One page shows one card at a time: another member's proposal is staged
//! only while the page has no open card, the newest first.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::version::{self, Author, AuthorKind, Decides, PageVersion, VersionMeta};
use super::{now_ts, writer_edited_by, Pass};
use crate::db::revision_cards::{OpenPageCard, PageCardState};
use crate::error::WenlanError;
use crate::pages::Page;
use crate::post_write::Writer;
use crate::store::format::page::{PageFile, PageKind};

/// What this Wenlan shared and staged, by scope.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Review {
    /// Cards an AI tool made here, by card id, and the proposal each was
    /// written as.
    #[serde(default)]
    shared: BTreeMap<String, Card>,
    /// Other members' proposals staged here, by proposal sha.
    #[serde(default)]
    staged: BTreeMap<String, Card>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Card {
    page: String,
    /// The proposal's sha (`shared`) or the card's id (`staged`; `None` while
    /// the proposal waits for the page's open card to be decided).
    #[serde(default)]
    id: Option<String>,
    /// The body digest of the proposed page.
    digest: String,
    /// The page's version the card was staged over.
    #[serde(default)]
    at: Option<i64>,
}

fn review_key(key: &str) -> String {
    format!("shared_space.review:{key}")
}

impl Pass<'_> {
    /// Shares the AI tools' cards, stages the others' proposals, and writes
    /// a decision for each card decided here. Runs after the page steps, so
    /// an accepted card's page is already written as a version.
    pub(super) async fn review(
        &self,
        proposals: &HashMap<String, Vec<PageVersion>>,
        decisions: &HashMap<String, Vec<PageVersion>>,
        heads: &HashMap<String, String>,
    ) -> Result<(), WenlanError> {
        let key = review_key(&self.scope.key());
        let mut review: Review = match self.db.get_app_metadata(&key).await? {
            Some(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            None => Review::default(),
        };
        let before = review.clone();
        let decided: HashSet<&str> = decisions
            .values()
            .flatten()
            .filter_map(|decision| decision.meta.decides.as_ref())
            .map(|decides| decides.proposal.as_str())
            .collect();

        // Proposals of others staged here.
        let staged: Vec<(String, Card)> = review.staged.clone().into_iter().collect();
        for (sha, card) in staged {
            if decided.contains(sha.as_str()) {
                if let Some(id) = &card.id {
                    self.close_card(id).await?;
                }
                review.staged.remove(&sha);
                continue;
            }
            let Some(id) = &card.id else { continue };
            match self.db.page_card_state(id).await? {
                PageCardState::Open => {}
                // A newer proposal made here took its place: staged again
                // once that one is decided.
                PageCardState::Replaced => {
                    if let Some(kept) = review.staged.get_mut(&sha) {
                        kept.id = None;
                    }
                }
                PageCardState::Closed => {
                    let accepted = self.accepted(&card).await?;
                    self.decide(&card.page, &sha, accepted).await?;
                    review.staged.remove(&sha);
                }
            }
        }

        // Cards made here and shared.
        let shared: Vec<(String, Card)> = review.shared.clone().into_iter().collect();
        for (card_id, card) in shared {
            let Some(sha) = card.id.as_deref() else {
                review.shared.remove(&card_id);
                continue;
            };
            if decided.contains(sha) {
                self.close_card(&card_id).await?;
                review.shared.remove(&card_id);
                continue;
            }
            match self.db.page_card_state(&card_id).await? {
                PageCardState::Open => {}
                // The AI tool proposed again; its newer card is shared
                // below and this proposal is closed.
                PageCardState::Replaced => {
                    self.decide(&card.page, sha, false).await?;
                    review.shared.remove(&card_id);
                }
                PageCardState::Closed => {
                    let accepted = self.accepted(&card).await?;
                    self.decide(&card.page, sha, accepted).await?;
                    review.shared.remove(&card_id);
                }
            }
        }

        let open = self.db.open_page_cards().await?;
        let staged_ids: HashSet<String> = review
            .staged
            .values()
            .filter_map(|card| card.id.clone())
            .collect();
        let mut pages_with_card: HashSet<String> = HashSet::new();
        for card in &open {
            if self.space_pages.contains_key(&card.page_id) {
                pages_with_card.insert(card.page_id.clone());
            }
        }

        // An AI tool's new card here becomes a proposal.
        if !self.settings.read_only {
            for card in &open {
                if review.shared.contains_key(&card.card_id)
                    || staged_ids.contains(&card.card_id)
                    || !self.space_pages.contains_key(&card.page_id)
                    || !shareable(card)
                {
                    continue;
                }
                let Some(page) = self.db.get_page(&card.page_id).await? else {
                    continue;
                };
                if page.status != "active" || !page.review_required {
                    continue;
                }
                let Some(body) = self.db.staged_card_body(&card.card_id).await? else {
                    continue;
                };
                let author = self.card_author(card);
                if let Some(sha) = self
                    .propose(&page, &body, heads.get(&page.id), author)
                    .await?
                {
                    review.shared.insert(
                        card.card_id.clone(),
                        Card {
                            page: page.id.clone(),
                            id: Some(sha),
                            digest: version::body_digest(&body),
                            at: card.page_version,
                        },
                    );
                }
            }
        }

        // The newest open proposal of each page, from anyone but this
        // Wenlan, waits as a card here. A member who may only read decides
        // nothing, so is shown none.
        if !self.settings.read_only {
            let mine: HashSet<&str> = review
                .shared
                .values()
                .filter_map(|card| card.id.as_deref())
                .collect();
            let mut changes: Vec<(String, Card)> = Vec::new();
            for (page_id, list) in proposals {
                if !self.space_pages.contains_key(page_id) {
                    continue;
                }
                let newest = list
                    .iter()
                    .filter(|proposal| {
                        !decided.contains(proposal.sha.as_str())
                            && !mine.contains(proposal.sha.as_str())
                    })
                    .max_by(|a, b| (&a.meta.ts, &a.sha).cmp(&(&b.meta.ts, &b.sha)));
                let Some(proposal) = newest else { continue };
                let Some(page) = self.db.get_page(page_id).await? else {
                    continue;
                };
                if page.status != "active" || !page.review_required {
                    continue;
                }
                let shown = review.staged.get(&proposal.sha).cloned();
                let card_id = match &shown {
                    // Still the card it was staged as, over the page as it
                    // is: nothing to do.
                    Some(Card {
                        id: Some(id),
                        at: Some(at),
                        ..
                    }) if *at == page.version => continue,
                    // The page moved since: staged again over it, so the
                    // card can still be accepted (it replaces the page with
                    // the proposal, as the person chooses).
                    Some(Card { id: Some(id), .. }) => Some(id.clone()),
                    _ if pages_with_card.contains(page_id) => continue,
                    _ => None,
                };
                if let Some(id) = &card_id {
                    // A card the person reworded here is left as they made it.
                    let body = self.db.staged_card_body(id).await?;
                    if body
                        .is_none_or(|body| version::body_digest(&body) != proposal.meta.body_digest)
                    {
                        continue;
                    }
                }
                if let Some(id) = self.stage(&page, proposal).await? {
                    changes.push((
                        proposal.sha.clone(),
                        Card {
                            page: page.id.clone(),
                            id: Some(id),
                            digest: proposal.meta.body_digest.clone(),
                            at: Some(page.version),
                        },
                    ));
                }
            }
            review.staged.extend(changes);
        }

        if review != before {
            self.db
                .set_app_metadata(&key, &serde_json::to_string(&review)?)
                .await?;
        }
        Ok(())
    }

    /// The card's change is on the page: the page has the proposed body, or
    /// was last changed by accepting a card after it was staged (accepted
    /// with the person's own edits).
    async fn accepted(&self, card: &Card) -> Result<bool, WenlanError> {
        let Some(page) = self.db.get_page(&card.page).await? else {
            return Ok(false);
        };
        if version::body_digest(&page.content) == card.digest {
            return Ok(true);
        }
        let since = card.at.unwrap_or(i64::MAX);
        Ok(self
            .db
            .list_page_history(&card.page, 20)
            .await?
            .iter()
            .any(|row| row.version > since && row.edited_by == "revision_accept"))
    }

    /// Takes a card away here because it was decided elsewhere. A card the
    /// person already decided here is gone; nothing else is done.
    async fn close_card(&self, card_id: &str) -> Result<(), WenlanError> {
        if self.db.page_card_state(card_id).await? == PageCardState::Open {
            self.db.delete_by_source_id("memory", card_id).await?;
        }
        Ok(())
    }

    /// Who made a card, as its proposal names them: on a team server, the
    /// member whose AI tool wrote through the team listener; else this
    /// person's AI tool.
    fn card_author(&self, card: &OpenPageCard) -> Author {
        let proposer = card.proposer.as_deref();
        if self.settings.hosted {
            if let Some((tool, member)) = proposer.and_then(wenlan_types::team::split_team_agent) {
                if self.names.contains_key(member) {
                    return Author {
                        member: member.to_string(),
                        kind: AuthorKind::Ai,
                        agent: Some(tool.to_string()),
                    };
                }
            }
        }
        Author {
            member: self.settings.member_id.clone(),
            kind: AuthorKind::Ai,
            agent: proposer.map(|agent| {
                wenlan_types::team::split_team_agent(agent)
                    .map(|(tool, _)| tool.to_string())
                    .unwrap_or_else(|| agent.to_string())
            }),
        }
    }

    /// Writes `body` as a proposal for `page`, made over `head`.
    async fn propose(
        &self,
        page: &Page,
        body: &str,
        head: Option<&String>,
        author: Author,
    ) -> Result<Option<String>, WenlanError> {
        if !version::is_safe_id(&page.id) {
            return Ok(None);
        }
        let mut file = PageFile::new(&page.title, body);
        file.id = Some(page.id.clone());
        file.kind = PageKind::Page;
        let meta = VersionMeta {
            parent: head.cloned(),
            merged: Vec::new(),
            body_digest: version::body_digest(body),
            author,
            ts: now_ts(),
            archived: false,
            locked: true,
            proposal: true,
            decides: None,
        };
        let text = version::render(&file, &meta);
        let name = version::version_file_name(&version::sha256_hex(text.as_bytes()));
        match version::parse(&name, text.as_bytes()) {
            Ok(read) if read.page.body == body => {}
            _ => {
                log::warn!(
                    "[shared_space] a proposal for {} does not read back as written; not shared",
                    page.id
                );
                return Ok(None);
            }
        }
        let member = meta.author.member.clone();
        self.write_kind(&member, version::PROPOSALS_DIR, &page.id, text)
            .await
            .map(Some)
    }

    /// Stages `proposal` as a card on `page`, naming the member whose AI
    /// tool made it. Returns the card's id.
    async fn stage(
        &self,
        page: &Page,
        proposal: &PageVersion,
    ) -> Result<Option<String>, WenlanError> {
        let author = &proposal.meta.author;
        let edited_by = writer_edited_by(author, &self.settings.member_id);
        let source_revision = self.db.get_page_source_revision(&page.id).await?;
        let staged = crate::post_write::stage_page_revision_card_proposed_by(
            self.db,
            page,
            &proposal.page.body,
            &page.source_memory_ids,
            source_revision,
            &edited_by,
            author.agent.as_deref(),
            None,
            &[],
            None,
        )
        .await;
        match staged {
            Ok(result) => Ok(result.revision_card_id),
            Err(error) => {
                log::warn!(
                    "[shared_space] proposal {} not shown on {}: {error}",
                    proposal.sha,
                    page.id
                );
                Ok(None)
            }
        }
    }

    /// Writes this person's decision on proposal `sha`.
    async fn decide(&self, page_id: &str, sha: &str, accepted: bool) -> Result<(), WenlanError> {
        if self.settings.read_only || !version::is_safe_id(page_id) {
            return Ok(());
        }
        let mut file = PageFile::new("", "");
        file.id = Some(page_id.to_string());
        file.kind = PageKind::Page;
        let meta = VersionMeta {
            parent: None,
            merged: Vec::new(),
            body_digest: version::body_digest(""),
            author: Author {
                member: self.settings.member_id.clone(),
                kind: AuthorKind::Person,
                agent: None,
            },
            ts: now_ts(),
            archived: false,
            locked: false,
            proposal: false,
            decides: Some(Decides {
                proposal: sha.to_string(),
                accepted,
            }),
        };
        let text = version::render(&file, &meta);
        let member = self.settings.member_id.clone();
        self.write_kind(&member, version::DECISIONS_DIR, page_id, text)
            .await?;
        Ok(())
    }
}

/// A card made here by an AI tool, not one staged from another member's
/// proposal or another of this person's computers, nor one that stays here.
fn shareable(card: &OpenPageCard) -> bool {
    !card.local_only && matches!(Writer::classify(&card.edited_by), Writer::Agent(_))
}
