// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA2: &str = "89abcdef0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Fake {
    routes: HashMap<String, (u16, Vec<u8>)>,
    seen: Mutex<Vec<String>>,
}

impl Fake {
    fn with(mut self, url: &str, status: u16, body: impl Into<Vec<u8>>) -> Self {
        self.routes.insert(url.to_string(), (status, body.into()));
        self
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

impl Fetch for Fake {
    fn get<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<HttpResponse, String>> {
        self.seen.lock().unwrap().push(url.to_string());
        let found = self.routes.get(url).cloned();
        Box::pin(async move {
            match found {
                Some((status, body)) => Ok(HttpResponse { status, body }),
                None => Err(format!("no route for {url}")),
            }
        })
    }
}

fn ep() -> Endpoints {
    Endpoints {
        skills_sh: "https://s".into(),
        wenlan_site: "https://w".into(),
        github_api: "https://api".into(),
        github_raw: "https://raw".into(),
    }
}

const SKILL_MD: &str = "---\nname: Meeting Notes\ndescription: Turn transcripts into decisions.\n---\n\n1. Read it all.\n";

fn tree(entries: &[(&str, &str, &str, u64)]) -> Vec<u8> {
    let list: Vec<serde_json::Value> = entries
        .iter()
        .map(|(path, kind, mode, size)| {
            serde_json::json!({"path": path, "type": kind, "mode": mode, "size": size})
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({"tree": list, "truncated": false})).unwrap()
}

fn repo_fake(commit: &str, skill_md: &str) -> Fake {
    Fake::default()
        .with(
            &format!("https://api/repos/o/r/git/trees/{commit}?recursive=1"),
            200,
            tree(&[
                ("README.md", "blob", "100644", 5),
                ("skills", "tree", "040000", 0),
                ("skills/meeting-notes", "tree", "040000", 0),
                ("skills/meeting-notes/SKILL.md", "blob", "100644", 60),
                ("skills/meeting-notes/scripts/run.sh", "blob", "100755", 20),
                ("skills/other/SKILL.md", "blob", "100644", 10),
            ]),
        )
        .with(
            &format!("https://raw/o/r/{commit}/skills/meeting-notes/SKILL.md"),
            200,
            skill_md,
        )
        .with(
            &format!("https://raw/o/r/{commit}/skills/meeting-notes/scripts/run.sh"),
            200,
            "#!/bin/sh\necho hi\n",
        )
}

#[tokio::test]
async fn search_uses_skills_sh_first() {
    let fake = Fake::default().with(
        "https://s/api/search?q=meeting%20notes&limit=30",
        200,
        r#"{"skills":[{"id":"o/r/meeting-notes","skillId":"meeting-notes","name":"meeting-notes","installs":12,"source":"o/r"},{"skillId":"../x","source":"o/r"},{"skillId":"y","source":"bad source"}]}"#,
    );
    let (hits, via) = search(&fake, &ep(), " meeting notes ").await.unwrap();
    assert_eq!(via, SearchVia::SkillsSh);
    assert_eq!(
        hits,
        vec![SkillHit {
            source: "o/r".into(),
            skill_id: "meeting-notes".into(),
            name: "meeting-notes".into(),
            installs: 12
        }]
    );
    assert_eq!(fake.seen().len(), 1);
}

#[tokio::test]
async fn search_falls_back_to_wenlan_app() {
    let fake = Fake::default()
        .with("https://s/api/search?q=notes&limit=30", 401, "{}")
        .with(
            "https://w/api/skills/search?q=notes&limit=30",
            200,
            r#"{"query":"notes","skills":[{"id":"o/r/n","slug":"n","name":"N","source":"o/r","installs":3}]}"#,
        );
    let (hits, via) = search(&fake, &ep(), "notes").await.unwrap();
    assert_eq!(via, SearchVia::WenlanApp);
    assert_eq!(hits[0].skill_id, "n");

    // Unreadable direct answer also falls back.
    let fake = Fake::default()
        .with("https://s/api/search?q=notes&limit=30", 200, "<html>")
        .with("https://w/api/skills/search?q=notes&limit=30", 503, "{}");
    assert_eq!(
        search(&fake, &ep(), "notes").await,
        Err(RegistryError::SearchUnavailable)
    );
}

#[tokio::test]
async fn short_query_calls_nothing() {
    let fake = Fake::default();
    let (hits, _) = search(&fake, &ep(), "a").await.unwrap();
    assert!(hits.is_empty());
    assert!(fake.seen().is_empty());
}

#[tokio::test]
async fn safety_verdicts() {
    let url = "https://s/api/v1/skills/audit/o/r/n";
    let pass = r#"{"audits":[{"provider":"Snyk","status":"pass","summary":"ok"},{"provider":"Socket","status":"pass","summary":"ok"}]}"#;
    let warn = r#"{"audits":[{"provider":"Snyk","status":"warn","summary":"1 issue"},{"provider":"Socket","status":"pass"}]}"#;
    let fail =
        r#"{"audits":[{"provider":"Snyk","status":"fail"},{"provider":"Socket","status":"warn"}]}"#;
    for (body, status, want) in [
        (pass, 200, Verdict::Pass),
        (warn, 200, Verdict::Warn),
        (fail, 200, Verdict::Fail),
        ("{}", 404, Verdict::Warn),
        (r#"{"audits":[]}"#, 200, Verdict::Warn),
    ] {
        let fake = Fake::default().with(url, status, body);
        assert_eq!(
            safety(&fake, &ep(), "o/r", "n").await.unwrap().verdict,
            want,
            "{body}"
        );
    }
}

#[tokio::test]
async fn fetch_finds_same_named_folder_and_pins_commit() {
    let fake = repo_fake(SHA, SKILL_MD);
    let skill = fetch_skill(&fake, &ep(), "o/r", "meeting-notes", SHA, None)
        .await
        .unwrap();
    assert_eq!(skill.repo_path, "skills/meeting-notes");
    let mut paths: Vec<&str> = skill.files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["SKILL.md", "scripts/run.sh"]);
    assert!(fake.seen().iter().all(|u| !u.contains("/main/")));
    assert_eq!(
        runnable_files(&skill.files),
        vec!["scripts/run.sh".to_string()]
    );
}

#[tokio::test]
async fn fetch_matches_front_matter_name_when_folder_differs() {
    let fake = Fake::default()
        .with(
            &format!("https://api/repos/o/r/git/trees/{SHA}?recursive=1"),
            200,
            tree(&[
                ("SKILL.md", "blob", "100644", 40),
                ("docs/SKILL.md", "blob", "100644", 40),
            ]),
        )
        .with(
            &format!("https://raw/o/r/{SHA}/SKILL.md"),
            200,
            "---\nname: weread\n---\nbody\n",
        )
        .with(
            &format!("https://raw/o/r/{SHA}/docs/SKILL.md"),
            200,
            "---\nname: other\n---\n",
        );
    let skill = fetch_skill(&fake, &ep(), "o/r", "weread", SHA, None).await;
    // The root skill owns the whole tree, including docs/ — fine, it is one folder.
    let skill = skill.unwrap();
    assert_eq!(skill.repo_path, "");
}

#[tokio::test]
async fn fetch_refuses_links_and_bad_commits() {
    let fake = Fake::default().with(
        &format!("https://api/repos/o/r/git/trees/{SHA}?recursive=1"),
        200,
        tree(&[
            ("s/SKILL.md", "blob", "100644", 10),
            ("s/evil", "blob", "120000", 10),
        ]),
    );
    assert!(matches!(
        fetch_skill(&fake, &ep(), "o/r", "s", SHA, None).await,
        Err(RegistryError::Refused(_))
    ));
    assert!(matches!(
        fetch_skill(&fake, &ep(), "o/r", "s", "main", None).await,
        Err(RegistryError::Refused(_))
    ));
    assert!(matches!(
        fetch_skill(&fake, &ep(), "../x", "s", SHA, None).await,
        Err(RegistryError::Refused(_))
    ));
}

#[test]
fn compose_adds_space_line_and_additions() {
    let out = compose_skill_md(
        SKILL_MD,
        &Scope::Space {
            name: "Clinic".into(),
        },
        "  Always cite the chart number.  \n",
    );
    assert!(out.starts_with(
        "---\nname: Meeting Notes\ndescription: Turn transcripts into decisions.\n---\n"
    ));
    assert!(out.contains("Wenlan Space \"Clinic\""));
    assert!(out.ends_with("## My additions\n\nAlways cite the chart number.\n"));
    assert_eq!(compose_skill_md(SKILL_MD, &Scope::All, "  "), SKILL_MD);
    assert_eq!(compose_skill_md("plain", &Scope::All, ""), "plain");
}

#[test]
fn folder_names_are_tool_safe() {
    assert_eq!(folder_name(SKILL_MD, "x").as_deref(), Some("meeting-notes"));
    assert_eq!(
        folder_name("no front matter", "My_Skill!").as_deref(),
        Some("my-skill")
    );
    assert_eq!(folder_name("---\nname: \"!!!\"\n---\n", "???"), None);
}

#[tokio::test]
async fn store_deliver_update_keeps_additions_and_respects_own_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let tool_a = tmp.path().join("claude/skills");
    let tool_b = tmp.path().join("agents/skills");
    // The person's own folder with the same name in tool B.
    std::fs::create_dir_all(tool_b.join("meeting-notes")).unwrap();
    std::fs::write(tool_b.join("meeting-notes/SKILL.md"), "mine").unwrap();

    let fake = repo_fake(SHA, SKILL_MD);
    let fetched = fetch_skill(&fake, &ep(), "o/r", "meeting-notes", SHA, None)
        .await
        .unwrap();
    let rec = store_skill(&store, &fetched, Verdict::Pass, Scope::All, 100).unwrap();
    assert_eq!(rec.name, "meeting-notes");
    assert_eq!(
        rec.description.as_deref(),
        Some("Turn transcripts into decisions.")
    );
    set_additions(&store, "meeting-notes", "Use our template.").unwrap();

    let (written, skipped) =
        deliver(&store, "meeting-notes", &[tool_a.clone(), tool_b.clone()]).unwrap();
    assert_eq!(written, vec![tool_a.join("meeting-notes")]);
    assert_eq!(skipped, vec![tool_b.join("meeting-notes")]);
    let copy = std::fs::read_to_string(tool_a.join("meeting-notes/SKILL.md")).unwrap();
    assert!(copy.contains("## My additions\n\nUse our template."));
    assert!(tool_a.join("meeting-notes").join(MANAGED_MARKER).is_file());
    assert_eq!(
        std::fs::read_to_string(tool_b.join("meeting-notes/SKILL.md")).unwrap(),
        "mine"
    );
    // The author's layer is untouched.
    assert_eq!(
        std::fs::read_to_string(store.join("skills/meeting-notes/.wenlan/author/SKILL.md"))
            .unwrap(),
        SKILL_MD
    );
    // The ready copy in the Wenlan folder matches what the tools got.
    assert_eq!(
        std::fs::read_to_string(store.join("skills/meeting-notes/SKILL.md")).unwrap(),
        copy
    );
    assert!(!tool_a.join("meeting-notes/.wenlan").exists());

    // Author publishes a new version that passes: it replaces author/, keeps additions.
    let new_md = SKILL_MD.replace("Read it all.", "Read it all twice.");
    let fake = repo_fake(SHA2, &new_md)
        .with(
            "https://api/repos/o/r/commits/HEAD",
            200,
            format!(r#"{{"sha":"{SHA2}"}}"#),
        )
        .with(
            "https://s/api/v1/skills/audit/o/r/meeting-notes",
            200,
            r#"{"audits":[{"provider":"Snyk","status":"pass"}]}"#,
        );
    let out = refresh_one(&fake, &ep(), &store, "meeting-notes", 200)
        .await
        .unwrap();
    assert_eq!(out.outcome, "updated");
    let rec = read_record(&store, "meeting-notes").unwrap();
    assert_eq!(
        (rec.commit.as_str(), rec.installed_at, rec.updated_at),
        (SHA2, 100, 200)
    );
    assert!(
        std::fs::read_to_string(store.join("skills/meeting-notes/.wenlan/author/SKILL.md"))
            .unwrap()
            .contains("twice")
    );
    assert_eq!(read_additions(&store, "meeting-notes"), "Use our template.");

    // A new version with a warning is left for the person.
    let fake = repo_fake(SHA, SKILL_MD)
        .with(
            "https://api/repos/o/r/commits/HEAD",
            200,
            format!(r#"{{"sha":"{SHA}"}}"#),
        )
        .with(
            "https://s/api/v1/skills/audit/o/r/meeting-notes",
            200,
            r#"{"audits":[{"provider":"Snyk","status":"warn"}]}"#,
        );
    let out = refresh_one(&fake, &ep(), &store, "meeting-notes", 300)
        .await
        .unwrap();
    assert_eq!(out.outcome, "needs_look");
    assert_eq!(read_record(&store, "meeting-notes").unwrap().commit, SHA2);

    // Failing skills are never stored.
    assert_eq!(
        store_skill(&store, &fetched, Verdict::Fail, Scope::All, 1),
        Err(RegistryError::FailedSafetyCheck)
    );

    remove(&store, "meeting-notes", &[tool_a.clone(), tool_b.clone()]).unwrap();
    assert!(!tool_a.join("meeting-notes").exists());
    assert!(tool_b.join("meeting-notes/SKILL.md").exists());
    assert!(list_installed(&store).is_empty());
}

#[test]
fn store_refuses_a_different_skill_with_the_same_name() {
    let tmp = tempfile::tempdir().unwrap();
    let skill = |source: &str| FetchedSkill {
        source: source.into(),
        skill_id: "meeting-notes".into(),
        commit: SHA.into(),
        repo_path: "".into(),
        files: vec![SkillFile {
            path: "SKILL.md".into(),
            bytes: SKILL_MD.into(),
        }],
        hash: "h".into(),
    };
    store_skill(tmp.path(), &skill("a/b"), Verdict::Pass, Scope::All, 1).unwrap();
    assert!(matches!(
        store_skill(tmp.path(), &skill("c/d"), Verdict::Pass, Scope::All, 1),
        Err(RegistryError::Refused(_))
    ));
}

#[test]
fn one_space_lives_under_spaces_and_moves_with_its_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fetched = FetchedSkill {
        source: "o/r".into(),
        skill_id: "meeting-notes".into(),
        commit: SHA.into(),
        repo_path: "".into(),
        files: vec![
            SkillFile {
                path: "SKILL.md".into(),
                bytes: SKILL_MD.into(),
            },
            SkillFile {
                path: "ref/notes.md".into(),
                bytes: b"ref".to_vec(),
            },
        ],
        hash: "h".into(),
    };
    let clinic = Scope::Space {
        name: "診所".into(),
    };
    store_skill(root, &fetched, Verdict::Pass, clinic.clone(), 1).unwrap();
    let dir = root.join("spaces/診所/skills/meeting-notes");
    let ready = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
    assert!(ready.contains("Wenlan Space \"診所\""));
    assert_eq!(
        std::fs::read_to_string(dir.join("ref/notes.md")).unwrap(),
        "ref"
    );
    assert_eq!(list_installed(root).len(), 1);

    // Reinstalling into all Spaces moves the folder; additions travel with it.
    set_additions(root, "meeting-notes", "Mine.").unwrap();
    let moved = set_scope(root, "meeting-notes", Scope::All).unwrap();
    assert_eq!(moved.scope, Scope::All);
    assert!(!dir.exists());
    let ready = std::fs::read_to_string(root.join("skills/meeting-notes/SKILL.md")).unwrap();
    assert!(!ready.contains("Wenlan Space"));
    assert!(ready.ends_with("Mine.\n"));
    assert_eq!(read_additions(root, "meeting-notes"), "Mine.");

    // A folder Wenlan did not download is never taken over.
    std::fs::create_dir_all(root.join("skills/other")).unwrap();
    let mut other = fetched.clone();
    other.skill_id = "other".into();
    other.files[0].bytes = b"---\nname: other\n---\n".to_vec();
    assert!(matches!(
        store_skill(root, &other, Verdict::Pass, Scope::All, 1),
        Err(RegistryError::Refused(_))
    ));
}

/// Real network run: skills.sh search, safety, GitHub at a pinned commit,
/// store and deliver into temp folders. Run by hand:
/// `cargo test -p wenlan-core --lib skill_registry::tests::live_end_to_end -- --ignored --nocapture`
#[tokio::test]
#[ignore = "needs skills.sh and GitHub"]
async fn live_end_to_end() {
    let http = ReqwestFetch::new().unwrap();
    let endpoints = Endpoints::default();
    let (hits, via) = search(&http, &endpoints, "meeting notes").await.unwrap();
    println!("search via {via:?}: {} hits", hits.len());
    assert!(!hits.is_empty());
    let mut chosen = None;
    for hit in hits.iter().take(10) {
        let report = safety(&http, &endpoints, &hit.source, &hit.skill_id)
            .await
            .unwrap();
        println!(
            "  {}/{} installs={} verdict={:?} audits={}",
            hit.source,
            hit.skill_id,
            hit.installs,
            report.verdict,
            report.audits.len()
        );
        if chosen.is_none() && report.verdict == Verdict::Pass {
            chosen = Some((hit.clone(), report));
        }
    }
    let (hit, report) = chosen.expect("a passing skill in the first 10");
    let commit = head_commit(&http, &endpoints, &hit.source).await.unwrap();
    let fetched = fetch_skill(&http, &endpoints, &hit.source, &hit.skill_id, &commit, None)
        .await
        .unwrap();
    println!(
        "fetched {} files from {}@{} at {:?}",
        fetched.files.len(),
        hit.source,
        &commit[..8],
        fetched.repo_path
    );
    let tmp = tempfile::tempdir().unwrap();
    let rec = store_skill(tmp.path(), &fetched, report.verdict, Scope::All, 1).unwrap();
    let tool = tmp.path().join("tool/skills");
    let (written, _) = deliver(tmp.path(), &rec.name, std::slice::from_ref(&tool)).unwrap();
    assert_eq!(written, vec![tool.join(&rec.name)]);
    assert!(tool.join(&rec.name).join("SKILL.md").is_file());
    println!(
        "installed as {} ({} runnable files)",
        rec.name,
        runnable_files(&fetched.files).len()
    );
    let out = refresh_one(&http, &endpoints, tmp.path(), &rec.name, 2)
        .await
        .unwrap();
    assert_eq!(out.outcome, "unchanged");
}
