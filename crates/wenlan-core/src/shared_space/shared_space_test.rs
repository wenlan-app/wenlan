// SPDX-License-Identifier: Apache-2.0
use std::collections::HashMap;

use super::rewrite::{quote_label, rewrite_sources, RewriteContext, QUOTE_LOCATOR};
use super::version::{self, Author, AuthorKind, PageVersion, VersionMeta};
use super::*;
use crate::store::format::page::{Basis, Citation, Locator, PageFile};

fn meta(parent: Option<&str>, body: &str, member: &str, ts: &str) -> VersionMeta {
    VersionMeta {
        parent: parent.map(str::to_string),
        merged: Vec::new(),
        body_digest: version::body_digest(body),
        author: Author {
            member: member.to_string(),
            kind: AuthorKind::Person,
            agent: None,
        },
        ts: ts.to_string(),
        archived: false,
        locked: false,
        proposal: false,
        decides: None,
        device: None,
    }
}

fn page(id: &str, body: &str) -> PageFile {
    let mut file = PageFile::new("Seating plan", body);
    file.id = Some(id.to_string());
    file.created = Some("2026-11-08T10:00:00Z".to_string());
    file.modified = Some("2026-11-08T11:00:00Z".to_string());
    file
}

/// A version as the folder would hold it.
fn stored(id: &str, body: &str, meta: VersionMeta) -> PageVersion {
    let text = version::render(&page(id, body), &meta);
    let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
    version::parse(&name, text.as_bytes()).expect("a rendered version reads back")
}

#[test]
fn version_file_reads_back_as_written() {
    let mut file = page("page_1", "Table 1: Ana, Ben [1].\n\nTable 2: Cleo.");
    file.citations = vec![Citation {
        marker: 1,
        label: "[[Guests]]".to_string(),
        locator: Locator::Wenlan {
            id: "page_2".to_string(),
            version: 3,
        },
    }];
    file.basis = vec![Basis {
        label: "[[Guests]]".to_string(),
        locator: Locator::Wenlan {
            id: "page_2".to_string(),
            version: 3,
        },
    }];
    let mut meta = meta(
        Some(&"a".repeat(64)),
        &file.body,
        "m1",
        "2026-11-08T12:00:00.000Z",
    );
    meta.merged = vec!["b".repeat(64)];
    meta.author = Author {
        member: "m1".to_string(),
        kind: AuthorKind::Ai,
        agent: Some("claude-code".to_string()),
    };
    let text = version::render(&file, &meta);
    let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
    let read = version::parse(&name, text.as_bytes()).unwrap();
    assert_eq!(read.page, file);
    assert_eq!(read.meta, meta);
    assert_eq!(read.sha, name.trim_end_matches(".md"));
    // Today's page reader still reads it, with the sharing facts as one key.
    let plain = crate::store::format::page::parse(&text, "").unwrap();
    assert_eq!(plain.body, file.body);
    assert!(plain.other.contains_key("shared"));
}

#[test]
fn an_archive_and_a_rename_are_versions_of_their_own() {
    let live = stored(
        "page_1",
        "Table 1.",
        meta(None, "Table 1.", "m1", "2026-11-08T12:00:00.000Z"),
    );
    assert!(!live.meta.archived);
    let mut archived_meta = meta(
        Some(&live.sha),
        "Table 1.",
        "m1",
        "2026-11-08T12:01:00.000Z",
    );
    archived_meta.archived = true;
    let archived = stored("page_1", "Table 1.", archived_meta);
    assert!(archived.meta.archived);
    let text = version::render(&archived.page, &archived.meta);
    assert!(text.contains("archived: true"), "{text}");
    assert_eq!(live.meta.body_digest, archived.meta.body_digest);
    assert_ne!(live.state(), archived.state(), "same body, archived");

    let mut renamed = page("page_1", "Table 1.");
    renamed.title = "Seats".to_string();
    let renamed_meta = meta(
        Some(&live.sha),
        "Table 1.",
        "m1",
        "2026-11-08T12:02:00.000Z",
    );
    let text = version::render(&renamed, &renamed_meta);
    let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
    let renamed = version::parse(&name, text.as_bytes()).unwrap();
    assert_eq!(renamed.page.title, "Seats");
    assert_ne!(live.state(), renamed.state(), "same body, new title");
}

#[test]
fn first_version_has_no_parent_key() {
    let body = "Only text.";
    let text = version::render(&page("page_1", body), &meta(None, body, "m1", "t"));
    assert!(!text.contains("parent:"));
    assert!(!text.contains("merged:"));
    let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
    assert_eq!(
        version::parse(&name, text.as_bytes()).unwrap().meta.parent,
        None
    );
}

