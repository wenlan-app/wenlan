// SPDX-License-Identifier: Apache-2.0
//! **Steep on demand**: the caller's own AI tool rewrites the notes that are
//! waiting for steep (`stale_reason` set), through the same write the
//! automatic background steep uses.
//!
//! Steep is one job with two modes. Automatic runs the daemon's own model in
//! the background (`refinery::run_redistill_page_slice`, the maintenance
//! stale-page stage). On demand, the user's AI tool does the model work and
//! the daemon does everything else: it lists the work, hands out a snapshot,
//! checks the result, and commits it. Both modes end in the same stale
//! rebuild with `re_distill` provenance (`post_write::steep_update_page` adds
//! the agent's name and its Space lock), so the ownership gate (a page the
//! user wrote becomes a revision card), the citation gate and the staleness
//! fence are the same code.
//!
//! The protocol:
//!
//! 1. [`list_work`]: the notes waiting, with what each one needs.
//! 2. [`claim`]: before any model work, the caller gets the note's surviving
//!    sources, numbered, and a token. The token stands for the exact page row
//!    (incarnation, version, source revision) and the exact source text it was
//!    handed, and it holds the note so no other run spends a model on it.
//! 3. [`commit`]: the rewrite is checked against that snapshot. A source
//!    added, edited or removed since the claim, a human edit, or the page
//!    deleted and recreated under the same id refuses the commit with nothing
//!    written. The body, its citations and the cleared staleness land in one
//!    transaction. Committing the same token twice returns the first result.
//!
//! Code a note relies on (its code basis, [`crate::code_basis`]) is steep's
//! work too. A changed code basis no model has judged yet is a `code_check`:
//! the claim hands the caller the same question the background recheck asks
//! its own model ([`crate::code_basis_recheck`]), for the caller's small model,
//! and a `judge` commit lands the answers. A code basis judged `stale` makes
//! the note `code_changed`: the claim hands over the code as it reads now, the
//! rewrite goes through the same write as above, and a rewrite that lands
//! settles those code bases.
//! In Automatic mode the daemon's own model takes `code_changed` notes through
//! this same claim and commit ([`rewrite_one_code_page`]).
//!
//! Holds live in memory ([`SteepLeases`]). Both runs live in this daemon
//! process, so a restart ends every hold together with the work behind it; a
//! caller whose token is gone claims again.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wenlan_types::requests::UpdatePageRequest;
use wenlan_types::steep::{
    SteepCode, SteepCodeAnswer, SteepCodeCheck, SteepSource, SteepWorkItem, STEEP_ACTION_JUDGE,
    STEEP_ACTION_RELEASE, STEEP_ACTION_REWRITE, STEEP_ACTION_STILL_ACCURATE, STEEP_PLAN_JUDGE,
    STEEP_PLAN_NEEDS_YOU, STEEP_PLAN_REVIEW_CARD, STEEP_PLAN_REWRITE, STEEP_REASON_CODE_CHANGED,
    STEEP_REASON_CODE_CHECK,
};
use wenlan_types::CodeBasisLocator;

use crate::db::{MemoryDB, PageFence};
use crate::error::WenlanError;
use crate::post_write::{WriteOutcome, WriteResult};
use crate::read_scope::ReadScope;

/// `operation_receipts.caller_id` of every steep commit; the token is the
/// operation id.
pub const STEEP_RECEIPT_CALLER: &str = "wenlan-steep";

/// How long an on-demand claim holds a note. Long enough for a main model to
/// rewrite one note; a caller that needs longer claims again.
pub const ON_DEMAND_HOLD: Duration = Duration::from_secs(15 * 60);

/// How long the automatic run holds a note. It releases as soon as its one
/// refresh returns; the expiry only matters if that task dies.
pub const AUTOMATIC_HOLD: Duration = Duration::from_secs(10 * 60);

/// `refresh_blocked_reason` for a note whose sources are all gone. No model
/// can rebuild it from nothing, so it waits for the user to keep, rewrite or
/// archive it.
pub const NEEDS_YOU_NO_SOURCES: &str = "no_sources_left";

/// Most sources one on-demand rewrite takes, the same cap the automatic run
/// uses. A bigger note is rebuilt with "Update from sources" in the app.
pub const STEEP_SOURCE_CAP: usize = crate::synthesis::distill::AUTOMATIC_PAGE_REFRESH_SOURCE_CAP;

/// Who holds a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteepHolder {
    Automatic,
    OnDemand,
}

impl SteepHolder {
    pub fn as_str(self) -> &'static str {
        match self {
            SteepHolder::Automatic => "automatic",
            SteepHolder::OnDemand => "on_demand",
        }
    }
}

/// What an on-demand claim saw, so its commit can prove nothing moved.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    fence: PageFence,
    version: i64,
    stale_reason: String,
    /// `(memory id, text)` in citation order: `[n]` is `sources[n - 1]`.
    sources: Vec<(String, String)>,
    /// The Space the claimer was locked to. The commit lands only while the
    /// page is still in it: moving a page to another Space bumps none of the
    /// fields above.
    scope: ReadScope,
    /// The note's own `stale_reason` at the claim; `None` for code work.
    page_stale_reason: Option<String>,
    /// For `code_changed` and `code_check`: the code bases the claim is
    /// about, in the order the claim numbered them.
    code: Vec<CodeSnap>,
}

/// One code basis as a code claim saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeSnap {
    locator: CodeBasisLocator,
    /// The row's stored anchor: the row has not been rewritten or settled.
    anchor_json: String,
    /// `code_check`: the key the question was asked under. `code_changed`:
    /// the key the `stale` verdict was recorded under.
    key: String,
    /// The digest of the changed lines as the caller was shown them.
    code_digest: String,
}

fn is_code_work(reason: &str) -> bool {
    reason == STEEP_REASON_CODE_CHANGED || reason == STEEP_REASON_CODE_CHECK
}

