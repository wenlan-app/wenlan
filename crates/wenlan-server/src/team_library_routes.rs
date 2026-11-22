// SPDX-License-Identifier: AGPL-3.0-only
//! The authenticated team library surface. It never routes a miss to the
//! host's personal retrieval or pages endpoints.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, Router};
use serde::Deserialize;
use wenlan_core::shared_space::version::{self, PageVersion};
use wenlan_core::team::{self, Member};
use wenlan_core::team_library::{
    LegacyFile, LegacyHead, LegacySpace, Library, LibraryError, Reader,
};
use wenlan_mcp::tools::team_library as tools;
use wenlan_types::team::{TeamAccess, TeamPerson, TeamRole};
use wenlan_types::team_library::{
    TeamAuthorship, TeamGroup, TeamGroupCreate, TeamItem, TeamItemKind, TeamLibraryGrant,
    TeamOriginal, TeamPublication, TeamPublicationConfirm, TeamPublicationPreview,
    TeamPublicationReceipt, TeamTerminalAction, TeamTerminalReply, TeamToolKeyCreate,
    TeamToolKeyReceipt, TeamToolScope,
};

use super::TeamState;
use crate::member_view::{self, MemberBody, MemberResponse};

pub(super) const PREFIX: &str = "/team/v1/library";
type McpSessions = Arc<Mutex<HashMap<McpSessionKey, super::McpService>>>;

#[derive(Eq, Hash, PartialEq)]
struct McpSessionKey {
    member: String,
    pin: Option<String>,
    tool_key: Option<String>,
    rules: Option<(String, TeamAccess)>,
}

#[derive(Clone)]
pub(super) struct Host {
    pub(super) library: Library,
    previews: Arc<Mutex<HashMap<String, Pending>>>,
    migration: Arc<tokio::sync::Mutex<()>>,
    mcp: McpSessions,
}

struct Pending {
    member: String,
    session: String,
    preview: TeamPublicationPreview,
    originals: Vec<TeamOriginal>,
    created: Instant,
}

const REVIEW_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING: usize = 32;

impl Host {
    pub(super) fn new(team_path: &Path, memories: bool) -> Self {
        Self {
            library: Library::new(team_path.to_owned(), memories),
            previews: Arc::default(),
            migration: Arc::default(),
            mcp: Arc::default(),
        }
    }

