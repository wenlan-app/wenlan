// SPDX-License-Identifier: Apache-2.0
use super::*;

#[derive(serde::Deserialize)]
struct Case {
    source: String,
    mermaid: String,
    wenlan: Option<String>,
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("../tests/fixtures/mermaid/cases.json")).expect("cases parse")
}

fn page(source: &str) -> String {
    format!("Some text.\n\n```mermaid\n{source}\n```\n\nMore text.\n")
}

#[test]
fn each_case_gets_the_problem_it_lists() {
    let wrong: Vec<String> = cases()
        .iter()
        .filter_map(|case| {
            let found = diagram_problems(&page(&case.source))
                .first()
                .map(|problem| problem.kind.code().to_string());
            (found != case.wenlan).then(|| format!("{:?}: found {found:?}", case.source))
        })
        .collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_diagram_mermaid_draws_is_only_ever_flagged_for_a_citation() {
    for case in cases().iter().filter(|case| case.mermaid == "ok") {
        assert!(
            matches!(case.wenlan.as_deref(), None | Some("citation")),
            "{:?}",
            case.source
        );
    }
}

#[test]
fn the_warning_names_the_diagram_and_its_line() {
    let content = "```mermaid\nflowchart LR\n  a --> b\n```\n\n```mermaid\nflowchart LR\n  a --> b\n  c -->\n```\n";
    assert_eq!(
        diagram_warnings(content),
        vec![
            "diagram 2 line 3: an arrow is missing the box at one end. Fix it and write the page \
             again, or remove the diagram."
                .to_string()
        ]
    );
}

#[test]
fn only_mermaid_blocks_are_read() {
    let content = "```sh\na -->\n```\n\n````markdown\n```mermaid\na -->\n```\n````\n\n~~~mermaid\nflowchart LR\n  a -->\n~~~\n";
    let problems = diagram_problems(content);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].diagram, 1);
    assert_eq!(problems[0].kind, ProblemKind::LooseArrow);
}

#[test]
fn a_page_without_diagrams_has_no_problems() {
    assert!(diagram_problems("Plain text with [1] a citation.\n").is_empty());
}

#[test]
fn dropping_keeps_good_diagrams_and_the_text_around_them() {
    let content = "Intro.\n\n```mermaid\nflowchart LR\n  a --> b\n```\n\nMiddle.\n\n```mermaid\nflowchart LR\n  a -->\n```\n\nEnd.\n";
    let (kept, dropped) = drop_broken_diagrams(content);
    assert_eq!(dropped, 1);
    assert_eq!(
        kept,
        "Intro.\n\n```mermaid\nflowchart LR\n  a --> b\n```\n\nMiddle.\n\nEnd.\n"
    );
    assert_eq!(drop_broken_diagrams(&kept), (kept.clone(), 0));
}

#[test]
fn an_unclosed_diagram_runs_to_the_end_of_the_page() {
    let (kept, dropped) = drop_broken_diagrams("Intro.\n\n```mermaid\nflowchart LR\n  a -->\n");
    assert_eq!(dropped, 1);
    assert_eq!(kept, "Intro.\n");
}