/// Whether steep's rewrite of `page` goes to the user as a revision card.
/// For work from sources that is every human-owned page. For code work it is
/// a page a person edited, or one locked for review: a note an agent wrote
/// from code has no sources and so lands as `authored`, but it is Wenlan's
/// own and is kept up to date like one.
fn owned(page: &crate::pages::Page, code_work: bool) -> bool {
    page.review_required
        || if code_work {
            page.user_edited
        } else {
            crate::post_write::page_is_human_owned(page)
        }
}

#[derive(Debug, Clone)]
struct Lease {
    page_id: String,
    token: String,
    holder: SteepHolder,
    expires_at: Instant,
    snapshot: Option<Snapshot>,
}

/// The holds steep runs have on notes. One hold per note; a hold past its
/// expiry can be taken over, and taking it over invalidates the old token.
#[derive(Debug, Default)]
pub struct SteepLeases {
    by_page: Mutex<HashMap<String, Lease>>,
    /// Automatic code rewrites that did not land, by what they were made
    /// from (see `automatic`). The same note and code would fail the same
    /// way, so the automatic run waits for either to change.
    failed: Mutex<HashSet<String>>,
}

impl SteepLeases {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Lease>> {
        // A panic while holding this lock leaves only plain data behind.
        self.by_page.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Take the note, or say who holds it.
    fn take(
        &self,
        page_id: &str,
        holder: SteepHolder,
        hold: Duration,
        snapshot: Option<Snapshot>,
    ) -> Result<String, SteepHolder> {
        let now = Instant::now();
        let mut leases = self.lock();
        if let Some(current) = leases.get(page_id) {
            if current.expires_at > now {
                return Err(current.holder);
            }
        }
        let token = format!("stp_{}", uuid::Uuid::new_v4().simple());
        leases.insert(
            page_id.to_string(),
            Lease {
                page_id: page_id.to_string(),
                token: token.clone(),
                holder,
                expires_at: now + hold,
                snapshot,
            },
        );
        Ok(token)
    }

    /// Who holds the note now, if anyone.
    pub fn holder_of(&self, page_id: &str) -> Option<SteepHolder> {
        let now = Instant::now();
        self.lock()
            .get(page_id)
            .filter(|lease| lease.expires_at > now)
            .map(|lease| lease.holder)
    }

    /// The lease this token names, while it is still the note's lease. An
    /// expired lease nobody took over still answers: its snapshot fence
    /// decides whether the commit lands.
    fn find(&self, token: &str) -> Option<Lease> {
        self.lock()
            .values()
            .find(|lease| lease.token == token)
            .cloned()
    }

    fn failed(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.failed.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Give the note back. Only the token's own lease is removed, never one
    /// that took over after it.
    fn release(&self, page_id: &str, token: &str) {
        let mut leases = self.lock();
        if leases
            .get(page_id)
            .is_some_and(|lease| lease.token == token)
        {
            leases.remove(page_id);
        }
    }
}

/// The automatic run's hold on one note, released when dropped.
pub struct AutomaticHold<'a> {
    leases: &'a SteepLeases,
    page_id: String,
    token: String,
}

impl Drop for AutomaticHold<'_> {
    fn drop(&mut self) {
        self.leases.release(&self.page_id, &self.token);
    }
}

/// Hold a note for the automatic run. `None` when an on-demand caller holds
/// it: the automatic run leaves it alone instead of paying for the same
/// rewrite twice.
pub fn hold_for_automatic<'a>(db: &'a MemoryDB, page_id: &str) -> Option<AutomaticHold<'a>> {
    let leases = db.steep_leases();
    let token = leases
        .take(page_id, SteepHolder::Automatic, AUTOMATIC_HOLD, None)
        .ok()?;
    Some(AutomaticHold {
        leases,
        page_id: page_id.to_string(),
        token,
    })
}

/// Why a steep call was refused.
#[derive(Debug, thiserror::Error)]
pub enum SteepError {
    #[error("page not found")]
    NotFound,
    /// The note is not waiting for steep (any more).
    #[error("{0}")]
    NotWaiting(String),
    /// Another run holds the note.
    #[error("{}", held_message(*.0))]
    Held(SteepHolder),
    /// The note or its sources moved since the claim. Nothing was written.
    #[error("{0}")]
    Changed(String),
    /// The token names no hold: Wenlan restarted, or another run took the
    /// note over after the hold expired.
    #[error(
        "this claim is gone (Wenlan restarted, or another run took the page); claim the page again"
    )]
    TokenGone,
    #[error("{0}")]
    Invalid(String),
    /// The rewrite did not cite its sources well enough. The hold is kept so
    /// the caller can fix the text and commit again.
    #[error("{0}")]
    CitationGate(String),
    #[error(transparent)]
    Db(#[from] WenlanError),
}

impl SteepError {
    /// The machine code the daemon puts before the message, so an MCP client
    /// can pass the reason on to the agent (`None` for errors that are not
    /// steep's own refusals).
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            SteepError::NotFound | SteepError::Db(_) => return None,
            SteepError::NotWaiting(_) => "steep_not_waiting",
            SteepError::Held(_) => "steep_held",
            SteepError::Changed(_) => "steep_changed",
            SteepError::TokenGone => "steep_token_gone",
            SteepError::Invalid(_) => "steep_invalid",
            SteepError::CitationGate(_) => "steep_citations",
        })
    }
}

fn held_message(holder: SteepHolder) -> String {
    match holder {
        SteepHolder::Automatic => {
            "Wenlan is steeping this page in the background right now; pick another page".into()
        }
        SteepHolder::OnDemand => {
            "another AI session is rewriting this page right now; pick another page".into()
        }
    }
}