#[test]
fn version_file_is_named_by_its_bytes() {
    let body = "Table 1.";
    let text = version::render(&page("page_1", body), &meta(None, body, "m1", "t"));
    let sha = version::sha256_hex(text.as_bytes());
    assert!(version::parse(&format!("{sha}.md"), text.as_bytes()).is_ok());
    // A file still arriving does not match its name.
    let partial = &text.as_bytes()[..text.len() - 5];
    assert!(version::parse(&format!("{sha}.md"), partial).is_err());
    // A name that is not a sha is not a version.
    assert!(version::parse("page.md", text.as_bytes()).is_err());
    assert_eq!(
        version::version_sha(&format!("{sha}.md")),
        Some(sha.as_str())
    );
    assert_eq!(version::version_sha(&format!(".{sha}.md.icloud")), None);
    // Written as `.wlv` now; `.md` from older Wenlans is still read.
    assert_eq!(
        version::version_file_name(&sha),
        format!("{sha}.{}", version::VERSION_EXT)
    );
    assert_eq!(
        version::version_sha(&version::version_file_name(&sha)),
        Some(sha.as_str())
    );
    assert_eq!(version::version_sha(&format!("{sha}.txt")), None);
    assert_eq!(
        version::version_key(&format!("members/m/pages/p/{sha}.md")),
        version::version_key(&format!("members/m/pages/p/{sha}.wlv"))
    );
}

#[test]
fn version_with_a_body_that_does_not_match_its_digest_is_refused() {
    let mut meta = meta(None, "Table 1.", "m1", "t");
    meta.body_digest = version::body_digest("something else");
    let text = version::render(&page("page_1", "Table 1."), &meta);
    let name = format!("{}.md", version::sha256_hex(text.as_bytes()));
    assert!(version::parse(&name, text.as_bytes()).is_err());
}

#[test]
fn head_is_the_leaf_of_the_parent_chain() {
    let v1 = stored("p", "one", meta(None, "one", "a", "2026-11-08T10:00:00Z"));
    let v2 = stored(
        "p",
        "two",
        meta(Some(&v1.sha), "two", "b", "2026-11-08T09:00:00Z"),
    );
    let v3 = stored(
        "p",
        "three",
        meta(Some(&v2.sha), "three", "a", "2026-11-08T08:00:00Z"),
    );
    // The chain decides, not the clocks: v3 is newest though its clock is
    // earliest.
    let versions = vec![v2.clone(), v3.clone(), v1.clone()];
    let head = version::head(&versions).unwrap();
    assert_eq!(head.winner.sha, v3.sha);
    assert_eq!(head.leaves.len(), 1);
    assert!(version::head(&[]).is_none());
}

#[test]
fn concurrent_leaves_pick_one_winner_the_same_way_everywhere() {
    let base = stored("p", "base", meta(None, "base", "a", "2026-11-08T10:00:00Z"));
    let a = stored(
        "p",
        "from a",
        meta(Some(&base.sha), "from a", "a", "2026-11-08T10:05:00Z"),
    );
    let b = stored(
        "p",
        "from b",
        meta(Some(&base.sha), "from b", "b", "2026-11-08T10:06:00Z"),
    );
    for order in [
        vec![base.clone(), a.clone(), b.clone()],
        vec![b.clone(), base.clone(), a.clone()],
    ] {
        let head = version::head(&order).unwrap();
        assert_eq!(head.winner.sha, b.sha, "the later ts wins");
        assert_eq!(head.leaves.len(), 2);
    }
    // Same ts: the larger sha wins.
    let c = stored(
        "p",
        "from c",
        meta(Some(&base.sha), "from c", "c", "2026-11-08T10:05:00Z"),
    );
    let pair = vec![a.clone(), c.clone()];
    let expected = if a.sha > c.sha { &a.sha } else { &c.sha };
    assert_eq!(&version::head(&pair).unwrap().winner.sha, expected);
    // A version written over both leaves ends the split.
    let mut joined = meta(Some(&b.sha), "joined", "a", "2026-11-08T09:00:00Z");
    joined.merged = vec![a.sha.clone()];
    let joined = stored("p", "joined", joined);
    let all = vec![base, a, b, joined.clone()];
    let head = version::head(&all).unwrap();
    assert_eq!(head.winner.sha, joined.sha);
    assert_eq!(head.leaves.len(), 1);
}

