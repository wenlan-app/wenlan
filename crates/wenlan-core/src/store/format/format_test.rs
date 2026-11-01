// SPDX-License-Identifier: Apache-2.0
use super::names::{collision_stem, file_stem, same_file_name, MAX_STEM_BYTES};
use super::page::{self, Basis, Citation, Details, Locator, PageFile, PageKind};

#[test]
fn a_stem_is_safe_on_every_system() {
    assert_eq!(file_stem("Trip: plan / budget?"), "Trip plan budget");
    assert_eq!(file_stem("  notes...  "), "notes");
    assert_eq!(file_stem("a\tb\u{7}c"), "a b c");
    assert_eq!(file_stem("CON"), "CON-page");
    assert_eq!(file_stem("nul.txt"), "nul.txt-page");
    assert_eq!(file_stem("Lpt9"), "Lpt9-page");
    assert_eq!(file_stem("Console"), "Console");
    assert_eq!(file_stem(""), "page");
    assert_eq!(file_stem("???"), "page");
    // Decomposed é becomes the composed one.
    assert_eq!(file_stem("Cafe\u{301}"), "Café");
    assert_eq!(file_stem("旅行計畫"), "旅行計畫");

    for title in ["a".repeat(300), "旅".repeat(100), "🙂".repeat(80)] {
        let stem = file_stem(&title);
        assert!(stem.len() <= MAX_STEM_BYTES, "{} bytes", stem.len());
        assert!(title.starts_with(&stem));
    }
    assert_eq!(
        file_stem(&format!("{}.  x", "a".repeat(119))),
        "a".repeat(119)
    );
}

#[test]
fn a_taken_name_gets_the_end_of_the_id() {
    assert_eq!(collision_stem("Trip", "page_9f8e7d6c"), "Trip-8e7d6c");
    let long = collision_stem(&"a".repeat(MAX_STEM_BYTES), "page_123456");
    assert!(long.len() <= MAX_STEM_BYTES);
    assert!(long.ends_with("-123456"));
    assert!(same_file_name("Trip Plan", "trip plan"));
    assert!(same_file_name("Café", "CAFE\u{301}"));
    assert!(!same_file_name("Trip", "Trips"));
}

fn sample() -> PageFile {
    let mut page = PageFile::new(
        "Trip planning",
        "We fly on Friday [1].\n\nThe hotel is booked.",
    );
    page.id = Some("page_abc".to_string());
    page.tags = vec!["Travel".to_string()];
    page.created = Some("2026-11-01T10:00:00Z".to_string());
    page.modified = Some("2026-11-01T12:00:00Z".to_string());
    page.details = Some(Details {
        heading: "Details for AI".to_string(),
        text: "Flight LH 454, seat 32A.".to_string(),
    });
    page.citations = vec![Citation {
        marker: 1,
        label: "[[Flights]]".to_string(),
        locator: Locator::Wenlan {
            id: "page_def".to_string(),
            version: 3,
        },
    }];
    page.basis = vec![Basis {
        label: "[[Budget]]".to_string(),
        locator: Locator::Other("repo://docs/budget.md#L1-L4".to_string()),
    }];
    page
}

#[test]
fn a_page_file_reads_back_as_written() {
    let text = page::render(&sample());
    assert_eq!(
        text,
        "---
id: page_abc
title: Trip planning
type: page
tags:
- Travel
created: 2026-11-01T10:00:00Z
modified: 2026-11-01T12:00:00Z
---

We fly on Friday [1].

The hotel is booked.

<!-- wenlan:details -->
## Details for AI

Flight LH 454, seat 32A.

<!-- origin:sources:start -->
## Sources
[1]: [[Flights]] · wenlan:page_def@3
basis: [[Budget]] · repo://docs/budget.md#L1-L4
<!-- origin:sources:end -->
"
    );
    assert_eq!(page::parse(&text, "ignored").unwrap(), sample());
}

#[test]
fn a_file_made_in_obsidian_keeps_its_own_properties() {
    let text =
        "---\ncssclasses: wide\ntags: work\naliases: [Trip]\nrating: 4\n---\nBody with #tag.\n";
    let page = page::parse(text, "Trip plan").unwrap();
    assert_eq!(page.id, None);
    assert_eq!(page.title, "Trip plan");
    assert_eq!(page.kind, PageKind::Page);
    assert_eq!(page.tags, ["work"]);
    assert_eq!(page.aliases, ["Trip"]);
    assert_eq!(page.body, "Body with #tag.");
    let keys: Vec<_> = page.other.keys().filter_map(|key| key.as_str()).collect();
    assert_eq!(keys, ["cssclasses", "rating"]);

    let again = page::parse(&page::render(&page), "other").unwrap();
    assert_eq!(again, page);

    let plain = page::parse("Just text.\n", "Loose note").unwrap();
    assert_eq!(plain.title, "Loose note");
    assert_eq!(plain.body, "Just text.");
}

