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
//! Holds live in memory ([`SteepLeases`]). Both runs live in this daemon
//! process, so a restart ends every hold together with the work behind it; a
//! caller whose token is gone claims again.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wenlan_types::requests::UpdatePageRequest;
use wenlan_types::steep::{
    SteepSource, SteepWorkItem, STEEP_ACTION_RELEASE, STEEP_ACTION_REWRITE,
    STEEP_ACTION_STILL_ACCURATE, STEEP_PLAN_NEEDS_YOU, STEEP_PLAN_REVIEW_CARD, STEEP_PLAN_REWRITE,
};

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
    pub note: String,
}

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteepCommit {
    pub page_id: String,
    /// `written`, `review_card`, `still_accurate`, `unchanged` or `released`.
    pub outcome: &'static str,
    pub revision_card_id: Option<String>,
    pub citations: Option<String>,
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
    Ok(db
        .list_steep_pages()
        .await?
        .into_iter()
        .map(|page| page.id)
        .collect())
}

fn cursor_for(page: &crate::pages::Page) -> String {
    format!("{}|{}", page.last_modified, page.id)
}

/// The notes waiting for steep in `scope`, `limit` at a time after `cursor`.
/// `visible` says which notes the caller may see at all; the rest are left
/// out of both the items and the total.
pub async fn list_work(
    db: &MemoryDB,
    scope: &ReadScope,
    visible: impl Fn(&str) -> bool,
    limit: usize,
    cursor: Option<&str>,
) -> Result<SteepWorklist, WenlanError> {
    let pages: Vec<_> = db
        .list_steep_pages()
        .await?
        .into_iter()
        .filter(|page| scope.matches(page.space.as_deref()) && visible(&page.id))
        .collect();
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
    let window = &pages[start..pages.len().min(start + limit)];
    let next_cursor = (start + window.len() < pages.len())
        .then(|| window.last().map(cursor_for))
        .flatten();
    let mut items = Vec::with_capacity(window.len());
    for page in window {
        let ids = current_source_ids(db, page).await?;
        let source_count = db
            .get_memory_contents_by_ids(&ids)
            .await?
            .len()
            .try_into()
            .unwrap_or(u32::MAX);
        let has_basis = db.page_has_basis(&page.id).await?;
        items.push(SteepWorkItem {
            page_id: page.id.clone(),
            title: page.title.clone(),
            space: page.space.clone(),
            stale_reason: page.stale_reason.clone().unwrap_or_default(),
            human_owned: crate::post_write::page_is_human_owned(page) || page.review_required,
            needs_you: source_count == 0 || has_basis,
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
        note,
    }
}

/// Claim one note for an on-demand rewrite: snapshot it, hold it, and say
/// what the caller should do with it.
///
/// `scope` is the caller's Space lock. A note outside it answers as missing;
/// a note that draws on memories outside it needs the user, because a rewrite
/// that cannot see every source would drop the ones it cannot see.
pub async fn claim(
    db: &MemoryDB,
    page_id: &str,
    scope: &ReadScope,
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
    let Some(stale_reason) = page.stale_reason.clone() else {
        return Err(SteepError::NotWaiting(
            "this page is not waiting for steep".into(),
        ));
    };
    if page.status != "active" || page.kind == "entity" {
        return Err(SteepError::NotWaiting(
            "this page is not waiting for steep".into(),
        ));
    }

    // Same rule as the automatic run: a note that relies on other notes is
    // rebuilt only by an explicit "Update from sources", so a rewrite never
    // lands over text a person confirmed against those notes.
    if db.page_has_basis(page_id).await? {
        db.set_page_refresh_blocked_reason_at_fence(
            page_id,
            crate::synthesis::distill::REFRESH_BLOCKED_HAS_BASIS,
            &fence,
        )
        .await?;
        return Ok(needs_you(
            &page,
            "This page relies on other pages. Open it in Wenlan and use Update from sources, \
             so you can check it against those pages."
                .into(),
        ));
    }

    let ids = current_source_ids(db, &page).await?;
    if ids.len() > STEEP_SOURCE_CAP {
        return Ok(needs_you(
            &page,
            format!(
                "This page has more than {STEEP_SOURCE_CAP} sources, too many for one rewrite. \
                 Open it in Wenlan and use Update from sources."
            ),
        ));
    }
    if *scope != ReadScope::Global {
        let visible = db.get_memories_by_source_ids_scoped(&ids, scope).await?;
        let visible: std::collections::HashSet<&str> =
            visible.iter().map(|m| m.source_id.as_str()).collect();
        if ids.iter().any(|id| !visible.contains(id.as_str())) {
            return Ok(needs_you(
                &page,
                "This page draws on memories outside this Space. Steep it from a session \
                 that is not locked to one Space."
                    .into(),
            ));
        }
    }
    let sources = surviving_sources(db, &ids).await?;
    if sources.is_empty() {
        // Nothing left to rebuild from. Marked so the automatic run and the
        // list both show it as the user's call, never as an endless update.
        db.set_page_refresh_blocked_reason_at_fence(page_id, NEEDS_YOU_NO_SOURCES, &fence)
            .await?;
        return Ok(needs_you(
            &page,
            "Every source of this page was removed, so there is nothing to rebuild it from. \
             The user decides: keep it as their own writing, rewrite it, or archive it."
                .into(),
        ));
    }

    let human_owned = crate::post_write::page_is_human_owned(&page) || page.review_required;
    let snapshot = Snapshot {
        fence,
        version: page.version,
        stale_reason: stale_reason.clone(),
        sources: sources.clone(),
        scope: scope.clone(),
    };
    let token = db
        .steep_leases()
        .take(
            page_id,
            SteepHolder::OnDemand,
            ON_DEMAND_HOLD,
            Some(snapshot),
        )
        .map_err(SteepError::Held)?;
    let held_until = chrono::Utc::now().timestamp()
        + i64::try_from(ON_DEMAND_HOLD.as_secs()).unwrap_or(i64::MAX);

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
        sources: sources
            .into_iter()
            .enumerate()
            .map(|(i, (memory_id, text))| SteepSource {
                n: u32::try_from(i + 1).unwrap_or(u32::MAX),
                memory_id,
                text,
            })
            .collect(),
        note,
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
        replayed: true,
    })
}

const CHANGED: &str = "the page or its sources changed since you claimed it, so nothing was \
                       written; claim it again and work from the new snapshot";

/// Settle a claimed note: write the rewrite, stage it as a revision card,
/// record that the note still holds, or give it back. `agent` is the tool
/// committing, named on the page's history and on a revision card.
pub async fn commit(
    db: &MemoryDB,
    token: &str,
    action: &str,
    content: Option<&str>,
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
                replayed: false,
            });
        }
        STEEP_ACTION_REWRITE | STEEP_ACTION_STILL_ACCURATE => {}
        other => {
            return Err(SteepError::Invalid(format!(
                "unknown action '{other}'; use rewrite, still_accurate or release"
            )))
        }
    }

    // Nothing may have moved since the claim: the same row, version, source
    // revision and source text, and the note still waiting.
    let fence = db.try_get_page_fence(&page_id).await?;
    let page = db.get_page(&page_id).await?;
    let unmoved = match (&fence, &page) {
        (Some(fence), Some(page)) => {
            *fence == snapshot.fence
                && page.version == snapshot.version
                && page.stale_reason.as_deref() == Some(snapshot.stale_reason.as_str())
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
    if stats.verified == 0 || stats.unverified > stats.verified {
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
        replayed: false,
    })
}

#[cfg(test)]
#[path = "steep_test.rs"]
mod steep_test;