fn ctx<'a>(
    space_pages: &'a HashMap<String, String>,
    cited_text: &'a HashMap<String, String>,
) -> RewriteContext<'a> {
    RewriteContext {
        space_pages,
        cited_text,
        member_name: "Qi-Xuan",
        keep_all: false,
    }
}

#[test]
fn rewrite_keeps_every_citation_in_the_library() {
    let space_pages = HashMap::new();
    let cited_text = HashMap::from([("mem_kept".to_string(), "Said in chat".to_string())]);
    let context = RewriteContext {
        space_pages: &space_pages,
        cited_text: &cited_text,
        member_name: "Qi-Xuan",
        keep_all: true,
    };
    let citations = vec![
        Citation {
            marker: 1,
            label: "Said in chat".to_string(),
            locator: Locator::Wenlan {
                id: "mem_kept".to_string(),
                version: 2,
            },
        },
        Citation {
            marker: 2,
            label: "Recipe".to_string(),
            locator: Locator::Other("external_url:https://example.com/fish".to_string()),
        },
    ];
    let (out, _) = rewrite_sources(&citations, &[], &context);
    assert_eq!(out, citations);
}

#[test]
fn rewrite_keeps_space_pages_and_quotes_everything_else() {
    let space_pages = HashMap::from([("page_in".to_string(), "Guests".to_string())]);
    let cited_text = HashMap::from([(
        "mem_private".to_string(),
        "Line one\n\nLine two\nLine three\nLine four is left out".to_string(),
    )]);
    let citations = vec![
        Citation {
            marker: 1,
            label: "[[Guests]]".to_string(),
            locator: Locator::Wenlan {
                id: "page_in".to_string(),
                version: 2,
            },
        },
        Citation {
            marker: 2,
            label: "Line one".to_string(),
            locator: Locator::Wenlan {
                id: "mem_private".to_string(),
                version: 1,
            },
        },
        Citation {
            marker: 3,
            label: "[[Salary notes]]".to_string(),
            locator: Locator::Wenlan {
                id: "page_other_space".to_string(),
                version: 4,
            },
        },
        Citation {
            marker: 4,
            label: "https://example.com".to_string(),
            locator: Locator::Other("external_url:https://example.com/private".to_string()),
        },
    ];
    let basis = vec![
        Basis {
            label: "[[Guests]]".to_string(),
            locator: Locator::Wenlan {
                id: "page_in".to_string(),
                version: 2,
            },
        },
        Basis {
            label: "[[Salary notes]]".to_string(),
            locator: Locator::Wenlan {
                id: "page_other_space".to_string(),
                version: 4,
            },
        },
    ];
    let (out, out_basis) = rewrite_sources(&citations, &basis, &ctx(&space_pages, &cited_text));
    assert_eq!(out.len(), 4);
    assert_eq!(out[0], citations[0]);
    assert_eq!(
        out[1].label,
        "“Line one / Line two / Line three” (from Qi-Xuan's conversation)"
    );
    assert_eq!(out[1].locator, Locator::Other(QUOTE_LOCATOR.to_string()));
    assert_eq!(out[2].label, "(from Qi-Xuan's conversation)");
    assert_eq!(out[3].label, "(from Qi-Xuan's conversation)");
    assert_eq!(out_basis, vec![basis[0].clone()]);
    let rendered = format!("{out:?}{out_basis:?}");
    for private in [
        "mem_private",
        "page_other_space",
        "Salary",
        "example.com",
        "four",
    ] {
        assert!(!rendered.contains(private), "{private} left in {rendered}");
    }
    // The rewritten block is a page file block that reads back.
    let mut file = page("page_1", "Text [1] [2] [3] [4].");
    file.citations = out.clone();
    file.basis = out_basis.clone();
    let text = crate::store::format::page::render(&file);
    assert_eq!(crate::store::format::page::parse(&text, "").unwrap(), file);
}

