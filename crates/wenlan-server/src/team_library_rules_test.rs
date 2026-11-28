// SPDX-License-Identifier: Apache-2.0
//! The Rules pages of a team-library Space on the team host: made when the
//! Space is created, proposed and decided by the host's person, and
//! delivered only into the host's own bound checkouts.

use std::collections::BTreeMap;
use std::path::Path;

use super::*;
use crate::rulesync_checkout_local::tests::Fixture as Checkout;
use crate::rulesync_layers::tests::REMOTE;
use wenlan_core::team_library::LibraryError;
use wenlan_types::team::TeamRulesHostProposeRequest;
use wenlan_types::team_library::{TeamGroup, TeamTerminalAction, TeamTerminalReply};

const SPACE_TEXT: &str = "- Ship each Roadmap change behind a flag.";
const TEAM_TEXT: &str = "- Write the test before the fix.";

struct Host {
    f: Fixture,
    db: Arc<wenlan_core::db::MemoryDB>,
    owner: Member,
}

async fn host() -> Host {
    let (f, db) = fixture_without_embedder(HostedMode::Migrated).await;
    let owner = team::ensure_owner(&f.state.team_path, "Owner").unwrap();
    Host { f, db, owner }
}

impl Host {
    async fn create(&self, name: &str) -> Result<TeamTerminalReply, LibraryError> {
        let host = self.f.state.library.as_ref().unwrap().clone();
        library::terminal_action(
            &host,
            &self.f.state,
            &self.db,
            TeamTerminalAction::CreateSpace {
                name: name.into(),
                restricted: false,
            },
        )
        .await
    }

    async fn create_space(&self, name: &str) -> (TeamGroup, Option<String>) {
        match self.create(name).await.unwrap() {
            TeamTerminalReply::Space { space, rules_note } => (space, rules_note),
            other => panic!("expected a Space, got {other:?}"),
        }
    }

    fn groups(&self) -> Vec<TeamGroup> {
        let library = &self.f.state.library.as_ref().unwrap().library;
        library
            .reader(&library.owner().unwrap(), None)
            .unwrap()
            .info()
            .spaces
    }

    fn settings(&self) -> &Path {
        &self.f.state.settings_path
    }

    fn rules_dir(&self) -> PathBuf {
        let id = team::read_team(&self.f.state.team_path)
            .unwrap()
            .info()
            .unwrap()
            .id;
        self.f
            .state
            .team_path
            .parent()
            .unwrap()
            .join("teams")
            .join(id)
            .join("rules")
    }

    /// Binds the local Space `space` to a checkout of [`REMOTE`].
    fn bind(&self, spaces: &[&str]) {
        self.bind_to(spaces, REMOTE);
    }

    fn bind_to(&self, spaces: &[&str], remote: &str) {
        let bindings: Vec<_> = spaces
            .iter()
            .map(|space| {
                serde_json::json!({
                    "space": space,
                    "repo_root": self.f._dir.path().join("repo"),
                    "folder": "wiki",
                    "remote_url": remote,
                })
            })
            .collect();
        std::fs::write(
            self.settings().with_file_name("repo-wiki.json"),
            serde_json::to_vec(&serde_json::json!({ "bindings": bindings })).unwrap(),
        )
        .unwrap();
    }

    fn propose(&self, space: &str, markdown: &str) -> String {
        rules::propose_at(
            self.settings(),
            &TeamRulesHostProposeRequest {
                space: space.into(),
                markdown: markdown.into(),
            },
        )
        .unwrap()
    }

    fn checkout(&self) -> Checkout {
        Checkout::for_settings(self.settings(), self.owner.clone())
    }
}

fn files_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

fn tool_file(checkout: &Checkout) -> Option<String> {
    std::fs::read_to_string(checkout.repo.join("CLAUDE.local.md")).ok()
}