/// A claimed note, ready for the caller's model.
#[derive(Debug, Clone)]
pub struct SteepClaim {
    pub page_id: String,
    pub title: String,
    pub plan: &'static str,
    pub token: Option<String>,
    /// Unix seconds.
    pub held_until: Option<i64>,
    pub version: i64,
    pub stale_reason: String,
    pub current_content: String,
    pub sources: Vec<SteepSource>,
    pub code: Vec<SteepCode>,
    pub judge_instructions: Option<String>,
    pub code_checks: Vec<SteepCodeCheck>,
    pub note: String,
}

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteepCommit {
    pub page_id: String,
    /// `written`, `review_card`, `still_accurate`, `unchanged`, `released`
    /// or `judged`.
    pub outcome: &'static str,
    pub revision_card_id: Option<String>,
    pub citations: Option<String>,
    /// For `judged`: what the answers settled.
    pub judged: Option<String>,
    pub replayed: bool,
}

/// One page of the work list, the full count in scope, and the cursor to the
/// next page.
pub struct SteepWorklist {
    pub items: Vec<SteepWorkItem>,
    pub total: u32,
    pub next_cursor: Option<String>,
}

/// The current source ids of a note: the citation edges first, the legacy
/// JSON list for a note that has none.
async fn current_source_ids(
    db: &MemoryDB,
    page: &crate::pages::Page,
) -> Result<Vec<String>, WenlanError> {
    let joined = db.get_page_sources(&page.id).await?;
    if joined.is_empty() {
        return Ok(page.source_memory_ids.clone());
    }
    Ok(joined.into_iter().map(|s| s.memory_source_id).collect())
}

/// `(memory id, text)` for each source that still exists, in `ids` order,
/// with text capped the way the automatic run caps it.
async fn surviving_sources(
    db: &MemoryDB,
    ids: &[String],
) -> Result<Vec<(String, String)>, WenlanError> {
    let found: HashMap<String, String> = db
        .get_memory_contents_by_ids(ids)
        .await?
        .into_iter()
        .collect();
    let cap = crate::synthesis::distill::CANDIDATE_MEM_SNIPPET_CAP;
    let mut seen = std::collections::HashSet::new();
    Ok(ids
        .iter()
        .filter(|id| seen.insert(id.as_str()))
        .filter_map(|id| {
            found
                .get(id)
                .map(|text| (id.clone(), text.chars().take(cap).collect()))
        })
        .collect())
}

/// Ids of every note waiting for steep, in any scope. A route filters them
/// down to what its caller may see before calling [`list_work`].
pub async fn waiting_page_ids(db: &MemoryDB) -> Result<Vec<String>, WenlanError> {
    let mut ids: Vec<String> = db
        .list_steep_pages()
        .await?
        .into_iter()
        .map(|page| page.id)
        .collect();
    ids.extend(
        db.list_steep_code_pages()
            .await?
            .into_iter()
            .map(|(page, _, _)| page.id),
    );
    Ok(ids)
}