#[test]
fn a_value_wenlan_cannot_read_stays_as_written() {
    let text = "---\nid: page_x\ntitle: T\ntype: canvas-ish\ntags:\n  nested: map\n---\n\nBody\n";
    let page = page::parse(text, "T").unwrap();
    assert_eq!(page.kind, PageKind::Page);
    assert!(page.tags.is_empty());
    let keys: Vec<_> = page.other.keys().filter_map(|key| key.as_str()).collect();
    assert_eq!(keys, ["type", "tags"]);
}

#[test]
fn a_sources_block_that_is_not_wenlans_stays_in_the_body() {
    let text = "Body\n\n<!-- origin:sources:start -->\n## Sources\n- [[mem_1]]\n<!-- origin:sources:end -->\n";
    let page = page::parse(text, "T").unwrap();
    assert!(page.citations.is_empty());
    assert!(page.body.ends_with("<!-- origin:sources:end -->"));
}

#[test]
fn a_file_an_editor_rewrote_with_crlf_reads_with_lf() {
    let text = page::render(&sample()).replace('\n', "\r\n");
    assert_eq!(page::parse(&text, "x").unwrap(), sample());
}

/// Small deterministic generator, so the round trip runs over many shapes
/// without a property-testing dependency.
struct Gen(u64);

impl Gen {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() as usize) % items.len()]
    }
    fn chance(&mut self) -> bool {
        self.next().is_multiple_of(2)
    }
}

const WORDS: &[&str] = &[
    "plain",
    "旅行計畫",
    "🙂 emoji",
    "colon: inside",
    "\"quoted\"",
    "'single'",
    "yes",
    "null",
    "123",
    "1.5",
    "# not a heading",
    "- dash",
    "[1]",
    "a · b",
    "---",
    "  leading spaces",
    "tab\there",
    "back\\slash",
    "",
];

fn text_block(gen: &mut Gen) -> String {
    let lines = gen.next() % 4;
    let mut out = Vec::new();
    for _ in 0..lines {
        out.push(gen.pick(WORDS).to_string());
        if gen.chance() {
            out.push(String::new());
        }
    }
    out.join("\n").trim_end_matches('\n').to_string()
}

fn random_page(gen: &mut Gen) -> PageFile {
    let mut page = PageFile::new(gen.pick(WORDS), text_block(gen));
    if gen.chance() {
        page.id = Some(format!("page_{}", gen.next()));
    }
    if gen.chance() {
        page.kind = PageKind::Topic;
    }
    page.tags = (0..gen.next() % 3)
        .map(|_| gen.pick(WORDS).to_string())
        .collect();
    page.aliases = (0..gen.next() % 3)
        .map(|_| gen.pick(WORDS).to_string())
        .collect();
    if gen.chance() {
        page.created = Some("2026-11-01T10:00:00Z".to_string());
    }
    if gen.chance() {
        page.modified = Some(gen.pick(WORDS).to_string());
    }
    if gen.chance() {
        page.other
            .insert("rating".into(), serde_yaml::Value::Number(4.into()));
        page.other.insert("note".into(), gen.pick(WORDS).into());
    }
    if gen.chance() {
        page.details = Some(Details {
            heading: gen
                .pick(&["Details for AI", "給 AI 的細節", ""])
                .to_string(),
            text: text_block(gen),
        });
    }
    for marker in 1..=(gen.next() % 3) as u32 {
        page.citations.push(Citation {
            marker,
            label: format!("[[{}]]", gen.pick(WORDS)),
            locator: if gen.chance() {
                Locator::Wenlan {
                    id: format!("mem_{}", gen.next()),
                    version: (gen.next() % 9) as i64,
                }
            } else {
                Locator::Other(
                    gen.pick(&["url:https://example.com/a", "repo://src/a.rs#L1-L2"])
                        .to_string(),
                )
            },
        });
    }
    if gen.chance() {
        page.basis.push(Basis {
            label: format!("[[{}]]", gen.pick(WORDS)),
            locator: Locator::Wenlan {
                id: "page_b".to_string(),
                version: 3,
            },
        });
    }
    page
}