/// Design test 1: a new team-library Space gets both Rules pages; the
/// host's person proposes, lists and accepts, and the text reaches a real
/// tool file of a bound checkout together with the team-wide section.
#[tokio::test]
async fn team_library_space_rules_reach_a_bound_checkout() {
    let h = host().await;
    h.bind(&["Roadmap"]);
    let (space, note) = h.create_space("Roadmap").await;
    assert_eq!(note, None);
    for scope in [rulesync_team::SPACE, space.id.as_str()] {
        let dir = h.rules_dir().join(scope);
        assert!(
            !files_under(&dir).is_empty(),
            "{scope}: the Rules page is made"
        );
    }

    let space_change = h.propose("Roadmap", SPACE_TEXT);
    let team_change = h.propose(rulesync_team::SPACE, TEAM_TEXT);
    let pending = rules::proposals_at(h.settings()).unwrap();
    assert_eq!(pending.len(), 2, "{pending:?}");
    for (change, scope, text) in [
        (&space_change, "Roadmap", SPACE_TEXT),
        (&team_change, rulesync_team::SPACE, TEAM_TEXT),
    ] {
        let listed = pending
            .iter()
            .find(|listed| &listed.proposal.id == change)
            .unwrap();
        assert_eq!(listed.space, scope);
        assert_eq!(listed.proposal.markdown, text);
        assert_eq!(listed.author_name.as_deref(), Some("Owner"));
        assert!(!listed.proposal.proposed_at.is_empty());
    }

    for change in [&space_change, &team_change] {
        let decision = rules::decide_at(h.settings(), change, true).unwrap();
        assert!(decision.accepted);
        assert_eq!(decision.note, None, "{decision:?}");
    }
    assert!(rules::proposals_at(h.settings()).unwrap().is_empty());

    let mut checkout = h.checkout();
    assert_eq!(checkout.project.len(), 1);
    assert_eq!(checkout.team.len(), 1);
    checkout.run();
    let text = tool_file(&checkout).expect("the checkout's tool file is written");
    let team = text.find(TEAM_TEXT).expect("team-wide section");
    let project = text.find(SPACE_TEXT).expect("Space section");
    assert!(team < project, "{text}");
}

/// Design test 2: accepting the Rules of a Space no repository is bound to
/// says so, and no tool file is written.
#[tokio::test]
async fn team_library_unbound_space_accept_says_it_is_not_delivered() {
    let h = host().await;
    h.create_space("Roadmap").await;
    let change = h.propose("Roadmap", SPACE_TEXT);
    let decision = rules::decide_at(h.settings(), &change, true).unwrap();
    // The exact words the person reads.
    assert_eq!(
        decision.note.as_deref(),
        Some("accepted; not in any repository's tool files yet — bind a repository to this Space")
    );
    let team = h.propose(rulesync_team::SPACE, TEAM_TEXT);
    let decision = rules::decide_at(h.settings(), &team, true).unwrap();
    assert_eq!(
        decision.note.as_deref(),
        Some("accepted; not in any repository's tool files yet — the team-wide Rules reach a repository together with a Space bound to it")
    );
    let mut checkout = h.checkout();
    assert!(checkout.project.is_empty() && checkout.team.is_empty());
    checkout.run();
    assert_eq!(tool_file(&checkout), None);
}

/// Design test 3: creating the same Space again changes no Rules bytes.
#[tokio::test]
async fn team_library_create_space_twice_keeps_the_rules_bytes() {
    let h = host().await;
    h.create_space("Roadmap").await;
    let before = files_under(&h.rules_dir());
    assert!(!before.is_empty());
    h.create_space("Roadmap").await;
    assert_eq!(files_under(&h.rules_dir()), before);
}