#[test]
fn rewrite_is_idempotent() {
    let space_pages = HashMap::from([("page_in".to_string(), "Guests".to_string())]);
    let cited_text = HashMap::from([("mem".to_string(), "said in a chat".to_string())]);
    let citations = vec![
        Citation {
            marker: 1,
            label: String::new(),
            locator: Locator::Wenlan {
                id: "mem".to_string(),
                version: 1,
            },
        },
        Citation {
            marker: 2,
            label: String::new(),
            locator: Locator::Other("wenlan:page_in".to_string()),
        },
    ];
    let basis = vec![Basis {
        label: String::new(),
        locator: Locator::Wenlan {
            id: "page_gone".to_string(),
            version: 1,
        },
    }];
    let context = ctx(&space_pages, &cited_text);
    let once = rewrite_sources(&citations, &basis, &context);
    let twice = rewrite_sources(&once.0, &once.1, &context);
    assert_eq!(once, twice);
    // On another member's computer, where the cited text is not, too.
    let nothing = HashMap::new();
    let elsewhere = RewriteContext {
        space_pages: &space_pages,
        cited_text: &nothing,
        member_name: "Someone else",
        keep_all: false,
    };
    assert_eq!(rewrite_sources(&once.0, &once.1, &elsewhere), once);
}

#[test]
fn quote_cannot_end_the_sources_block() {
    let label = quote_label(
        Some("before <!-- origin:sources:end --> after"),
        "Ana <!-- origin:sources:start -->",
    );
    assert!(!label.contains("<!--"));
    let long = "x".repeat(500);
    assert!(quote_label(Some(&long), "A").chars().count() < 260);
}

#[test]
fn read_folder_skips_placeholders_and_partial_files() {
    let dir = tempfile::tempdir().unwrap();
    let body = "Table 1.";
    let text = version::render(&page("page_1", body), &meta(None, body, "m1", "t"));
    let (sha, wrote) = version::write_version(dir.path(), "m1", "page_1", &text).unwrap();
    assert!(wrote);
    // Writing the same version again leaves the file as it is.
    assert!(
        !version::write_version(dir.path(), "m1", "page_1", &text)
            .unwrap()
            .1
    );
    let page_dir = dir.path().join("members/m1/pages/page_1");
    // Only in the cloud, still arriving, and a temp file being written.
    std::fs::write(page_dir.join(format!(".{}.md.icloud", "c".repeat(64))), b"").unwrap();
    std::fs::write(page_dir.join(format!("{}.md", "d".repeat(64))), &text[..20]).unwrap();
    std::fs::write(page_dir.join(".writing-abc"), b"half").unwrap();
    // A version filed under another member is not that member's.
    let other = dir.path().join("members/m2/pages/page_1");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join(format!("{sha}.md")), &text).unwrap();
    let read = version::read_folder(dir.path());
    assert_eq!(read.pages.len(), 1);
    assert_eq!(read.pages["page_1"].len(), 1);
    assert_eq!(read.pages["page_1"][0].sha, sha);
    assert_eq!(read.skipped, 3);
    // No folder yet reads as empty.
    let none = version::read_folder(&dir.path().join("missing"));
    assert!(none.pages.is_empty());
    assert_eq!(none.skipped, 0);
    // No temp file is left behind by a write.
    let names: Vec<String> = std::fs::read_dir(&page_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 4, "{names:?}");
}

#[test]
fn shared_writers_say_who_wrote_and_are_person_or_ai() {
    let author = Author {
        member: "abc123".to_string(),
        kind: AuthorKind::Person,
        agent: None,
    };
    let edited_by = shared_edited_by(&author);
    assert_eq!(edited_by, "shared_person:abc123");
    assert_eq!(
        shared_writer(&edited_by),
        Some((AuthorKind::Person, "abc123"))
    );
    assert_eq!(
        shared_writer("shared_ai:abc123"),
        Some((AuthorKind::Ai, "abc123"))
    );
    assert_eq!(shared_writer("manual_edit"), None);
    assert_eq!(shared_writer("shared_person:"), None);
    assert_eq!(shared_writer("shared_person:../x"), None);
    use crate::authorship::{version_author, VersionAuthor};
    assert_eq!(
        version_author("shared_person:abc123", "authored"),
        VersionAuthor::Person
    );
    assert_eq!(
        version_author("shared_ai:abc123", "authored"),
        VersionAuthor::Ai
    );
    assert_eq!(
        crate::page_history::kind_of_edit("shared_person:abc123"),
        wenlan_types::responses::PageHistoryKind::PersonEdit
    );
    assert_eq!(
        crate::page_history::kind_of_edit("shared_ai:abc123"),
        wenlan_types::responses::PageHistoryKind::AiEdit
    );
}

