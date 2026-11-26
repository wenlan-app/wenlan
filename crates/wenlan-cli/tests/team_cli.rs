// SPDX-License-Identifier: Apache-2.0
//! `wenlan team`: argument parsing, and what each command asks a daemon for
//! and prints, against a scripted stub. The daemon's own behavior is covered
//! by wenlan-server tests; nothing here reaches a real daemon.

mod common;

use assert_cmd::Command;
use serde_json::json;

const SERVER: &str = "http://team.test:7879";
const LINK: &str = "http://team.test:7879/join#wlt_abc";

fn cli() -> Command {
    let mut command = Command::cargo_bin("wenlan").expect("wenlan binary built");
    command.env("WENLAN_NO_AUTOSTART", "1");
    command
}

#[test]
fn team_subcommands_help_exits_zero() {
    for args in [
        &["team", "--help"][..],
        &["team", "status", "--help"][..],
        &["team", "start", "--help"][..],
        &["team", "stop", "--help"][..],
        &["team", "invite", "--help"][..],
        &["team", "leave", "--help"][..],
        // The name `invite` had before; scripts that use it keep working.
        &["team", "add", "--help"][..],
        &["team", "update", "--help"][..],
        &["team", "remove", "--help"][..],
        &["team", "new-key", "--help"][..],
        &["team", "host", "--help"][..],
        &["team", "join", "--help"][..],
    ] {
        cli().args(args).assert().success();
    }
}