    pub(super) async fn migrate(&self, state: &TeamState) -> Result<(), LibraryError> {
        let _migration = self.migration.lock().await;
        let spaces = team::hosted_spaces(&state.settings_path)?;
        if spaces.is_empty() {
            return self.library.reconcile_legacy_spaces(Vec::new());
        }
        let db = state
            .db
            .as_ref()
            .ok_or(LibraryError::Invalid("The host database is unavailable."))?;
        let file = team::read_team(&state.team_path)?;
        let info = file.info().ok_or(LibraryError::Absent)?;
        let mut migrated = Vec::new();
        for (name, folder) in spaces {
            let id = team::hosted_space_id(&folder)
                .ok_or(LibraryError::Invalid("Invalid hosted Space ID."))?;
            let group = TeamGroup {
                id,
                name,
                restricted: true,
            };
            let authorized = db.shared_folder_pages(&group.name).await?;
            let held =
                wenlan_core::shared_space::held_hosted_page_ids(db, &group.name, &folder).await?;
            let possible_copies = held
                .chain(authorized.known_page_ids().map(str::to_owned))
                .filter(|id| {
                    version::is_safe_id(id) && !wenlan_core::rulesync_team::is_master_page(id)
                })
                .collect();
            let heads = if self.library.needs_migration(&group)? {
                let scope = wenlan_core::read_scope::ReadScope::Space(group.name.clone());
                let pages = db
                    .list_shared_pages_scoped("active", i64::MAX, 0, &scope)
                    .await?;
                let mut versions = legacy_heads(&folder, &authorized);
                pages
                    .into_iter()
                    .filter(|page| {
                        authorized
                            .legacy_head(&page.id)
                            .is_some_and(|proof| proof.revision == page.version)
                            && !matches!(
                                page.stale_reason.as_deref(),
                                Some("source_updated" | "source_removed")
                            )
                    })
                    .map(|mut page| {
                        let private_source =
                            !page.citations.is_empty() || !page.source_memory_ids.is_empty();
                        page.content = wenlan_core::team_library::draft_page_body(&page.content);
                        let projected = wenlan_core::member_view::page(page);
                        let versions = versions.remove(&projected.id).unwrap_or_default();
                        let parsed: Vec<_> =
                            versions.iter().map(|(page, _)| page.clone()).collect();
                        let current = version::head(&parsed).map(|head| head.winner);
                        let matching = current.filter(|head| {
                            !head.meta.archived
                                && head.page.title == projected.title
                                && version::body_digest(&projected.content) == head.meta.body_digest
                        });
                        let editor = matching
                            .and_then(|head| {
                                file.members
                                    .iter()
                                    .find(|member| member.id == head.meta.author.member)
                            })
                            .map(|member| TeamPerson {
                                id: member.id.clone(),
                                name: member.name.clone(),
                                owner: member.role == TeamRole::Owner,
                            });
                        let authorship = matching.map(|head| match head.meta.author.kind {
                            version::AuthorKind::Person => TeamAuthorship::Human,
                            version::AuthorKind::Ai => TeamAuthorship::Ai,
                        });
                        let legacy = matching
                            .filter(|head| {
                                head.page.citations.is_empty() && head.page.basis.is_empty()
                            })
                            .and_then(|head| {
                                versions.into_iter().find(|(page, _)| page.sha == head.sha)
                            })
                            .map(|(_, file)| file);
                        LegacyHead {
                            item: TeamItem {
                                team_id: info.id.clone(),
                                item_id: projected.id,
                                kind: TeamItemKind::Page,
                                revision: projected.version,
                                space_id: Some(group.id.clone()),
                                title: projected.title,
                                body: projected.content,
                                contributor: None,
                                authorship,
                                publisher: None,
                                last_editor: editor,
                            },
                            private_source,
                            file: legacy,
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
            migrated.push(LegacySpace {
                group,
                folder,
                authorized,
                heads,
                possible_copies,
            });
        }
        self.library.reconcile_legacy_spaces(migrated)
    }

    pub(super) async fn reader(
        &self,
        state: &TeamState,
        member: &Member,
    ) -> Result<Reader, LibraryError> {
        self.migrate(state).await?;
        self.library.reader(&member.id, None)
    }

    pub(super) fn preview(
        &self,
        member: &str,
        session: &str,
        draft: TeamPublication,
        originals: Vec<TeamOriginal>,
    ) -> Result<TeamPublicationPreview, LibraryError> {
        let preview = self.library.preview(member, draft)?;
        let mut pending = self
            .previews
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        pending.retain(|_, entry| entry.created.elapsed() < REVIEW_TTL);
        if pending.len() >= MAX_PENDING {
            if let Some(id) = pending
                .iter()
                .min_by_key(|(_, entry)| entry.created)
                .map(|(id, _)| id.clone())
            {
                pending.remove(&id);
            }
        }
        pending.insert(
            preview.id.clone(),
            Pending {
                member: member.into(),
                session: session.into(),
                preview: preview.clone(),
                originals,
                created: Instant::now(),
            },
        );
        Ok(preview)
    }

    pub(super) fn publish(
        &self,
        member: &str,
        session: &str,
        id: &str,
    ) -> Result<TeamPublicationReceipt, LibraryError> {
        let preview = {
            let mut pending = self
                .previews
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending.retain(|_, entry| entry.created.elapsed() < REVIEW_TTL);
            match pending.get(id) {
                Some(entry) if entry.member == member && entry.session == session => {
                    Some(entry.preview.clone())
                }
                Some(_) => return Err(LibraryError::Absent),
                None => None,
            }
        };
        let Some(preview) = preview else {
            return self
                .library
                .retry(member, id)?
                .ok_or(LibraryError::Conflict(
                    "Review a new preview before publishing.",
                ));
        };
        let receipt = self.library.publish(member, preview)?;
        self.previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
        Ok(receipt)
    }
}

fn legacy_heads(
    folder: &Path,
    authorized: &team::SharedFolderPages,
) -> BTreeMap<String, Vec<(PageVersion, LegacyFile)>> {
    let mut versions: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for path in team::list_folder_files(folder, authorized) {
        let mut parts = path.split('/');
        let (Some(version::MEMBERS_DIR), Some(_), Some(version::PAGES_DIR), Some(id), Some(name)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            continue;
        };
        let Some(file) = team::served_path(folder, &path) else {
            continue;
        };
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        if !team::can_serve_version(id, name, &bytes, authorized) {
            continue;
        }
        let Ok(parsed) = version::parse(name, &bytes) else {
            continue;
        };
        if let Ok(bytes) = String::from_utf8(bytes) {
            versions
                .entry(id.into())
                .or_default()
                .push((parsed, LegacyFile { path, bytes }));
        }
    }
    versions
}

pub(super) fn failure(error: LibraryError) -> MemberResponse {
    match error {
        LibraryError::Denied => MemberResponse::problem(
            StatusCode::FORBIDDEN,
            "Team Write and an active membership are required.",
        ),
        LibraryError::Absent => {
            MemberResponse::problem(StatusCode::NOT_FOUND, "Team item not found.")
        }
        LibraryError::Conflict(reason) => MemberResponse::problem(StatusCode::CONFLICT, reason),
        LibraryError::Invalid(reason) => {
            MemberResponse::problem(StatusCode::UNPROCESSABLE_ENTITY, reason)
        }
        LibraryError::MemoriesDisabled => MemberResponse::problem(
            StatusCode::FORBIDDEN,
            "Team memory publication is disabled on this host.",
        ),
        LibraryError::Storage(error) => {
            tracing::warn!(%error, "Team library storage unavailable");
            MemberResponse::status(StatusCode::SERVICE_UNAVAILABLE)
        }
    }
}

fn reply<T: MemberBody>(result: Result<T, LibraryError>) -> MemberResponse {
    match result {
        Ok(value) => MemberResponse::json(StatusCode::OK, value),
        Err(error) => failure(error),
    }
}

fn host(state: &TeamState) -> Result<&Host, LibraryError> {
    state.library.as_ref().ok_or(LibraryError::Absent)
}

pub(super) fn routes() -> Router<TeamState> {
    use crate::member_view::{get, post, put};
    let router = Router::new();
    let router = get(info).install(router, "/team/v1/library");
    let router = get(pages).install(router, "/team/v1/library/pages");
    let router = get(page).install(router, "/team/v1/library/pages/{id}");
    let router = get(sources).install(router, "/team/v1/library/pages/{id}/sources");
    let router = get(history).install(router, "/team/v1/library/pages/{id}/history");
    let router = get(page_file).install(router, "/team/v1/library/pages/{id}/file");
    let router = get(memory).install(router, "/team/v1/library/memories/{id}");
    let router = get(recall).install(router, "/team/v1/library/recall");
    let router = get(brief).install(router, "/team/v1/library/brief");
    let router = post(preview).install(router, "/team/v1/library/preview");
    let router = post(publish).install(router, "/team/v1/library/publish");
    let router = post(withdraw).install(router, "/team/v1/library/withdraw/{id}");
    let router = post(create_group).install(router, "/team/v1/library/spaces");
    let router = put(set_grant).install(router, "/team/v1/library/members/{id}/grant");
    let router = post(issue_key).install(router, "/team/v1/library/tool-keys");
    let router = post(revoke_key).install(router, "/team/v1/library/tool-keys/{id}/revoke");
    router.layer(axum::middleware::from_fn(no_store))
}

async fn no_store(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    space_id: Option<String>,
    topic: Option<String>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn info(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
) -> MemberResponse {
    reply(async { Ok(host(&state)?.reader(&state, &member).await?.info()) }.await)
}

async fn pages(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Query(q): Query<Selection>,
) -> MemberResponse {
    reply(
        async {
            Ok(host(&state)?.reader(&state, &member).await?.pages(
                q.space_id.as_deref(),
                q.limit.unwrap_or(50),
                q.offset.unwrap_or(0),
            ))
        }
        .await,
    )
}

async fn page(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    reply(async { host(&state)?.reader(&state, &member).await?.page(&id) }.await)
}

async fn memory(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    reply(async { host(&state)?.reader(&state, &member).await?.memory(&id) }.await)
}

async fn sources(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    reply(async { host(&state)?.reader(&state, &member).await?.sources(&id) }.await)
}

async fn history(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    reply(async { host(&state)?.reader(&state, &member).await?.history(&id) }.await)
}

async fn page_file(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    match async { host(&state)?.reader(&state, &member).await?.page(&id) }.await {
        Ok(page) => {
            let mut file = wenlan_core::store::format::page::PageFile::new(page.title, page.body);
            file.id = Some(page.item_id);
            member_view::team::markdown_download(
                wenlan_core::store::format::page::render(&file).into_bytes(),
            )
        }
        Err(error) => failure(error),
    }
}

async fn recall(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Query(q): Query<Selection>,
) -> MemberResponse {
    reply(
        async {
            Ok(host(&state)?.reader(&state, &member).await?.recall(
                q.space_id.as_deref(),
                q.topic.as_deref().unwrap_or(""),
                q.limit.unwrap_or(10),
            ))
        }
        .await,
    )
}

async fn brief(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Query(q): Query<Selection>,
) -> MemberResponse {
    reply(
        async {
            Ok(host(&state)?.reader(&state, &member).await?.brief(
                q.space_id.as_deref(),
                q.topic.as_deref(),
                q.limit.unwrap_or(10),
            ))
        }
        .await,
    )
}

fn human(
    state: &TeamState,
    credential: &team::Credential,
    headers: &HeaderMap,
) -> Result<String, LibraryError> {
    if credential.kind() == team::CredentialKind::BrowserSession {
        if let Some(token) = crate::team_session::cookie_token(headers)
            .filter(|token| state.sessions.fresh(token, REVIEW_TTL))
        {
            return Ok(version::sha256_hex(token.as_bytes()));
        }
    }
    Err(LibraryError::Denied)
}

async fn preview(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    Json(draft): Json<TeamPublication>,
) -> MemberResponse {
    reply(
        async {
            let session = human(&state, &credential, &headers)?;
            let host = host(&state)?;
            host.migrate(&state).await?;
            host.preview(&member.id, &session, draft, Vec::new())
        }
        .await,
    )
}

async fn publish(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    Json(confirm): Json<TeamPublicationConfirm>,
) -> MemberResponse {
    reply(
        async {
            let session = human(&state, &credential, &headers)?;
            let host = host(&state)?;
            host.migrate(&state).await?;
            host.publish(&member.id, &session, &confirm.preview_id)
        }
        .await,
    )
}

async fn withdraw(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    let result = async {
        human(&state, &credential, &headers)?;
        let host = host(&state)?;
        host.migrate(&state).await?;
        host.library.withdraw(&member.id, &id)
    }
    .await;
    match result {
        Ok(()) => MemberResponse::status(StatusCode::NO_CONTENT),
        Err(error) => failure(error),
    }
}

async fn create_group(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    Json(group): Json<TeamGroupCreate>,
) -> MemberResponse {
    reply(
        async {
            human(&state, &credential, &headers)?;
            let host = host(&state)?;
            host.migrate(&state).await?;
            host.library
                .create_group(&member.id, &group.name, group.restricted)
        }
        .await,
    )
}

async fn set_grant(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Json(grant): Json<TeamLibraryGrant>,
) -> MemberResponse {
    if human(&state, &credential, &headers).is_err() || member.role != TeamRole::Owner {
        return failure(LibraryError::Denied);
    }
    match team::set_library_access(&state.team_path, &member.id, &id, grant.access) {
        Ok(()) => MemberResponse::status(StatusCode::NO_CONTENT),
        Err(_) => MemberResponse::problem(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The team member or grant is unavailable.",
        ),
    }
}

async fn issue_key(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    Json(key): Json<TeamToolKeyCreate>,
) -> MemberResponse {
    reply(
        async {
            human(&state, &credential, &headers)?;
            let host = host(&state)?;
            host.migrate(&state).await?;
            let library = host.library.reader(&member.id, key.space_id.as_deref())?;
            if key.space_id.is_none() && member.library_access().is_none() {
                return Err(LibraryError::Denied);
            }
            let scope = TeamToolScope {
                team_id: library.info().team.id,
                space_id: key.space_id,
            };
            let (id, secret) = team::issue_scoped_tool_key(
                &state.team_path,
                &member.id,
                &key.name,
                Some(scope.clone()),
            )?;
            Ok(TeamToolKeyReceipt {
                id,
                key: secret,
                scope,
            })
        }
        .await,
    )
}

async fn revoke_key(
    State(state): State<TeamState>,
    Extension(member): Extension<Member>,
    Extension(credential): Extension<team::Credential>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> MemberResponse {
    if let Err(error) = human(&state, &credential, &headers) {
        return failure(error);
    }
    match team::revoke_tool_key(&state.team_path, &member.id, &id) {
        Ok(()) => MemberResponse::status(StatusCode::NO_CONTENT),
        Err(_) => failure(LibraryError::Absent),
    }
}

pub(super) async fn serve_mcp(
    state: &TeamState,
    member: &Member,
    credential: &team::Credential,
    space: Option<&str>,
    req: Request,
) -> MemberResponse {
    let key_id = match credential {
        team::Credential::ToolKey { id } => Some(id),
        _ => None,
    };
    let bound = key_id
        .and_then(|id| member.keys.iter().find(|key| &key.id == id))
        .and_then(|key| key.scope.as_ref());
    let get_scope = async {
        let host = host(state)?;
        let reader = host.reader(state, member).await?;
        let pin = if let Some(pin) = bound.and_then(|binding| binding.space_id.as_deref()) {
            host.library.reader(&member.id, Some(pin))?;
            if space.is_some_and(|space| reader.space(space).as_deref() != Some(pin)) {
                return Err(LibraryError::Absent);
            }
            Some(pin.into())
        } else {
            match space {
                Some(space) => Some(reader.space(space).ok_or(LibraryError::Absent)?),
                None if member.library_access().is_some() => None,
                None => {
                    let info = reader.info();
                    let [group] = info.spaces.as_slice() else {
                        return Err(LibraryError::Denied);
                    };
                    Some(group.id.clone())
                }
            }
        };
        let rules = pin.as_deref().and_then(|pin| {
            let name = reader.migrated_space(pin)?;
            let (folder, access) = super::member_space_folder(state, member, name).ok()?;
            (team::hosted_space_id(&folder).as_deref() == Some(pin))
                .then(|| (name.to_owned(), access))
        });
        Ok::<_, LibraryError>((host, pin, rules))
    };
    let (host, pin, rules) = match get_scope.await {
        Ok(scope) => scope,
        Err(error) => return failure(error),
    };
    let service = {
        let mut services = host.mcp.lock().unwrap_or_else(|error| error.into_inner());
        let key = McpSessionKey {
            member: member.id.clone(),
            pin: pin.clone(),
            tool_key: key_id.cloned(),
            rules: rules.clone(),
        };
        match services.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                use wenlan_mcp::serve::MigratedSpaceRules;
                use wenlan_mcp::tools::ToolProfile;
                let rules = if let Some((space, access)) = rules {
                    let daemon_client = match super::team_mcp_client(state) {
                        Ok(client) => client,
                        Err(_) => {
                            return crate::member_view::team::empty(
                                StatusCode::SERVICE_UNAVAILABLE,
                            );
                        }
                    };
                    Some(MigratedSpaceRules {
                        daemon_client,
                        space,
                        profile: match access {
                            TeamAccess::Read => ToolProfile::QueryOnly,
                            TeamAccess::Write => ToolProfile::WebWrite,
                        },
                    })
                } else {
                    None
                };
                let library = host.library.clone();
                let actor = member.id.clone();
                let scoped = pin.is_some();
                let lookup = Arc::new(move |call| {
                    let library = library.clone();
                    let actor = actor.clone();
                    let pin = pin.clone();
                    Box::pin(async move {
                        tokio::task::spawn_blocking(move || {
                            read_tool(&library, &actor, pin.as_deref(), call)
                        })
                        .await
                        .map_err(|_| "The team library is unavailable.")?
                    }) as tools::ReadFuture
                });
                entry
                    .insert(wenlan_mcp::serve::team_library_service(
                        member.id.clone(),
                        scoped,
                        lookup,
                        rules,
                    ))
                    .clone()
            }
        }
    };
    crate::member_view::team::hosted_mcp(service, req).await
}

fn read_tool(
    library: &Library,
    actor: &str,
    pin: Option<&str>,
    call: tools::Read,
) -> Result<tools::Output, &'static str> {
    fn closed(error: LibraryError) -> &'static str {
        match error {
            LibraryError::Absent | LibraryError::Denied => "Team item not found.",
            _ => "The team library is unavailable.",
        }
    }
    let reader = library.reader(actor, pin).map_err(closed)?;
    let scope = |space: Option<String>| {
        if pin.is_some() {
            return Ok(None);
        }
        space
            .map(|space| reader.space(&space).ok_or("Team Space not found."))
            .transpose()
    };
    Ok(match call {
        tools::Read::Page(id) => tools::Output::Page(Box::new(reader.page(&id).map_err(closed)?)),
        tools::Read::Sources(id) => tools::Output::Sources(reader.sources(&id).map_err(closed)?),
        tools::Read::Pages {
            space,
            limit,
            offset,
        } => tools::Output::Pages(reader.pages(scope(space)?.as_deref(), limit, offset)),
        tools::Read::Recall {
            space,
            query,
            limit,
        } => tools::Output::Recall(reader.recall(scope(space)?.as_deref(), &query, limit)),
        tools::Read::Brief { space, topic } => tools::Output::Brief(Box::new(reader.brief(
            scope(space)?.as_deref(),
            topic.as_deref(),
            10,
        ))),
    })
}

pub(super) async fn legacy_inventory(
    host: &Host,
    state: &TeamState,
    member: &Member,
    space: &str,
    folder: &Path,
) -> MemberResponse {
    let result = async {
        let reader = host.reader(state, member).await?;
        let id = reader.space(space).ok_or(LibraryError::Absent)?;
        let mut list = wenlan_core::rulesync_team::hosted_file_list(
            &state.team_path,
            &state.settings_path,
            space,
            folder,
            wenlan_types::team::TeamAccess::Read,
            &team::SharedFolderPages::default(),
        )?;
        list.files
            .extend(reader.legacy_paths(&id).map(str::to_owned));
        list.deleted_pages = reader.deleted_legacy_pages(&id);
        Ok(list)
    }
    .await;
    match result {
        Ok(list) => member_view::team::files(list),
        Err(error) => failure(error),
    }
}

pub(super) async fn legacy_download(
    host: &Host,
    state: &TeamState,
    member: &Member,
    space: &str,
    path: &str,
) -> MemberResponse {
    let result = async {
        let reader = host.reader(state, member).await?;
        let id = reader.space(space).ok_or(LibraryError::Absent)?;
        reader.legacy_file(&id, path).ok_or(LibraryError::Absent)
    }
    .await;
    match result {
        Ok(file) => member_view::team::markdown_download(file.into_bytes()),
        Err(error) => failure(error),
    }
}

/// The server-side counterpart of the owner's terminal approval. The same
/// person-key gate as other terminal writes authenticates this local path;
/// the public team listener never forwards it.
pub(super) async fn terminal(
    State(shared): State<crate::state::SharedState>,
    headers: HeaderMap,
    Json(action): Json<TeamTerminalAction>,
) -> Response {
    use std::sync::OnceLock;
    static HOST: OnceLock<Host> = OnceLock::new();

    if !matches!(
        crate::security::bind_scope_from_env(),
        crate::security::BindScope::Unset | crate::security::BindScope::Loopback
    ) || !crate::memory_routes::caller_is_person(&headers)
    {
        return MemberResponse::problem(
            StatusCode::FORBIDDEN,
            "Use the team host's own terminal to approve this publication.",
        )
        .into_response();
    }
    let team_path = team::team_path();
    let host =
        HOST.get_or_init(|| Host::new(&team_path, wenlan_core::team_library::memories_enabled()));
    let db = shared.read().await.db.clone();
    let Some(db) = db else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let state = TeamState {
        team_path,
        db: Some(db.clone()),
        settings_path: wenlan_core::shared_space::settings_path(),
        library: None,
        daemon_url: String::new(),
        #[cfg(test)]
        in_process_daemon: { None },
        mcp: Arc::default(),
        sessions: Arc::default(),
        auth: Arc::default(),
        device: Arc::default(),
        cloud: None,
    };
    let result = terminal_action(host, &state, &db, action).await;
    let mut response = match result {
        Ok(result) => Json(result).into_response(),
        Err(error) => failure(error).into_response(),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) async fn terminal_action(
    host: &Host,
    state: &TeamState,
    db: &wenlan_core::db::MemoryDB,
    action: TeamTerminalAction,
) -> Result<TeamTerminalReply, LibraryError> {
    const SESSION: &str = "local-terminal";
    let owner = host.library.owner()?;
    host.migrate(state).await?;
    match action {
        TeamTerminalAction::Preview { draft, originals } => {
            if originals.len() != draft.memories.len() + 1
                || originals
                    .first()
                    .is_none_or(|original| original.kind != draft.kind)
                || originals
                    .iter()
                    .skip(1)
                    .any(|source| source.kind != TeamItemKind::Memory)
            {
                return Err(LibraryError::Invalid(
                    "Review the original item and selected personal sources.",
                ));
            }
            check_originals(db, &originals).await?;
            let preview = host.preview(&owner, SESSION, draft, originals)?;
            Ok(TeamTerminalReply::Preview {
                preview: Box::new(preview),
            })
        }
        TeamTerminalAction::Publish { preview_id } => {
            let originals = host
                .previews
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&preview_id)
                .filter(|entry| entry.member == owner && entry.session == SESSION)
                .map(|entry| entry.originals.clone())
                .unwrap_or_default();
            check_originals(db, &originals).await?;
            let receipt = host.publish(&owner, SESSION, &preview_id)?;
            if !originals.is_empty() {
                if let Err(error) =
                    save_local_receipt(&state.team_path, &preview_id, &originals, &receipt)
                {
                    tracing::warn!(%error, "Publication succeeded but its personal link could not be saved");
                }
            }
            Ok(TeamTerminalReply::Published { receipt })
        }
        TeamTerminalAction::Withdraw { item_id } => {
            host.library.withdraw(&owner, &item_id)?;
            Ok(TeamTerminalReply::Done)
        }
        TeamTerminalAction::CreateSpace { name, restricted } => {
            let space = host.library.create_group(&owner, &name, restricted)?;
            Ok(TeamTerminalReply::Space { space })
        }
        TeamTerminalAction::SetGrant { member_id, access } => {
            team::set_library_access(&state.team_path, &owner, &member_id, access)?;
            Ok(TeamTerminalReply::Done)
        }
    }
}

async fn check_originals(
    db: &wenlan_core::db::MemoryDB,
    originals: &[TeamOriginal],
) -> Result<(), LibraryError> {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    for original in originals {
        if !seen.insert(&original.id) {
            return Err(LibraryError::Invalid(
                "A personal source was selected more than once.",
            ));
        }
        let revision = match original.kind {
            TeamItemKind::Page => db
                .get_page(&original.id)
                .await?
                .filter(|page| page.status == "active")
                .map(|page| page.version),
            TeamItemKind::Memory => db
                .get_memory_detail(&original.id)
                .await?
                .filter(|memory| memory.supersede_mode != "evicted" && !memory.pending_revision)
                .map(|memory| memory.version),
        };
        if revision != Some(original.revision) {
            return Err(LibraryError::Conflict(
                "A personal original changed. Prepare and review a new publication.",
            ));
        }
    }
    Ok(())
}

fn save_local_receipt(
    team_path: &Path,
    id: &str,
    originals: &[TeamOriginal],
    receipt: &TeamPublicationReceipt,
) -> Result<(), LibraryError> {
    let root = team_path
        .parent()
        .ok_or(LibraryError::Absent)?
        .join("personal-publications");
    std::fs::create_dir_all(&root)?;
    let file = tempfile::NamedTempFile::new_in(&root)?;
    serde_json::to_writer(file.as_file(), &(originals, receipt))
        .map_err(wenlan_core::error::WenlanError::from)?;
    file.as_file().sync_all()?;
    file.persist(root.join(format!("{id}.json")))
        .map_err(|error| error.error)?;
    Ok(())
}