#[test]
fn every_page_wenlan_can_write_reads_back_unchanged() {
    let mut gen = Gen(7);
    for round in 0..2000 {
        let page = random_page(&mut gen);
        let text = page::render(&page);
        let read = page::parse(&text, "fallback")
            .unwrap_or_else(|error| panic!("round {round}: {error}\n{text}"));
        assert_eq!(read, page, "round {round}:\n{text}");
        assert_eq!(page::render(&read), text, "round {round}: render is stable");
    }
}

use super::brief::{self, BriefFile, BriefItem};
use super::entity::{self, EntityFile};
use super::memory::{self, MemoryFile};
use super::original::{self, Message, OriginalFile};

fn maybe(gen: &mut Gen) -> Option<String> {
    gen.chance().then(|| gen.pick(WORDS).to_string())
}

fn person_keys(gen: &mut Gen) -> serde_yaml::Mapping {
    let mut other = serde_yaml::Mapping::new();
    if gen.chance() {
        other.insert("rating".into(), serde_yaml::Value::Number(4.into()));
        other.insert("note".into(), gen.pick(WORDS).into());
    }
    other
}

/// One line, no line end at the end: the shape the formats keep exactly.
fn one_line(gen: &mut Gen) -> String {
    gen.pick(WORDS).to_string()
}

#[test]
fn a_memory_file_reads_back_as_written() {
    let mut memory = MemoryFile::new("mem_1", "Prefers window seats.");
    memory.kind = Some("preference".to_string());
    memory.space = Some("Travel".to_string());
    memory.agent = Some("claude-code".to_string());
    memory.created = Some("2026-11-01T10:00:00Z".to_string());
    memory.pinned = true;
    memory.importance = Some(7);
    memory.about = vec!["Lufthansa".to_string()];
    let text = memory::render(&memory);
    assert_eq!(
        text,
        "---
id: mem_1
type: memory
kind: preference
space: Travel
agent: claude-code
created: 2026-11-01T10:00:00Z
pinned: true
importance: 7
about:
- '[[Lufthansa]]'
---

Prefers window seats.
"
    );
    assert_eq!(memory::parse(&text).unwrap(), memory);
    assert!(memory::parse("---\ntype: memory\n---\nno id\n").is_err());
    assert!(memory::parse("---\nid: x\ntype: page\n---\n").is_err());
}

#[test]
fn every_memory_reads_back_unchanged() {
    let mut gen = Gen(11);
    for round in 0..1000 {
        let mut memory = MemoryFile::new(format!("mem_{}", gen.next()), text_block(&mut gen));
        memory.kind = maybe(&mut gen);
        memory.space = maybe(&mut gen);
        memory.agent = maybe(&mut gen);
        memory.created = maybe(&mut gen);
        memory.pinned = gen.chance();
        memory.confirmed = gen.chance();
        memory.supersedes = maybe(&mut gen);
        memory.supersede_mode = maybe(&mut gen);
        memory.episode_of = maybe(&mut gen);
        memory.event_date = maybe(&mut gen);
        memory.importance = gen.chance().then(|| (gen.next() % 11) as i64);
        memory.about = (0..gen.next() % 3).map(|_| one_line(&mut gen)).collect();
        memory.other = person_keys(&mut gen);
        let text = memory::render(&memory);
        let read = memory::parse(&text).unwrap_or_else(|error| panic!("{round}: {error}\n{text}"));
        assert_eq!(read, memory, "round {round}:\n{text}");
        assert_eq!(memory::render(&read), text, "round {round}");
    }
}

#[test]
fn every_entity_reads_back_unchanged() {
    let mut gen = Gen(13);
    for round in 0..1000 {
        let mut entity = EntityFile::new(format!("ent_{}", gen.next()), one_line(&mut gen));
        entity.entity_type = maybe(&mut gen);
        entity.aliases = (0..gen.next() % 3).map(|_| one_line(&mut gen)).collect();
        entity.confirmed = gen.chance();
        entity.established_by = maybe(&mut gen);
        entity.space = maybe(&mut gen);
        entity.other = person_keys(&mut gen);
        entity.description = text_block(&mut gen);
        let text = entity::render(&entity);
        let read = entity::parse(&text).unwrap_or_else(|error| panic!("{round}: {error}\n{text}"));
        assert_eq!(read, entity, "round {round}:\n{text}");
        assert_eq!(entity::render(&read), text, "round {round}");
    }
}