/// Every note steep can act on, with its reason: the notes waiting for steep
/// first by their own staleness, then notes whose code changed. A `code_changed` note
/// with a revision card already waiting for the user is left out, and
/// `code_check` notes only count when `judge_code` (no background model judges
/// changed code).
async fn steep_work(
    db: &MemoryDB,
    judge_code: bool,
) -> Result<Vec<(crate::pages::Page, String)>, WenlanError> {
    let mut work: Vec<(crate::pages::Page, String)> = db
        .list_steep_pages()
        .await?
        .into_iter()
        .map(|page| {
            let reason = page.stale_reason.clone().unwrap_or_default();
            (page, reason)
        })
        .collect();
    for (page, stale, unjudged) in db.list_steep_code_pages().await? {
        if stale > 0 {
            if db.get_pending_revision_for(&page.id).await?.is_none() {
                work.push((page, STEEP_REASON_CODE_CHANGED.to_string()));
            }
        } else if judge_code && unjudged > 0 {
            work.push((page, STEEP_REASON_CODE_CHECK.to_string()));
        }
    }
    work.sort_by(|(a, _), (b, _)| {
        b.last_modified
            .cmp(&a.last_modified)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(work)
}

fn cursor_for(page: &crate::pages::Page) -> String {
    format!("{}|{}", page.last_modified, page.id)
}

/// The notes waiting for steep in `scope`, `limit` at a time after `cursor`.
/// `visible` says which notes the caller may see at all; the rest are left
/// out of both the items and the total.
///
/// `judge_code`: no background model judges changed code, so notes whose code
/// changed unjudged are listed for the caller's small model (`code_check`).
pub async fn list_work(
    db: &MemoryDB,
    scope: &ReadScope,
    visible: impl Fn(&str) -> bool,
    limit: usize,
    cursor: Option<&str>,
    judge_code: bool,
) -> Result<SteepWorklist, WenlanError> {
    let work: Vec<_> = steep_work(db, judge_code)
        .await?
        .into_iter()
        .filter(|(page, _)| scope.matches(page.space.as_deref()) && visible(&page.id))
        .collect();
    let pages: Vec<&crate::pages::Page> = work.iter().map(|(page, _)| page).collect();
    let total = u32::try_from(pages.len()).unwrap_or(u32::MAX);
    // Keyset: newest change first, ties by id, the same order the query uses.
    let start = match cursor.and_then(|c| c.split_once('|')) {
        Some((last_modified, id)) => pages
            .iter()
            .position(|page| {
                page.last_modified.as_str() < last_modified
                    || (page.last_modified == last_modified && page.id.as_str() > id)
            })
            .unwrap_or(pages.len()),
        None => 0,
    };
    let limit = limit.clamp(1, 100);
    let window = &work[start..work.len().min(start + limit)];
    let next_cursor = (start + window.len() < work.len())
        .then(|| window.last().map(|(page, _)| cursor_for(page)))
        .flatten();
    let mut items = Vec::with_capacity(window.len());
    for (page, reason) in window {
        let has_basis = db.page_has_basis(&page.id).await?;
        let code_work = is_code_work(reason);
        let ids = current_source_ids(db, page).await?;
        let source_count: u32 = db
            .get_memory_contents_by_ids(&ids)
            .await?
            .len()
            .try_into()
            .unwrap_or(u32::MAX);
        // A note written from code alone has no sources to lose.
        let nothing_left = !code_work && source_count == 0;
        // Judging changed code never touches the note, so a note that rests
        // on other notes is only the user's once it would be rewritten.
        let needs_you = nothing_left || (has_basis && reason != STEEP_REASON_CODE_CHECK);
        items.push(SteepWorkItem {
            page_id: page.id.clone(),
            title: page.title.clone(),
            space: page.space.clone(),
            stale_reason: reason.clone(),
            human_owned: owned(page, code_work),
            needs_you,
            blocked_reason: page.refresh_blocked_reason.clone(),
            leased_by: db
                .steep_leases()
                .holder_of(&page.id)
                .map(|holder| holder.as_str().to_string()),
            source_count,
        });
    }
    Ok(SteepWorklist {
        items,
        total,
        next_cursor,
    })
}

fn needs_you(page: &crate::pages::Page, note: String) -> SteepClaim {
    SteepClaim {
        page_id: page.id.clone(),
        title: page.title.clone(),
        plan: STEEP_PLAN_NEEDS_YOU,
        token: None,
        held_until: None,
        version: page.version,
        stale_reason: page.stale_reason.clone().unwrap_or_default(),
        current_content: page.content.clone(),
        sources: Vec::new(),
        code: Vec::new(),
        judge_instructions: None,
        code_checks: Vec::new(),
        note,
    }
}

fn numbered(sources: Vec<(String, String)>) -> Vec<SteepSource> {
    sources
        .into_iter()
        .enumerate()
        .map(|(i, (memory_id, text))| SteepSource {
            n: u32::try_from(i + 1).unwrap_or(u32::MAX),
            memory_id,
            text,
        })
        .collect()
}

/// The sources a rewrite of `page` cites, or the answer that leaves the page
/// to the user. `allow_none`: the note may rest on no memory at all (a note
/// written from code only).
async fn rewrite_sources(
    db: &MemoryDB,
    page: &crate::pages::Page,
    fence: &PageFence,
    scope: &ReadScope,
    allow_none: bool,
) -> Result<Result<Vec<(String, String)>, SteepClaim>, SteepError> {
    // Same rule as the automatic run: a note that relies on other notes is
    // rebuilt only by an explicit "Update from sources", so a rewrite never
    // lands over text a person confirmed against those notes.
    if db.page_has_basis(&page.id).await? {
        db.set_page_refresh_blocked_reason_at_fence(
            &page.id,
            crate::synthesis::distill::REFRESH_BLOCKED_HAS_BASIS,
            fence,
        )
        .await?;
        return Ok(Err(needs_you(
            page,
            "This page relies on other pages. Open it in Wenlan and use Update from sources, \
             so you can check it against those pages."
                .into(),
        )));
    }

    let ids = current_source_ids(db, page).await?;
    if ids.len() > STEEP_SOURCE_CAP {
        return Ok(Err(needs_you(
            page,
            format!(
                "This page has more than {STEEP_SOURCE_CAP} sources, too many for one rewrite. \
                 Open it in Wenlan and use Update from sources."
            ),
        )));
    }
    if *scope != ReadScope::Global {
        let visible = db.get_memories_by_source_ids_scoped(&ids, scope).await?;
        let visible: std::collections::HashSet<&str> =
            visible.iter().map(|m| m.source_id.as_str()).collect();
        if ids.iter().any(|id| !visible.contains(id.as_str())) {
            return Ok(Err(needs_you(
                page,
                "This page draws on memories outside this Space. Steep it from a session \
                 that is not locked to one Space."
                    .into(),
            )));
        }
    }
    let sources = surviving_sources(db, &ids).await?;
    if sources.is_empty() && !(allow_none && ids.is_empty()) {
        // Nothing left to rebuild from. Marked so the automatic run and the
        // list both show it as the user's call, never as an endless update.
        db.set_page_refresh_blocked_reason_at_fence(&page.id, NEEDS_YOU_NO_SOURCES, fence)
            .await?;
        return Ok(Err(needs_you(
            page,
            "Every source of this page was removed, so there is nothing to rebuild it from. \
             The user decides: keep it as their own writing, rewrite it, or archive it."
                .into(),
        )));
    }
    Ok(Ok(sources))
}

/// Claim one note for an on-demand rewrite: snapshot it, hold it, and say
/// what the caller should do with it.
///
/// `scope` is the caller's Space lock. A note outside it answers as missing;
/// a note that draws on memories outside it needs the user, because a rewrite
/// that cannot see every source would drop the ones it cannot see.
///
/// `judge_instructions`: the system prompt for judging changed code, when no
/// background model judges it; `None` leaves `code_check` work to that model.
pub async fn claim(
    db: &MemoryDB,
    page_id: &str,
    scope: &ReadScope,
    judge_instructions: Option<&str>,
) -> Result<SteepClaim, SteepError> {
    claim_as(
        db,
        page_id,
        scope,
        judge_instructions,
        SteepHolder::OnDemand,
    )
    .await
}

/// [`claim`] for `holder`: the automatic run claims through the same path.
async fn claim_as(
    db: &MemoryDB,
    page_id: &str,
    scope: &ReadScope,
    judge_instructions: Option<&str>,
    holder: SteepHolder,
) -> Result<SteepClaim, SteepError> {
    // The fence is read before the fields it protects, the same order the
    // automatic refresh uses: a change landing after this read moves the
    // fence, and one landing before it is in the reads below.
    let fence = db
        .try_get_page_fence(page_id)
        .await?
        .ok_or(SteepError::NotFound)?;
    let page = db.get_page(page_id).await?.ok_or(SteepError::NotFound)?;
    if !scope.matches(page.space.as_deref()) {
        return Err(SteepError::NotFound);
    }
    if page.status != "active" || page.kind == "entity" {
        return Err(SteepError::NotWaiting(
            "this page is not waiting for steep".into(),
        ));
    }
    let stale_reason = match page.stale_reason.clone() {
        Some(reason) if reason == "source_updated" || reason == "source_removed" => reason,
        _ => return claim_code(db, page, fence, scope, judge_instructions, holder).await,
    };

    let sources = match rewrite_sources(db, &page, &fence, scope, false).await? {
        Ok(sources) => sources,
        Err(user) => return Ok(user),
    };

    let human_owned = owned(&page, false);
    let snapshot = Snapshot {
        fence,
        version: page.version,
        stale_reason: stale_reason.clone(),
        sources: sources.clone(),
        scope: scope.clone(),
        page_stale_reason: Some(stale_reason.clone()),
        code: Vec::new(),
    };
    let (token, held_until) = hold(db, page_id, holder, snapshot)?;

    let removed = stale_reason == "source_removed";
    let mut note = String::from(
        "Rewrite the page from the numbered sources only. Cite each claim with its source \
         number, like [1]. Keep the title and the page's language.",
    );
    if removed {
        note.push_str(
            " A source of this page was removed: drop everything that only it supported.",
        );
    } else {
        note.push_str(
            " If the sources changed but the page still says the right thing, commit \
             still_accurate instead of rewriting.",
        );
    }
    if human_owned {
        note.push_str(
            " The user wrote or edited this page, so the rewrite goes to them as a revision \
             card; the page itself does not change until they accept it.",
        );
    }

    Ok(SteepClaim {
        page_id: page.id.clone(),
        title: page.title.clone(),
        plan: if human_owned {
            STEEP_PLAN_REVIEW_CARD
        } else {
            STEEP_PLAN_REWRITE
        },
        token: Some(token),
        held_until: Some(held_until),
        version: page.version,
        stale_reason,
        current_content: page.content,
        sources: numbered(sources),
        code: Vec::new(),
        judge_instructions: None,
        code_checks: Vec::new(),
        note,
    })
}

fn not_waiting() -> SteepError {
    SteepError::NotWaiting("this page is not waiting for steep".into())
}

fn hold(
    db: &MemoryDB,
    page_id: &str,
    holder: SteepHolder,
    snapshot: Snapshot,
) -> Result<(String, i64), SteepError> {
    let duration = match holder {
        SteepHolder::OnDemand => ON_DEMAND_HOLD,
        SteepHolder::Automatic => AUTOMATIC_HOLD,
    };
    let token = db
        .steep_leases()
        .take(page_id, holder, duration, Some(snapshot))
        .map_err(SteepError::Held)?;
    let held_until =
        chrono::Utc::now().timestamp() + i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    Ok((token, held_until))
}

/// Claim a note for work on the code it relies on: a rewrite when a model
/// judged a changed code basis `stale`, else (with `judge_instructions`) the
/// questions about changed code no model has judged yet.
async fn claim_code(
    db: &MemoryDB,
    page: crate::pages::Page,
    fence: PageFence,
    scope: &ReadScope,
    judge_instructions: Option<&str>,
    holder: SteepHolder,
) -> Result<SteepClaim, SteepError> {
    let mut stale = Vec::new();
    let mut unjudged = Vec::new();
    for reference in db.list_page_code_basis(&page.id).await? {
        if reference.state != crate::code_basis::CODE_BASIS_STATE_PENDING {
            continue;
        }
        let locator = CodeBasisLocator::from(&reference);
        let Some(row) = db.get_code_basis_row(&page.id, &locator).await? else {
            continue;
        };
        match reference.ai_verdict.as_deref() {
            Some(crate::code_basis_recheck::STORED_STALE) => {
                // Lines that changed again since the verdict are judged
                // afresh, the way the background recheck would.
                match crate::code_basis_recheck::current_code_excerpt(&row).await {
                    Some((code_digest, span, excerpt))
                        if row.recheck.digest.as_deref()
                            == Some(
                                crate::code_basis_recheck::recheck_key(&code_digest, page.version)
                                    .as_str(),
                            ) =>
                    {
                        stale.push((row, code_digest, span, excerpt));
                    }
                    Some(_) => unjudged.push(row),
                    None => {}
                }
            }
            None => unjudged.push(row),
            Some(_) => {}
        }
    }

    if !stale.is_empty() {
        if db.get_pending_revision_for(&page.id).await?.is_some() {
            return Err(SteepError::NotWaiting(
                "a revision card for this page is already waiting for the user".into(),
            ));
        }
        let sources = match rewrite_sources(db, &page, &fence, scope, true).await? {
            Ok(sources) => sources,
            Err(user) => return Ok(user),
        };
        let mut code = Vec::new();
        let mut snaps = Vec::new();
        for (row, code_digest, span, excerpt) in stale {
            let Some(key) = row.recheck.digest.clone() else {
                continue;
            };
            code.push(SteepCode {
                path: row.reference.path.clone(),
                line_start: span.start,
                line_end: span.end,
                excerpt,
                what_changed: row.recheck.note.clone().unwrap_or_default(),
            });
            snaps.push(CodeSnap {
                locator: CodeBasisLocator::from(&row.reference),
                anchor_json: row.anchor_json.clone(),
                key,
                code_digest,
            });
        }
        if snaps.is_empty() {
            return Err(not_waiting());
        }
        let human_owned = owned(&page, true);
        let (token, held_until) = hold(
            db,
            &page.id,
            holder,
            Snapshot {
                fence,
                version: page.version,
                stale_reason: STEEP_REASON_CODE_CHANGED.to_string(),
                sources: sources.clone(),
                scope: scope.clone(),
                page_stale_reason: page.stale_reason.clone(),
                code: snaps,
            },
        )?;
        let mut note = String::from(
            "The code this page relies on changed, and the page now says something the code \
             contradicts. Correct the page so it agrees with the code below (lines marked > \
             are the ones it relies on); keep the rest of the page, its title, language and \
             citations. The code is not a numbered source: a sentence only the code supports \
             carries no [n]. If the page still agrees with the code, commit still_accurate.",
        );
        if human_owned {
            note.push_str(
                " The user edited this page, so the rewrite goes to them as a revision card; the \
                 page itself does not change until they accept it.",
            );
        }
        return Ok(SteepClaim {
            page_id: page.id.clone(),
            title: page.title.clone(),
            plan: if human_owned {
                STEEP_PLAN_REVIEW_CARD
            } else {
                STEEP_PLAN_REWRITE
            },
            token: Some(token),
            held_until: Some(held_until),
            version: page.version,
            stale_reason: STEEP_REASON_CODE_CHANGED.to_string(),
            current_content: page.content,
            sources: numbered(sources),
            code,
            judge_instructions: None,
            code_checks: Vec::new(),
            note,
        });
    }

    let Some(instructions) = judge_instructions else {
        return Err(not_waiting());
    };
    let mut asks = Vec::new();
    for row in unjudged {
        if let Some(ask) = crate::code_basis_recheck::prepare_code_basis_ask(db, &row).await? {
            asks.push(ask);
        }
    }
    if asks.is_empty() {
        return Err(not_waiting());
    }
    let code_checks = asks
        .iter()
        .enumerate()
        .map(|(i, ask)| SteepCodeCheck {
            n: u32::try_from(i + 1).unwrap_or(u32::MAX),
            path: ask.row.reference.path.clone(),
            prompt: ask.prompt.clone(),
        })
        .collect();
    let snaps = asks
        .iter()
        .map(|ask| CodeSnap {
            locator: CodeBasisLocator::from(&ask.row.reference),
            anchor_json: ask.row.anchor_json.clone(),
            key: ask.key.clone(),
            code_digest: ask.code_digest.clone(),
        })
        .collect();
    let (token, held_until) = hold(
        db,
        &page.id,
        holder,
        Snapshot {
            fence,
            version: page.version,
            stale_reason: STEEP_REASON_CODE_CHECK.to_string(),
            sources: Vec::new(),
            scope: scope.clone(),
            page_stale_reason: page.stale_reason.clone(),
            code: snaps,
        },
    )?;
    Ok(SteepClaim {
        page_id: page.id.clone(),
        title: page.title.clone(),
        plan: STEEP_PLAN_JUDGE,
        token: Some(token),
        held_until: Some(held_until),
        version: page.version,
        stale_reason: STEEP_REASON_CODE_CHECK.to_string(),
        current_content: page.content,
        sources: Vec::new(),
        code: Vec::new(),
        judge_instructions: Some(instructions.to_string()),
        code_checks,
        note: "Code this page relies on changed. Put each check to a small model: \
               judge_instructions as the system prompt, the check's prompt as the user \
               message. Pass each reply back unchanged with action judge, as answers \
               [{n, answer}]. Do not rewrite the page."
            .into(),
    })
}

/// A recorded commit, handed back for the same token.
fn replayed(stored: &str) -> Result<SteepCommit, WenlanError> {
    let result: WriteResult = serde_json::from_str(stored)?;
    let outcome = if result.gated {
        "review_card"
    } else if result.wrote {
        "written"
    } else if result.acknowledged {
        "unchanged"
    } else {
        "still_accurate"
    };
    Ok(SteepCommit {
        page_id: result.id,
        outcome,
        revision_card_id: result.revision_card_id,
        citations: None,
        judged: None,
        replayed: true,
    })
}

const CHANGED: &str = "the page or its sources changed since you claimed it, so nothing was \
                       written; claim it again and work from the new snapshot";

/// The code bases a `code_changed` claim is about, while each still is what
/// the claim saw: the same stored anchor, still `pending`, still judged
/// `stale` under the same key, and the file still reading as shown.
async fn code_unmoved(
    db: &MemoryDB,
    page_id: &str,
    snapshot: &Snapshot,
) -> Result<Option<Vec<crate::db::CodeBasisRow>>, WenlanError> {
    let mut rows = Vec::with_capacity(snapshot.code.len());
    for snap in &snapshot.code {
        let Some(row) = db.get_code_basis_row(page_id, &snap.locator).await? else {
            return Ok(None);
        };
        let still_stale = row.reference.state == crate::code_basis::CODE_BASIS_STATE_PENDING
            && row.recheck.digest.as_deref() == Some(snap.key.as_str())
            && row.recheck.verdict.as_deref() == Some(crate::code_basis_recheck::STORED_STALE);
        if row.anchor_json != snap.anchor_json || !still_stale {
            return Ok(None);
        }
        // The file itself, read again: the verdict stays on the row until a
        // later judgement replaces it, so only the lines can say they moved.
        let now = crate::code_basis_recheck::current_code_excerpt(&row).await;
        if now.is_none_or(|(digest, _, _)| digest != snap.code_digest) {
            return Ok(None);
        }
        rows.push(row);
    }
    Ok(Some(rows))
}

/// `confirmed_by` of a code basis settled by a steep rewrite (or a steep run
/// that found the note still agrees with the code).
pub const STEEP_CONFIRMED_BY: &str = "steep";

/// Settle the code bases a `code_changed` claim was about, at the lines the
/// caller was shown. A row whose lines moved on since is left to be judged
/// again.
async fn confirm_code(
    db: &MemoryDB,
    rows: &[crate::db::CodeBasisRow],
    snapshot: &Snapshot,
) -> Result<usize, WenlanError> {
    let mut confirmed = 0;
    for (row, snap) in rows.iter().zip(&snapshot.code) {
        if crate::code_basis_recheck::confirm_code_basis_as_seen(
            db,
            row,
            &snap.code_digest,
            STEEP_CONFIRMED_BY,
        )
        .await?
        {
            confirmed += 1;
        }
    }
    Ok(confirmed)
}

/// Land the caller's answers to a `judge` claim, each the way the background
/// recheck lands its own model's: only while the row, the lines and the note
/// are what the question showed.
async fn judge(
    db: &MemoryDB,
    page_id: &str,
    snapshot: &Snapshot,
    answers: &[SteepCodeAnswer],
) -> Result<String, WenlanError> {
    use crate::code_basis_recheck::{
        apply_code_basis_judgement, parse_judgement, prepare_code_basis_ask,
        record_code_basis_no_verdict, Applied,
    };
    let (mut no_impact, mut stale, mut person, mut unusable, mut moved, mut unanswered) =
        (0, 0, 0, 0, 0, 0);
    for (i, snap) in snapshot.code.iter().enumerate() {
        let n = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let Some(answer) = answers.iter().find(|answer| answer.n == n) else {
            unanswered += 1;
            continue;
        };
        let row = db
            .get_code_basis_row(page_id, &snap.locator)
            .await?
            .filter(|row| row.anchor_json == snap.anchor_json);
        let ask = match row {
            Some(row) => prepare_code_basis_ask(db, &row).await?,
            None => None,
        };
        let Some(ask) =
            ask.filter(|ask| ask.key == snap.key && ask.code_digest == snap.code_digest)
        else {
            moved += 1;
            continue;
        };
        let Some(judgement) = parse_judgement(&answer.answer) else {
            record_code_basis_no_verdict(db, &ask).await?;
            unusable += 1;
            continue;
        };
        match apply_code_basis_judgement(db, &ask, &judgement).await? {
            Applied::NoImpact => no_impact += 1,
            Applied::Stale => stale += 1,
            Applied::NeedsHuman => person += 1,
            Applied::Superseded => moved += 1,
        }
    }
    let parts: Vec<String> = [
        (no_impact, "no impact"),
        (stale, "stale"),
        (person, "needs a person"),
        (unusable, "no usable answer"),
        (moved, "changed meanwhile"),
        (unanswered, "unanswered"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}"))
    .collect();
    Ok(if parts.is_empty() {
        "nothing judged".to_string()
    } else {
        parts.join(", ")
    })
}

/// Settle a claimed note: write the rewrite, stage it as a revision card,
/// record that the note still holds, land judgements about its changed code,
/// or give it back. `agent` is the tool committing, named on the page's
/// history and on a revision card.
#[allow(clippy::too_many_arguments)]
pub async fn commit(
    db: &MemoryDB,
    token: &str,
    action: &str,
    content: Option<&str>,
    answers: &[SteepCodeAnswer],
    knowledge_path: Option<&std::path::Path>,
    agent: Option<&str>,
) -> Result<SteepCommit, SteepError> {
    if let Some(stored) = db
        .get_operation_receipt(STEEP_RECEIPT_CALLER, token)
        .await?
    {
        return Ok(replayed(&stored.response)?);
    }
    let lease = db.steep_leases().find(token).ok_or(SteepError::TokenGone)?;
    let page_id = lease.page_id.clone();
    let Some(snapshot) = lease.snapshot.clone() else {
        // Automatic holds have no token anyone outside this process sees.
        return Err(SteepError::TokenGone);
    };
    let release = || db.steep_leases().release(&page_id, token);

    match action {
        STEEP_ACTION_RELEASE => {
            release();
            return Ok(SteepCommit {
                page_id,
                outcome: "released",
                revision_card_id: None,
                citations: None,
                judged: None,
                replayed: false,
            });
        }
        STEEP_ACTION_REWRITE | STEEP_ACTION_STILL_ACCURATE | STEEP_ACTION_JUDGE => {}
        other => {
            return Err(SteepError::Invalid(format!(
                "unknown action '{other}'; use rewrite, still_accurate, judge or release"
            )))
        }
    }
    let judging = snapshot.stale_reason == STEEP_REASON_CODE_CHECK;
    if judging != (action == STEEP_ACTION_JUDGE) {
        return Err(SteepError::Invalid(if judging {
            "this claim asks for judgements: pass the answers with action judge".into()
        } else {
            "judge is only for a claim whose plan is judge".into()
        }));
    }

    // Nothing may have moved since the claim: the same row, version, source
    // revision and source text, and the note still waiting.
    let fence = db.try_get_page_fence(&page_id).await?;
    let page = db.get_page(&page_id).await?;
    let unmoved = match (&fence, &page) {
        (Some(fence), Some(page)) => {
            *fence == snapshot.fence
                && page.version == snapshot.version
                && page.stale_reason == snapshot.page_stale_reason
        }
        _ => false,
    };
    if !unmoved {
        release();
        return Err(SteepError::Changed(CHANGED.into()));
    }
    // Moved out of the claimer's Space: the same not found as a page that
    // was never there. A rewrite re-checks this inside its own write.
    if !page
        .as_ref()
        .is_some_and(|page| snapshot.scope.matches(page.space.as_deref()))
    {
        release();
        return Err(SteepError::NotFound);
    }
    let ids: Vec<String> = snapshot.sources.iter().map(|(id, _)| id.clone()).collect();
    if surviving_sources(db, &ids).await? != snapshot.sources {
        release();
        return Err(SteepError::Changed(CHANGED.into()));
    }

    if judging {
        let judged = judge(db, &page_id, &snapshot, answers).await;
        release();
        return Ok(SteepCommit {
            page_id,
            outcome: "judged",
            revision_card_id: None,
            citations: None,
            judged: Some(judged?),
            replayed: false,
        });
    }
    let code_rows = if is_code_work(&snapshot.stale_reason) {
        match code_unmoved(db, &page_id, &snapshot).await? {
            Some(rows) => rows,
            None => {
                release();
                return Err(SteepError::Changed(CHANGED.into()));
            }
        }
    } else {
        Vec::new()
    };

    if action == STEEP_ACTION_STILL_ACCURATE && !code_rows.is_empty() {
        // The main model read the changed code and finds the note still
        // agrees with it: the code bases are settled at the lines it read.
        let confirmed = confirm_code(db, &code_rows, &snapshot).await;
        release();
        if confirmed? == 0 {
            return Err(SteepError::Changed(CHANGED.into()));
        }
        return Ok(SteepCommit {
            page_id,
            outcome: "still_accurate",
            revision_card_id: None,
            citations: None,
            judged: None,
            replayed: false,
        });
    }
    if action == STEEP_ACTION_STILL_ACCURATE {
        if snapshot.stale_reason != "source_updated" {
            return Err(SteepError::Invalid(
                "a source of this page was removed, so it must be rewritten without it; \
                 still_accurate is only for a page whose sources changed"
                    .into(),
            ));
        }
        let response = serde_json::to_string(&WriteResult {
            id: page_id.clone(),
            attached_to: None,
            warnings: Vec::new(),
            wrote: false,
            revision_card_id: None,
            gated: false,
            acknowledged: false,
            outcome: WriteOutcome::Unchanged,
            basis_confirm: None,
        })
        .map_err(WenlanError::from)?;
        let acknowledged = db
            .acknowledge_page_compile_at_fence(
                &page_id,
                snapshot.version,
                &snapshot.fence,
                Some(crate::db::OperationReceipt {
                    caller_id: STEEP_RECEIPT_CALLER,
                    operation_id: token,
                    request_digest: STEEP_ACTION_STILL_ACCURATE,
                    response: &response,
                }),
            )
            .await?;
        release();
        if !acknowledged {
            return Err(SteepError::Changed(CHANGED.into()));
        }
        return Ok(SteepCommit {
            page_id,
            outcome: "still_accurate",
            revision_card_id: None,
            citations: None,
            judged: None,
            replayed: false,
        });
    }

    let content = content.map(str::trim).unwrap_or_default();
    if content.is_empty() {
        return Err(SteepError::Invalid(
            "a rewrite needs content: the new body of the page".into(),
        ));
    }
    crate::export::provenance::validate_canonical_page_content(content)?;

    // The daemon, not the caller, turns `[n]` into evidence: it numbers the
    // sources, strips markers that point nowhere, and checks each cited
    // sentence against the source text. Same gate as the automatic run.
    let kinds = db.resolve_source_kinds(&ids).await.unwrap_or_default();
    let numbered: Vec<crate::citations::NumberedSource> = snapshot
        .sources
        .iter()
        .enumerate()
        .map(|(i, (id, text))| crate::citations::NumberedSource {
            index: u32::try_from(i + 1).unwrap_or(u32::MAX),
            source_kind: kinds
                .get(id)
                .cloned()
                .unwrap_or_else(|| "memory".to_string()),
            locator: id.clone(),
            text: text.clone(),
        })
        .collect();
    let (body, cites, stats) = crate::citations::process_citation_output(content, &numbered);
    // A marker that names no source would leave its sentence on the page with
    // nothing behind it, so the caller fixes it instead.
    if stats.stripped > 0 {
        return Err(SteepError::CitationGate(format!(
            "the rewrite was not saved: {} citation(s) name a source number that is not \
             in this claim ({}). Use only the numbers [1] to [{}] from the claim, or drop \
             the sentence; then commit again with the same token",
            stats.stripped,
            stats.summary(),
            numbered.len()
        )));
    }
    // A sentence corrected for the code carries no [n], so a rewrite for code
    // need not cite at all: only the markers it does carry must match their
    // source. A rewrite from sources must cite.
    let too_few = if code_rows.is_empty() {
        stats.verified == 0 || stats.unverified > stats.verified
    } else {
        stats.unverified > stats.verified
    };
    if too_few {
        return Err(SteepError::CitationGate(format!(
            "the rewrite was not saved: too few cited sentences match their source \
             ({}). Cite each claim with the number of the source that says it, like [1], \
             and keep the wording close to the source; then commit again with the same token",
            stats.summary()
        )));
    }
    let citations_json = serde_json::to_string(&cites).map_err(WenlanError::from)?;
    let summary = crate::synthesis::distill::extract_page_summary(&body);

    let space_guard = (snapshot.scope != ReadScope::Global).then(|| snapshot.scope.clone());
    let result = crate::post_write::steep_update_page(
        db,
        &page_id,
        UpdatePageRequest {
            content: body,
            source_memory_ids: ids,
            expected_version: Some(snapshot.version),
            caller_id: Some(STEEP_RECEIPT_CALLER.to_string()),
            operation_id: Some(token.to_string()),
        },
        agent,
        space_guard,
        snapshot.fence.source_revision,
        // A note rewritten for its code is not stale by its sources, and is
        // a card only when a person edited it.
        (!code_rows.is_empty()).then(|| page.as_ref().is_some_and(|page| owned(page, true))),
        knowledge_path,
        Some((citations_json, stats.summary())),
    )
    .await;
    release();
    let result = match result {
        Ok(result) => result,
        // The page left the claimer's Space between the check above and the
        // write, which refused it in its own transaction.
        Err(WenlanError::NotFound(_)) => return Err(SteepError::NotFound),
        Err(error) => return Err(error.into()),
    };
    let outcome = if result.gated {
        "review_card"
    } else if result.wrote {
        if let Err(error) = db.update_page_summary(&page_id, summary.as_deref()).await {
            log::warn!("[steep] {page_id} rewritten; summary not updated: {error}");
        }
        // The note now says what the code says: its code bases are settled at
        // the lines the rewrite was made from. A card settles nothing; the
        // note stays off the list until the user decides it.
        if !code_rows.is_empty() {
            if let Err(error) = confirm_code(db, &code_rows, &snapshot).await {
                log::warn!("[steep] {page_id} rewritten; code basis not settled: {error}");
            }
        }
        "written"
    } else if result.outcome == WriteOutcome::Unchanged && result.acknowledged {
        "unchanged"
    } else {
        return Err(SteepError::Changed(CHANGED.into()));
    };
    Ok(SteepCommit {
        page_id,
        outcome,
        revision_card_id: result.revision_card_id,
        citations: Some(stats.summary()),
        judged: None,
        replayed: false,
    })
}

#[path = "steep_automatic.rs"]
mod automatic;
pub use automatic::{rewrite_one_code_page, CodeRewriteOutcome};

#[cfg(test)]
#[path = "steep_test.rs"]
mod steep_test;