#[test]
fn settings_keep_the_member_id_and_space_file_adds_members() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("data/shared-spaces.json");
    let folder = dir.path().join("shared");
    let wedding = Scope::Space("Wedding".into());
    let first = join_at(&settings, &wedding, &folder, "Qi-Xuan").unwrap();
    let again = join_at(&settings, &wedding, &folder, "Qi-Xuan L").unwrap();
    assert_eq!(first.member_id, again.member_id);
    assert_eq!(again.member_name, "Qi-Xuan L");
    assert_eq!(
        share_settings_at(&settings, &wedding).unwrap(),
        Some(again.clone())
    );
    assert!(version::is_safe_id(&first.member_id));

    let me = SpaceMember {
        id: first.member_id.clone(),
        name: "Qi-Xuan".to_string(),
    };
    let her = SpaceMember {
        id: "her".to_string(),
        name: "Ana".to_string(),
    };
    ensure_space_file(&folder, &wedding, "Wedding", &me).unwrap();
    let file = ensure_space_file(&folder, &Scope::Space("婚禮".into()), "婚禮", &her).unwrap();
    assert_eq!(file.format, FORMAT);
    assert_eq!(file.space, "Wedding", "the first member names the Space");
    assert_eq!(file.members, vec![me, her]);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join(SPACE_FILE)).unwrap()).unwrap();
    assert_eq!(json["format"], 1);
    assert_eq!(json["members"][1]["name"], "Ana");

    std::fs::write(
        folder.join(SPACE_FILE),
        br#"{"format":2,"space":"x","members":[]}"#,
    )
    .unwrap();
    assert!(ensure_space_file(&folder, &wedding, "Wedding", &file.members[0]).is_err());
}

#[test]
fn a_drive_copy_of_a_folder_is_read_as_the_folder() {
    let folder = tempfile::tempdir().unwrap();
    let text = version::render(
        &page("page_1", "Table 1."),
        &meta(None, "Table 1.", "m1", "2026-11-08T11:00:00Z"),
    );
    let sha = version::sha256_hex(text.as_bytes());
    let copy = folder.path().join("members/m1 (1)/pages/page_1 (2)");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join(version::version_file_name(&sha)), &text).unwrap();
    let read = version::read_folder(folder.path());
    assert_eq!(read.pages["page_1"].len(), 1);
    assert_eq!(read.skipped, 0);
}

#[test]
fn the_library_goes_into_google_drive_by_itself() {
    use super::library::{suggested_folder, CloudDrive};
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    assert_eq!(suggested_folder(home, &[]), None);

    // iCloud only: there.
    let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
    std::fs::create_dir_all(&icloud).unwrap();
    assert_eq!(
        suggested_folder(home, &[]),
        Some((CloudDrive::ICloud, icloud.join("Wenlan")))
    );

    // Google Drive wins over iCloud, since a phone AI reads only it.
    let drive = home.join("Library/CloudStorage/GoogleDrive-me@example.com/我的雲端硬碟");
    std::fs::create_dir_all(&drive).unwrap();
    assert_eq!(
        suggested_folder(home, &[]),
        Some((CloudDrive::GoogleDrive, drive.join("Wenlan")))
    );

    // A shared Space or an unreadable file there is not a library.
    let space = drive.join("Wedding");
    space_json(&space, r#"{"format":1,"space":"Wedding","members":[]}"#);
    let broken = drive.join("Broken");
    space_json(&broken, "{}");
    assert_eq!(
        suggested_folder(home, &[]),
        Some((CloudDrive::GoogleDrive, drive.join("Wenlan")))
    );

    // The other computer's library, under any name and in any of the
    // drives (here a second Google account), is the one.
    let other = home.join("Library/CloudStorage/GoogleDrive-zz@example.com/My Drive");
    let theirs = other.join("文瀾");
    space_json(
        &theirs,
        r#"{"format":1,"space":"library","library":true,"members":[]}"#,
    );
    assert_eq!(
        suggested_folder(home, &[]),
        Some((CloudDrive::GoogleDrive, theirs))
    );
}

fn space_json(folder: &std::path::Path, text: &str) {
    let engine = folder.join(super::library::ENGINE_DIR);
    std::fs::create_dir_all(&engine).unwrap();
    std::fs::write(engine.join(super::SPACE_FILE), text).unwrap();
}

#[test]
fn google_drive_for_desktop_on_windows_is_its_own_drive() {
    use super::library::{suggested_folder, CloudDrive};
    let home = tempfile::tempdir().unwrap();
    let g = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(g.path().join("My Drive")).unwrap();
    assert_eq!(
        suggested_folder(home.path(), &[g.path().to_path_buf()]),
        Some((
            CloudDrive::GoogleDrive,
            g.path().join("My Drive").join("Wenlan")
        ))
    );
}