#[test]
fn a_brief_reads_as_a_checklist() {
    let brief = BriefFile {
        id: "brief_1".to_string(),
        space: "Work".to_string(),
        version: 7,
        other: serde_yaml::Mapping::new(),
        summary: "Left off at the export.".to_string(),
        active: vec![BriefItem {
            id: "item_1".to_string(),
            text: "Ship the export".to_string(),
            added: 1_793_491_200,
            version: 3,
            gate: None,
        }],
        backlog: vec![BriefItem {
            id: "item_2".to_string(),
            text: "Try <the> new --> parser\nnext week".to_string(),
            added: 1_793_491_300,
            version: 5,
            gate: Some("after the export".to_string()),
        }],
    };
    let text = brief::render(&brief);
    assert!(text.contains(
        "## Active\n- [ ] Ship the export <!-- wenlan {\"id\":\"item_1\",\"added\":1793491200,\"version\":3} -->\n"
    ), "{text}");
    assert!(
        text.contains("- [ ] Try <the> new --> parser next week <!-- wenlan "),
        "{text}"
    );
    assert_eq!(
        text.matches("-->").count(),
        3,
        "only the item comments close: {text}"
    );
    assert_eq!(brief::parse(&text).unwrap(), brief);
}

#[test]
fn every_brief_reads_back_unchanged() {
    let mut gen = Gen(17);
    let item = |gen: &mut Gen| BriefItem {
        id: format!("item_{}", gen.next()),
        text: if gen.chance() {
            text_block(gen)
        } else {
            one_line(gen)
        },
        added: gen.next() as i64,
        version: (gen.next() % 9) as i64,
        gate: maybe(gen),
    };
    for round in 0..1000 {
        let brief = BriefFile {
            id: format!("brief_{}", gen.next()),
            space: one_line(&mut gen),
            version: (gen.next() % 50) as i64,
            other: person_keys(&mut gen),
            summary: text_block(&mut gen),
            active: (0..gen.next() % 3).map(|_| item(&mut gen)).collect(),
            backlog: (0..gen.next() % 3).map(|_| item(&mut gen)).collect(),
        };
        let text = brief::render(&brief);
        let read = brief::parse(&text).unwrap_or_else(|error| panic!("{round}: {error}\n{text}"));
        assert_eq!(read, brief, "round {round}:\n{text}");
        assert_eq!(brief::render(&read), text, "round {round}");
    }
}

#[test]
fn an_original_keeps_each_message_and_a_heading_inside_one() {
    let original = OriginalFile {
        id: "orig_1".to_string(),
        vendor: "chatgpt".to_string(),
        external_id: "6720-abc".to_string(),
        title: Some("Trip ideas".to_string()),
        imported: Some("2026-11-01T10:00:00Z".to_string()),
        url: Some("https://chatgpt.com/c/6720-abc".to_string()),
        other: serde_yaml::Mapping::new(),
        messages: vec![
            Message {
                role: "user".to_string(),
                at: Some("2026-10-30T09:12:00Z".to_string()),
                text: "Where should we go in May?".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                at: None,
                text: "## Options\n\n- Lisbon\n- Kyoto".to_string(),
            },
        ],
    };
    let text = original::render(&original);
    assert!(text.contains("author: original\n"));
    assert_eq!(original::parse(&text).unwrap(), original);
    let edited = text.replace("author: original", "author: person");
    assert!(
        original::parse(&edited).is_err(),
        "an original is never the person's"
    );
}

#[test]
fn every_original_reads_back_unchanged() {
    let mut gen = Gen(19);
    for round in 0..1000 {
        let original = OriginalFile {
            id: format!("orig_{}", gen.next()),
            vendor: gen.pick(&["chatgpt", "claude", "gemini"]).to_string(),
            external_id: one_line(&mut gen),
            title: maybe(&mut gen),
            imported: maybe(&mut gen),
            url: maybe(&mut gen),
            other: person_keys(&mut gen),
            messages: (0..gen.next() % 4)
                .map(|_| Message {
                    role: gen.pick(&["user", "assistant", "tool"]).to_string(),
                    at: gen.chance().then(|| "2026-10-30T09:12:00Z".to_string()),
                    text: text_block(&mut gen),
                })
                .collect(),
        };
        let text = original::render(&original);
        let read =
            original::parse(&text).unwrap_or_else(|error| panic!("{round}: {error}\n{text}"));
        assert_eq!(read, original, "round {round}:\n{text}");
        assert_eq!(original::render(&read), text, "round {round}");
    }
}