/// Design test 4: a rejected change is recorded and the tool file keeps
/// the accepted text.
#[tokio::test]
async fn team_library_reject_keeps_the_tool_file() {
    let h = host().await;
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    let first = h.propose("Roadmap", SPACE_TEXT);
    rules::decide_at(h.settings(), &first, true).unwrap();
    let mut checkout = h.checkout();
    checkout.run();
    let before = tool_file(&checkout).unwrap();
    assert!(before.contains(SPACE_TEXT));

    let second = h.propose("Roadmap", "- Ship straight to main.");
    let decision = rules::decide_at(h.settings(), &second, false).unwrap();
    assert!(!decision.accepted);
    assert_eq!(decision.note, None);
    assert!(rules::proposals_at(h.settings()).unwrap().is_empty());
    assert!(matches!(
        rules::decide_at(h.settings(), &second, true),
        Err(wenlan_core::error::WenlanError::Conflict(_))
    ));
    checkout.reload();
    checkout.run();
    assert_eq!(tool_file(&checkout).unwrap(), before);
}

/// Design test 6: a hosted Space keeps its own folder's master. While it
/// has no repository it has no Rules page, and the library never makes one
/// for it; once bound, its master is the hosted folder's.
#[tokio::test]
async fn team_library_hosted_space_never_falls_back_to_the_library() {
    let h = host().await;
    // Creating any Space migrates the hosted Design share into a library
    // group with the same id.
    h.create_space("Roadmap").await;
    let design = h
        .groups()
        .into_iter()
        .find(|group| group.name == "Design")
        .expect("the hosted Design share is a library group");
    rulesync_team::ensure_library_masters(h.settings(), "Design").unwrap();
    assert!(!h.rules_dir().join(&design.id).exists());
    assert!(matches!(
        rules::propose_at(
            h.settings(),
            &TeamRulesHostProposeRequest {
                space: "Design".into(),
                markdown: SPACE_TEXT.into(),
            },
        ),
        Err(wenlan_core::error::WenlanError::NotFound(_))
    ));
    assert!(!h.rules_dir().join(&design.id).exists());

    h.bind(&["Design"]);
    let change = h.propose("Design", SPACE_TEXT);
    rules::decide_at(h.settings(), &change, true).unwrap();
    assert!(!h.rules_dir().join(&design.id).exists());
    let (_, approved) = rulesync_team::approved_hosted_rules(h.settings(), "Design")
        .unwrap()
        .expect("the hosted folder's master");
    assert_eq!(approved, SPACE_TEXT);
}

/// Design test 7: a migrated `everyone` group is never a project layer,
/// and the name is refused for a new Space.
#[tokio::test]
async fn team_library_everyone_is_never_a_space() {
    let (f, db) = fixture_without_embedder(HostedMode::Legacy).await;
    let settings = &f.state.settings_path;
    let scope = Scope::Space(rulesync_team::SPACE.into());
    let folder = team::new_hosted_folder(settings);
    shared_space::join_at(settings, &scope, &folder, "Server").unwrap();
    shared_space::update_share(settings, &scope, |share| share.hosted = true).unwrap();
    shared_space::ensure_space_file(
        &folder,
        &scope,
        rulesync_team::SPACE,
        &SpaceMember {
            id: "server".into(),
            name: "Server".into(),
        },
    )
    .unwrap();
    let mut f = f;
    f.set_hosted_mode(HostedMode::Migrated);
    let owner = team::ensure_owner(&f.state.team_path, "Owner").unwrap();
    let h = Host { f, db, owner };
    h.bind(&["Roadmap", rulesync_team::SPACE]);
    h.create_space("Roadmap").await;
    let groups = h.groups();
    assert!(
        groups
            .iter()
            .any(|group| group.name == rulesync_team::SPACE),
        "the hosted `everyone` share was migrated as a group: {groups:?}"
    );
    // Without the hosted `everyone` folder the team-wide page is the
    // library's own, and it has accepted text.
    shared_space::update_share(h.settings(), &scope, |share| share.hosted = false).unwrap();
    let change = h.propose("Roadmap", SPACE_TEXT);
    rules::decide_at(h.settings(), &change, true).unwrap();
    let change = h.propose(rulesync_team::SPACE, TEAM_TEXT);
    rules::decide_at(h.settings(), &change, true).unwrap();
    assert!(!files_under(&h.rules_dir().join(rulesync_team::SPACE)).is_empty());
    let team = rulesync_team::load_layers(h.settings()).unwrap();
    assert!(
        team.iter().any(|layer| layer.markdown.contains(TEAM_TEXT)),
        "{team:?}"
    );
    let layers = rulesync_team::load_project_layers(h.settings()).unwrap();
    assert!(!layers.is_empty());
    assert!(
        layers.iter().all(|layer| layer
            .project
            .as_ref()
            .is_some_and(|project| project.space != rulesync_team::SPACE)),
        "{layers:?}"
    );
    for name in ["everyone", "Everyone"] {
        assert!(
            matches!(h.create(name).await, Err(LibraryError::Invalid(_))),
            "{name}"
        );
    }
}