#[test]
fn team_add_accepts_repeated_spaces_with_read_suffix() {
    // Parsing succeeds; the run then fails only because no daemon answers.
    let out = cli()
        .env("WENLAN_HOST", "http://127.0.0.1:1")
        .args([
            "team",
            "add",
            "Ana",
            "--access",
            "Work",
            "--access",
            "Notes:read",
            "--url",
            "https://wenlan.example.com",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&out.get_output().stderr).to_string();
    assert!(!stderr.contains("unexpected argument"), "{stderr}");
    assert!(!stderr.contains("invalid value"), "{stderr}");
}

#[test]
fn team_add_rejects_empty_space_name() {
    cli()
        .args(["team", "add", "Ana", "--access", ":read"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("the Space name is empty"));
}

#[test]
fn team_add_and_remove_require_their_arguments() {
    cli().args(["team", "add"]).assert().failure();
    cli().args(["team", "remove"]).assert().failure();
    cli().args(["team", "new-key"]).assert().failure();
    cli().args(["team", "host"]).assert().failure();
    cli().args(["team", "join"]).assert().failure();
}

#[test]
fn team_update_without_changes_is_refused_before_any_call() {
    cli()
        .args(["team", "update", "abc"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("nothing to change"));
}

#[test]
fn team_join_as_requires_space() {
    cli()
        .args(["team", "join", "https://x#wlt_a", "--as", "Local"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--server-space"));
}

fn member(id: &str, name: &str) -> serde_json::Value {
    json!({ "id": id, "name": name, "spaces": [], "added_at": "2026-11-15T00:00:00Z" })
}

fn team_status(members: Vec<serde_json::Value>) -> String {
    json!({
        "team": { "id": "t1", "name": "Acme" },
        "members": members,
        "spaces": ["Product"],
        "listening": "0.0.0.0:7879",
        "url": SERVER,
    })
    .to_string()
}

fn me(spaces: serde_json::Value) -> serde_json::Value {
    json!({
        "member_id": "m1",
        "name": "Amy",
        "spaces": spaces,
        "team": { "id": "t1", "name": "Acme" },
    })
}

fn joined_library() -> String {
    json!({
        "teams": [{
            "team": { "id": "t1", "name": "Acme" },
            "library": {
                "state": "ready",
                "spaces": [{
                    "space": { "id": "roadmap", "name": "Roadmap", "restricted": false },
                    "pages": [{
                        "id": "page-1",
                        "title": "Launch plan",
                        "space": "Roadmap",
                        "published_by": null,
                        "published_at": null
                    }]
                }]
            }
        }]
    })
    .to_string()
}

fn space(name: &str) -> serde_json::Value {
    json!({
        "id": name, "name": name, "description": null, "suggested": false,
        "starred": false, "is_default": false, "sort_order": 0,
        "memory_count": 0, "entity_count": 0, "created_at": 0.0, "updated_at": 0.0,
    })
}

fn share(target: &str) -> serde_json::Value {
    json!({
        "kind": "space", "target": target, "folder": "/tmp/f", "host": SERVER,
        "team": { "id": "t1", "name": "Acme" },
        "member_id": "m1", "member_name": "Amy", "members": [], "last": null,
    })
}

const COUNTS: &str = r#"{"exported":0,"applied":2,"conflicts":0,"skipped":0}"#;

fn run(host: &str, args: &[&str]) -> (bool, String, String) {
    let output = cli()
        .env("WENLAN_HOST", host)
        .args(["--format", "table"])
        .args(args)
        .output()
        .expect("run wenlan");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn served(requests: &std::sync::mpsc::Receiver<(String, String)>) -> Vec<(String, String)> {
    requests.try_iter().collect()
}

#[test]
fn start_sends_the_team_and_says_what_comes_next() {
    let (host, requests) = common::spawn_stub(vec![
        ("POST /api/team/start", team_status(vec![])),
        ("GET /api/team/firewall", "null".to_string()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "start", "Acme", "--as", "Qi"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Started the team Acme"), "{stdout}");
    assert!(
        stdout.contains(&format!("Invite links use: {SERVER}")),
        "{stdout}"
    );
    assert!(stdout.contains("wenlan team invite <NAME>"), "{stdout}");
    let sent = served(&requests);
    let body: serde_json::Value = serde_json::from_str(&sent[0].1).unwrap();
    assert_eq!(body, json!({ "name": "Acme", "person": "Qi" }));
}

#[test]
fn start_reports_a_blocking_windows_firewall() {
    let (host, _requests) = common::spawn_stub(vec![
        ("POST /api/team/start", team_status(vec![])),
        (
            "GET /api/team/firewall",
            json!({ "program": "wenlan-server.exe", "open": false, "public_network": true })
                .to_string(),
        ),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "start", "Acme", "--as", "Qi"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Windows Firewall"), "{stdout}");
}

#[test]
fn ux_cli_4_a_daemon_refusal_is_shown_as_its_own_sentence() {
    // The stub answers an unknown route with 404 {"error":"no such route"}.
    let (host, _requests) = common::spawn_stub(vec![("GET /nothing", String::new())]);
    let (ok, _stdout, stderr) = run(&host, &["team", "rename", "Acme"]);
    assert!(!ok);
    assert!(stderr.contains("no such route"), "{stderr}");
    assert!(!stderr.contains("daemon returned"), "{stderr}");
    assert!(!stderr.contains("/api/team"), "{stderr}");
}

#[test]
fn invite_says_which_spaces_the_person_gets() {
    let mut status: serde_json::Value = serde_json::from_str(&team_status(vec![])).unwrap();
    status["everyone"] = json!([{ "space": "Product", "access": "read" }]);
    let (host, requests) = common::spawn_stub(vec![
        ("GET /api/team", status.to_string()),
        (
            "POST /api/team/members",
            json!({ "member": member("m1", "Amy"), "key": "wlt_abc", "join_code": LINK })
                .to_string(),
        ),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "invite", "Amy"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains(&format!("Invite link: {LINK}")), "{stdout}");
    assert!(stdout.contains("They can use: Product (read)"), "{stdout}");
    // The id is never something to read or type.
    assert!(!stdout.contains("m1"), "{stdout}");
    assert_eq!(served(&requests).len(), 2);
}

#[test]
fn invite_refuses_a_name_already_on_the_team() {
    let (host, requests) = common::spawn_stub(vec![(
        "GET /api/team",
        team_status(vec![member("m1", "Amy")]),
    )]);
    let (ok, _stdout, stderr) = run(&host, &["team", "invite", "amy"]);
    assert!(!ok);
    assert!(stderr.contains("already on the team"), "{stderr}");
    assert!(stderr.contains("wenlan team new-key"), "{stderr}");
    // Nothing was added.
    assert_eq!(served(&requests).len(), 1);
}

#[test]
fn remove_takes_a_name_and_removes_that_member() {
    let (host, requests) = common::spawn_stub(vec![
        (
            "GET /api/team",
            team_status(vec![member("m1", "Amy"), member("m2", "Bo")]),
        ),
        (
            "DELETE /api/team/members/m2",
            member("m2", "Bo").to_string(),
        ),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "remove", "bo"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Removed Bo"), "{stdout}");
    assert_eq!(served(&requests)[1].0, "DELETE /api/team/members/m2");
}

#[test]
fn remove_never_picks_between_two_people_with_one_name() {
    let (host, requests) = common::spawn_stub(vec![(
        "GET /api/team",
        team_status(vec![member("m1", "Amy"), member("m2", "Amy")]),
    )]);
    let (ok, _stdout, stderr) = run(&host, &["team", "remove", "Amy"]);
    assert!(!ok);
    assert!(stderr.contains("2 people"), "{stderr}");
    assert!(stderr.contains("m1") && stderr.contains("m2"), "{stderr}");
    assert_eq!(served(&requests).len(), 1);
}

#[test]
fn join_keeps_the_membership_and_syncs_every_open_space() {
    let spaces = json!([
        { "space": "Product", "access": "write" },
        { "space": "Notes", "access": "read" },
    ]);
    let (host, requests) = common::spawn_stub(vec![
        (
            "POST /api/team/join",
            json!({ "url": SERVER, "me": me(spaces.clone()) }).to_string(),
        ),
        ("POST /api/team/membership", me(spaces).to_string()),
        // A Space of this person's own is already called Product here.
        ("GET /api/spaces", json!([space("Product")]).to_string()),
        ("GET /api/shared", json!({ "shares": [] }).to_string()),
        (
            "POST /api/spaces/Product%20(Acme)/shared/sync",
            COUNTS.to_string(),
        ),
        ("POST /api/spaces/Notes/shared/sync", COUNTS.to_string()),
        ("GET /api/team/joined/library", joined_library()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "join", LINK]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Joined Acme as Amy."), "{stdout}");
    // The team's copy goes beside the person's own Space, never into it.
    assert!(stdout.contains("here as \"Product (Acme)\""), "{stdout}");
    assert!(
        stdout.contains("Notes (read only): 2 pages taken"),
        "{stdout}"
    );
    assert!(stdout.contains("Roadmap: 1 pages"), "{stdout}");
    assert!(
        stdout.contains("Connect your AI tool too: `wenlan login"),
        "{stdout}"
    );
    let sent = served(&requests);
    assert_eq!(sent.len(), 7, "{sent:?}");
    assert_eq!(sent[1].0, "POST /api/team/membership");
    let sync: serde_json::Value = serde_json::from_str(&sent[4].1).unwrap();
    assert_eq!(sync["host"]["space"], "Product");
    assert_eq!(sync["host"]["code"], LINK);
}

#[test]
fn joining_again_reuses_the_space_that_already_syncs_with_the_team() {
    let spaces = json!([{ "space": "Product", "access": "write" }]);
    let (host, requests) = common::spawn_stub(vec![
        (
            "POST /api/team/join",
            json!({ "url": SERVER, "me": me(spaces.clone()) }).to_string(),
        ),
        ("POST /api/team/membership", me(spaces).to_string()),
        ("GET /api/spaces", json!([space("Product")]).to_string()),
        (
            "GET /api/shared",
            json!({ "shares": [share("Product")] }).to_string(),
        ),
        ("POST /api/spaces/Product/shared/sync", COUNTS.to_string()),
        ("GET /api/team/joined/library", joined_library()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "join", LINK]);
    assert!(ok, "{stderr}");
    assert!(!stdout.contains("here as"), "{stdout}");
    assert_eq!(served(&requests).len(), 6);
}

#[test]
fn join_list_joins_nothing() {
    let spaces = json!([{ "space": "Product", "access": "write" }]);
    let (host, requests) = common::spawn_stub(vec![(
        "POST /api/team/join",
        json!({ "url": SERVER, "me": me(spaces) }).to_string(),
    )]);
    let (ok, stdout, stderr) = run(&host, &["team", "join", LINK, "--list"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Product (read and write)"), "{stdout}");
    assert_eq!(served(&requests).len(), 1);
}

#[test]
fn leave_stops_every_space_of_the_team_then_forgets_it() {
    let (host, requests) = common::spawn_stub(vec![
        (
            "GET /api/shared",
            json!({ "shares": [share("Product")] }).to_string(),
        ),
        (
            "GET /api/team/membership",
            json!([{ "url": SERVER, "team": { "id": "t1", "name": "Acme" } }]).to_string(),
        ),
        (
            "DELETE /api/spaces/Product/shared",
            json!({ "success": true }).to_string(),
        ),
        (
            "DELETE /api/team/membership?url=http%3A%2F%2Fteam.test%3A7879",
            json!({ "left": true }).to_string(),
        ),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "leave"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Left Acme"), "{stdout}");
    assert!(
        stdout.contains("The pages already on this computer stay"),
        "{stdout}"
    );
    let sent = served(&requests);
    assert_eq!(sent.len(), 4, "{sent:?}");
    assert!(sent[3].0.starts_with("DELETE /api/team/membership?url="));
}

#[test]
fn leave_with_no_team_changes_nothing() {
    let (host, requests) = common::spawn_stub(vec![
        ("GET /api/shared", json!({ "shares": [] }).to_string()),
        ("GET /api/team/membership", "[]".to_string()),
    ]);
    let (ok, _stdout, stderr) = run(&host, &["team", "leave"]);
    assert!(!ok);
    assert!(stderr.contains("has not joined a team"), "{stderr}");
    assert_eq!(served(&requests).len(), 2);
}

#[test]
fn status_on_a_computer_with_no_team_says_how_to_get_one() {
    let (host, _requests) = common::spawn_stub(vec![
        (
            "GET /api/team",
            json!({ "members": [], "spaces": [] }).to_string(),
        ),
        ("GET /api/shared", json!({ "shares": [] }).to_string()),
        ("GET /api/team/membership", "[]".to_string()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "status"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("wenlan team start <NAME>"), "{stdout}");
    assert!(
        stdout.contains("wenlan team join <INVITE LINK>"),
        "{stdout}"
    );
    assert!(!stdout.contains("WENLAN_TEAM_ADDR"), "{stdout}");
}

#[test]
fn status_on_a_members_computer_shows_the_team_it_joined() {
    let (host, _requests) = common::spawn_stub(vec![
        (
            "GET /api/team",
            json!({ "members": [], "spaces": [] }).to_string(),
        ),
        (
            "GET /api/shared",
            json!({ "shares": [share("Product")] }).to_string(),
        ),
        // A daemon from before the list route: the Space still names its team.
        ("GET /nothing", String::new()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "status"]);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains(&format!("Joined: Acme ({SERVER})")),
        "{stdout}"
    );
    assert!(stdout.contains("  Product"), "{stdout}");
}

#[test]
fn status_names_the_owner_apart_from_the_people_invited() {
    let mut owner = member("o1", "Qi");
    owner["role"] = json!("owner");
    let (host, _requests) = common::spawn_stub(vec![
        ("GET /api/team", team_status(vec![owner.clone()])),
        ("GET /api/shared", json!({ "shares": [] }).to_string()),
        ("GET /api/team/membership", "[]".to_string()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "status"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Owner: Qi"), "{stdout}");
    assert!(stdout.contains("No one invited yet"), "{stdout}");

    let (host, _requests) = common::spawn_stub(vec![
        (
            "GET /api/team",
            team_status(vec![owner, member("m1", "Amy")]),
        ),
        ("GET /api/shared", json!({ "shares": [] }).to_string()),
        ("GET /api/team/membership", "[]".to_string()),
    ]);
    let (ok, stdout, stderr) = run(&host, &["team", "status"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("Owner: Qi"), "{stdout}");
    assert!(stdout.contains("Amy"), "{stdout}");
    assert!(!stdout.contains("\nQi "), "{stdout}");
}