/// Design test 8: when the Rules pages cannot be made the Space is still
/// created and the reply says when the page will be made. Seeding never
/// depends on the tool-sync switch.
#[tokio::test]
async fn team_library_space_is_created_when_its_rules_cannot_be() {
    if !crate::person_key::person_key_test::in_isolated_child(
        "team_routes::tests::library_rules::team_library_space_is_created_when_its_rules_cannot_be",
        &[],
        tempfile::tempdir().unwrap().path(),
    ) {
        return;
    }
    assert!(!wenlan_core::rulesync_personal::enabled());
    let h = host().await;
    let (seeded, note) = h.create_space("Seeded").await;
    assert_eq!(note, None, "the switch is off and the pages are made");
    assert!(!files_under(&h.rules_dir().join(&seeded.id)).is_empty());

    std::fs::remove_dir_all(h.rules_dir()).unwrap();
    std::fs::write(h.rules_dir(), b"not a folder").unwrap();
    let (space, note) = h.create_space("Roadmap").await;
    assert_eq!(note.as_deref(), Some(library::RULES_LATER));
    let groups = h.groups();
    assert!(groups.contains(&space), "{groups:?}");
}

/// Design test 9: the team listener does not serve a library-only name,
/// even to the owner's own browser session.
#[tokio::test]
async fn team_library_rules_are_not_on_the_listener() {
    let h = host().await;
    team::set_url(&h.f.state.team_path, Some(ORIGIN)).unwrap();
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    h.propose("Roadmap", SPACE_TEXT);
    let session = h.f.state.sessions.create(&h.owner.id).unwrap();
    for uri in [
        "/team/v1/spaces/Roadmap/rules/proposals",
        "/team/v1/spaces/everyone/rules/proposals",
    ] {
        let response = browser(&h.f.state, Method::GET, uri, Some(&session), None, None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
    // Behind the listener's own hosted-folder gate, the review it calls
    // does not find library Rules either.
    assert!(matches!(
        rulesync_team::pending_proposals(
            &h.f.state.team_path,
            h.settings(),
            "Roadmap",
            &h.owner.id,
            team::CredentialKind::BrowserSession,
        ),
        Err(rulesync_team::ReviewError::NotConfigured)
    ));
}

/// Design test 10: an empty seed adds no section and does not blank a
/// tool file that already holds accepted Rules.
#[tokio::test]
async fn team_library_empty_seed_adds_no_section() {
    let h = host().await;
    h.bind(&["Roadmap", "Later"]);
    h.create_space("Roadmap").await;
    let change = h.propose("Roadmap", SPACE_TEXT);
    rules::decide_at(h.settings(), &change, true).unwrap();
    let mut checkout = h.checkout();
    assert_eq!(checkout.project.len(), 1);
    assert!(
        checkout.team.is_empty(),
        "the team-wide page is still empty"
    );
    checkout.run();
    let before = tool_file(&checkout).unwrap();
    assert!(before.contains(SPACE_TEXT));

    h.create_space("Later").await;
    checkout.reload();
    assert_eq!(checkout.project.len(), 1, "the empty seed is no layer");
    checkout.run();
    assert_eq!(tool_file(&checkout).unwrap(), before);
}

/// Accepting team-wide text while the only bound Space still has its empty
/// seed says the text reaches no repository yet, and no tool file changes.
#[tokio::test]
async fn team_library_team_wide_accept_beside_an_empty_space_is_not_delivered() {
    let h = host().await;
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    let mut checkout = h.checkout();
    checkout.run();
    let before = tool_file(&checkout);

    let change = h.propose(rulesync_team::SPACE, TEAM_TEXT);
    let decision = rules::decide_at(h.settings(), &change, true).unwrap();
    assert_eq!(
        decision.note.as_deref(),
        Some(rulesync_team::UNBOUND_TEAM_NOTE)
    );
    checkout.reload();
    checkout.run();
    assert_eq!(tool_file(&checkout), before);
    assert!(!before.unwrap_or_default().contains(TEAM_TEXT));
}

/// An unreadable team library does not stop a hosted Space's Rules from
/// reaching its checkout; only the host's own review refuses.
#[tokio::test]
async fn team_library_unreadable_library_keeps_hosted_rules_rendering() {
    let h = host().await;
    h.bind(&["Design"]);
    let change = h.propose("Design", SPACE_TEXT);
    rules::decide_at(h.settings(), &change, true).unwrap();
    let library = h.rules_dir().with_file_name("library.json");
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(&library, b"{ not json").unwrap();

    let mut checkout = h.checkout();
    assert_eq!(checkout.project.len(), 1, "{:?}", checkout.project);
    let layer = checkout.project[0].clone();
    rulesync_team::queued_proposal_path(h.settings(), &layer.id, &layer.revision, &"a".repeat(64))
        .expect("a tool-file proposal still finds its hosted Space");
    checkout.run();
    assert!(tool_file(&checkout).unwrap().contains(SPACE_TEXT));
    assert!(rules::proposals_at(h.settings()).is_err());
}

/// A checkout of another repository gets none of a library Space's Rules,
/// and neither the team-wide section.
#[tokio::test]
async fn team_library_rules_skip_a_checkout_of_another_repository() {
    let h = host().await;
    h.bind_to(&["Roadmap"], "https://example.com/other/elsewhere.git");
    h.create_space("Roadmap").await;
    for (space, text) in [("Roadmap", SPACE_TEXT), (rulesync_team::SPACE, TEAM_TEXT)] {
        let change = h.propose(space, text);
        let decision = rules::decide_at(h.settings(), &change, true).unwrap();
        assert_eq!(decision.note, None, "{decision:?}");
    }
    let mut checkout = h.checkout();
    assert!(checkout.project.is_empty() && checkout.team.is_empty());
    checkout.run();
    assert_eq!(tool_file(&checkout), None);
}

/// A proposal for a name that is neither a hosted nor a team-library Space
/// is refused with that name; a hosted Space without a repository is told
/// to bind one.
#[tokio::test]
async fn team_library_unknown_space_is_named_in_the_refusal() {
    let h = host().await;
    h.create_space("Roadmap").await;
    let error = rules::propose_at(h.settings(), &propose_request("Nowhere")).unwrap_err();
    match error {
        wenlan_core::error::WenlanError::NotFound(message) => {
            assert_eq!(message, "\"Nowhere\" is not a team Space.")
        }
        other => panic!("expected not found, got {other:?}"),
    }
    // A typed name that looks like a key is not repeated back.
    let typed = "api-key: w79wxza928af410e9c4f36xdpycb01h6";
    match rules::propose_at(h.settings(), &propose_request(typed)).unwrap_err() {
        wenlan_core::error::WenlanError::NotFound(message) => {
            assert_eq!(message, "That name is not a team Space.")
        }
        other => panic!("expected not found, got {other:?}"),
    }
    // A hosted Space no repository is bound to is a team Space; it waits.
    match rules::propose_at(h.settings(), &propose_request("Design")).unwrap_err() {
        wenlan_core::error::WenlanError::NotFound(message) => assert_eq!(
            message,
            "\"Design\" has no Rules page yet: bind a repository to it first."
        ),
        other => panic!("expected not found, got {other:?}"),
    }
}

fn propose_request(space: &str) -> TeamRulesHostProposeRequest {
    TeamRulesHostProposeRequest {
        space: space.into(),
        markdown: SPACE_TEXT.into(),
    }
}

/// Accepting on a bound Space updates the tool files at once; when they
/// were not updated the reply says so and why. Here tool sync is on but the
/// tool setup was never finished.
#[tokio::test]
async fn team_library_accept_says_when_the_tool_files_were_not_updated() {
    if !crate::person_key::person_key_test::in_isolated_child(
        "team_routes::tests::library_rules::team_library_accept_says_when_the_tool_files_were_not_updated",
        &[],
        tempfile::tempdir().unwrap().path(),
    ) {
        return;
    }
    // As on an installed Wenlan: the data folder exists and tool sync is on.
    std::fs::create_dir_all(wenlan_core::config::data_root()).unwrap();
    wenlan_core::rulesync_personal::set_enabled(true).unwrap();
    let h = host().await;
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    let change = h.propose("Roadmap", SPACE_TEXT);
    let decision =
        rules::decide_and_deliver(h.settings().to_path_buf(), change, true, Some(&h.db), None)
            .await
            .unwrap();
    assert!(decision.accepted);
    assert_eq!(
        decision.note.as_deref(),
        Some(
            "accepted; the repositories' tool files were not updated: the tool setup on this \
             computer is not finished; see `wenlan rules adopt --dry-run`"
        ),
        "{decision:?}"
    );

    let change = h.propose("Roadmap", TEAM_TEXT);
    let decision = rules::decide_and_deliver(h.settings().to_path_buf(), change, true, None, None)
        .await
        .unwrap();
    assert_eq!(
        decision.note.as_deref(),
        Some("accepted; the repositories' tool files were not updated: Wenlan's database is not open")
    );

    let change = h.propose("Roadmap", SPACE_TEXT);
    let decision =
        rules::decide_and_deliver(h.settings().to_path_buf(), change, false, Some(&h.db), None)
            .await
            .unwrap();
    assert!(!decision.accepted);
    assert_eq!(decision.note, None, "a rejection changes no tool file");
}

const KEY_LIKE: &str = "Password: reset-flow";

/// A Space name that looks like a password or key is refused, by the same
/// check as Rules text, and no Space or Rules page is made.
#[tokio::test]
async fn team_library_space_name_that_looks_like_a_key_is_refused() {
    let h = host().await;
    for name in [KEY_LIKE, "api-key: w79wxza928af410e9c4f36xdpycb01h6"] {
        match h.create(name).await {
            Err(LibraryError::Invalid(message)) => assert_eq!(
                message,
                "This name looks like a password or key; choose another Space name."
            ),
            other => panic!("{name}: expected a refusal, got {other:?}"),
        }
    }
    let before = h.groups();
    assert!(
        before.iter().all(|group| group.name == "Design"),
        "only the fixture's hosted Space: {before:?}"
    );
    assert!(!h.rules_dir().exists() || files_under(&h.rules_dir()).is_empty());
}

/// A Space made before that check with a key-like name is skipped: the
/// other Spaces' Rules are still listed, decided and delivered, and the
/// name never reaches a tool file.
#[tokio::test]
async fn team_library_space_with_a_key_like_name_is_skipped() {
    let h = host().await;
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    let (leak, _) = h.create_space("Leak").await;
    let library = h.rules_dir().with_file_name("library.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&library).unwrap()).unwrap();
    let renamed = manifest["groups"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .find(|group| group["id"] == leak.id.as_str())
        .unwrap();
    renamed["name"] = KEY_LIKE.into();
    std::fs::write(&library, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(h.groups().iter().any(|group| group.name == KEY_LIKE));

    let change = h.propose("Roadmap", SPACE_TEXT);
    let pending = rules::proposals_at(h.settings()).unwrap();
    assert_eq!(pending.len(), 1, "{pending:?}");
    let decision = rules::decide_at(h.settings(), &change, true).unwrap();
    assert_eq!(decision.note, None, "{decision:?}");
    let layers = rulesync_team::load_project_layers(h.settings()).unwrap();
    assert!(
        layers.iter().all(|layer| !layer.markdown.contains(KEY_LIKE)
            && layer
                .project
                .as_ref()
                .is_some_and(|project| project.space == "Roadmap")),
        "{layers:?}"
    );
    let mut checkout = h.checkout();
    assert_eq!(checkout.project.len(), 1);
    checkout.run();
    let text = tool_file(&checkout).unwrap();
    assert!(
        text.contains(SPACE_TEXT) && !text.contains(KEY_LIKE),
        "{text}"
    );
    // Proposing to it by that name is refused without repeating the name.
    let error = rules::propose_at(h.settings(), &propose_request(KEY_LIKE))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&leak.id) && !error.contains("reset-flow"),
        "{error}"
    );
}

/// After an accept whose tool files were updated, the reply says when no
/// checkout on this computer gets the Space's Rules, as its checkout
/// receipts tell; files that need the person point to `wenlan rules
/// status`; receipts that cannot be read give the plain reason.
#[tokio::test]
async fn team_library_accept_says_when_no_checkout_here_gets_the_rules() {
    if !crate::person_key::person_key_test::in_isolated_child(
        "team_routes::tests::library_rules::team_library_accept_says_when_no_checkout_here_gets_the_rules",
        &[],
        tempfile::tempdir().unwrap().path(),
    ) {
        return;
    }
    std::fs::create_dir_all(wenlan_core::config::data_root()).unwrap();
    let h = host().await;
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    for (space, text) in [("Roadmap", SPACE_TEXT), (rulesync_team::SPACE, TEAM_TEXT)] {
        let change = h.propose(space, text);
        let decision = rules::decide_at(h.settings(), &change, true).unwrap();
        assert_eq!(decision.note, None, "the Space is bound: {decision:?}");
    }
    let note = |tool_files, space: &str| {
        rules::delivery_note(tool_files, h.settings().to_path_buf(), space.to_string())
    };
    use crate::skill_grow::ToolFiles;
    const NO_CHECKOUT: &str = "accepted; this computer has no checkout of that repository yet";
    for space in ["Roadmap", rulesync_team::SPACE] {
        assert_eq!(
            note(ToolFiles::Updated, space).await.as_deref(),
            Some(NO_CHECKOUT),
            "{space}: no receipt at all"
        );
    }
    let setup = crate::rulesync_routes::Setup::system().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    crate::rulesync_checkout::record_checkout_for_test(&setup, &elsewhere.path().join("gone"));
    assert_eq!(
        note(ToolFiles::Updated, "Roadmap").await.as_deref(),
        Some(NO_CHECKOUT),
        "a receipt of no repository of this Space"
    );

    let unreadable = setup
        .data
        .join(format!("checkouts/{}.json", "0".repeat(64)));
    std::fs::write(&unreadable, b"not a receipt").unwrap();
    assert_eq!(
        note(ToolFiles::Updated, "Roadmap").await.as_deref(),
        Some(
            "accepted; the repositories' tool files were not updated: the checkout records on \
             this computer could not be read"
        )
    );
    std::fs::remove_file(&unreadable).unwrap();

    let checkout = h.checkout();
    crate::rulesync_checkout::record_checkout_for_test(&setup, &checkout.repo);
    for space in ["Roadmap", rulesync_team::SPACE] {
        assert_eq!(note(ToolFiles::Updated, space).await, None, "{space}");
    }
    assert_eq!(
        note(ToolFiles::NeedsAttention, "Roadmap").await.as_deref(),
        Some(
            "accepted; some tool files were not updated and need your attention: see \
             `wenlan rules status`"
        )
    );
}

/// The daemon's decide route itself reports the delivery: a person's accept
/// through HTTP gets the note about the tool files.
#[tokio::test]
async fn team_library_decide_route_reports_the_delivery() {
    if !crate::person_key::person_key_test::in_isolated_child(
        "team_routes::tests::library_rules::team_library_decide_route_reports_the_delivery",
        &[],
        tempfile::tempdir().unwrap().path(),
    ) {
        return;
    }
    let (f, db) = fixture_in_data_root(HostedMode::Migrated).await;
    let owner = team::ensure_owner(&f.state.team_path, "Owner").unwrap();
    let h = Host { f, db, owner };
    assert_eq!(h.settings(), shared_space::settings_path());
    wenlan_core::rulesync_personal::set_enabled(true).unwrap();
    h.bind(&["Roadmap"]);
    h.create_space("Roadmap").await;
    let change = h.propose("Roadmap", SPACE_TEXT);
    let app = crate::router::build_router(Arc::new(tokio::sync::RwLock::new(
        crate::state::ServerState {
            db: Some(h.db.clone()),
            ..Default::default()
        },
    )));
    use wenlan_types::requests::{CALLER_HEADER, PERSON_CALLER_VALUE, PERSON_KEY_HEADER};
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/team/rules/proposals/{change}/decision"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(CALLER_HEADER, PERSON_CALLER_VALUE)
        .header(PERSON_KEY_HEADER, crate::person_key::test_key())
        .body(Body::from(r#"{"accept":true}"#))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let decision: wenlan_types::team::TeamRulesHostDecision =
        serde_json::from_slice(&bytes).unwrap();
    assert!(decision.accepted);
    assert_eq!(
        decision.note.as_deref(),
        Some(
            "accepted; the repositories' tool files were not updated: the tool setup on this \
             computer is not finished; see `wenlan rules adopt --dry-run`"
        ),
        "{decision:?}"
    );
}

/// A hosted Space whose name looks like a key is skipped like a library
/// one: the other Spaces' Rules still load, list, decide and deliver.
#[tokio::test]
async fn team_library_hosted_space_with_a_key_like_name_is_skipped() {
    let h = host().await;
    let scope = Scope::Space(KEY_LIKE.into());
    let folder = team::new_hosted_folder(h.settings());
    shared_space::join_at(h.settings(), &scope, &folder, "Owner").unwrap();
    shared_space::update_share(h.settings(), &scope, |share| share.hosted = true).unwrap();
    shared_space::ensure_space_file(
        &folder,
        &scope,
        KEY_LIKE,
        &SpaceMember {
            id: h.owner.id.clone(),
            name: h.owner.name.clone(),
        },
    )
    .unwrap();
    h.bind(&["Roadmap", KEY_LIKE]);
    h.create_space("Roadmap").await;

    let change = h.propose("Roadmap", SPACE_TEXT);
    assert_eq!(rules::proposals_at(h.settings()).unwrap().len(), 1);
    let decision = rules::decide_at(h.settings(), &change, true).unwrap();
    assert_eq!(decision.note, None, "{decision:?}");
    let layers = rulesync_team::load_project_layers(h.settings()).unwrap();
    assert!(
        layers.iter().all(|layer| layer
            .project
            .as_ref()
            .is_some_and(|project| project.space == "Roadmap")),
        "{layers:?}"
    );
    let mut checkout = h.checkout();
    assert_eq!(checkout.project.len(), 1);
    checkout.run();
    let text = tool_file(&checkout).unwrap();
    assert!(
        text.contains(SPACE_TEXT) && !text.contains("reset-flow"),
        "{text}"
    );
}
