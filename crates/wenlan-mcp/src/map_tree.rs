// SPDX-License-Identifier: AGPL-3.0-only
//! A page's own mind map as a small text tree, for AI tools that can't show a
//! window. `read_page` carries it, with a link that opens the page in the
//! Wenlan app, only when the person asks to see that page's map. The
//! conversation's working-memory map (docs/ux/spec.md, UX-CHAT-4) is separate
//! and not built yet.

use std::collections::HashMap;

use wenlan_types::page_map::{PageMapNode, PageMapResponse, RefState};

/// Lines the tree may use before the rest is left for the app.
pub const MAX_TREE_LINES: usize = 24;

/// The link the desktop app opens a page from.
pub fn open_link(page_id: &str) -> String {
    format!("wenlan://page/{page_id}")
}

/// The name a node shows: its own label, else the name of what it points at
/// (`"{ref_kind}:{ref_id}"` in `names`). Never an id.
fn node_name(node: &PageMapNode, names: &HashMap<String, String>) -> String {
    if let Some(label) = node
        .label
        .as_deref()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        return label.to_string();
    }
    if node.ref_state == RefState::Dangling {
        return "(removed)".to_string();
    }
    names
        .get(&format!("{}:{}", node.ref_kind, node.ref_id))
        .cloned()
        .unwrap_or_else(|| "Untitled".to_string())
}

/// The tree under the map's root, or `None` when the map holds only its root
/// (a page that has never been mapped reads the same as one with no map).
/// AI suggestions the person has not accepted are marked; collapsed branches
/// stay collapsed and say how much they hide.
pub fn render(map: &PageMapResponse, names: &HashMap<String, String>) -> Option<String> {
    let mut children: HashMap<Option<&str>, Vec<&PageMapNode>> = HashMap::new();
    for node in &map.nodes {
        children
            .entry(node.parent_id.as_deref())
            .or_default()
            .push(node);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| a.rank.total_cmp(&b.rank));
    }
    let roots = children.get(&None)?;
    if map.nodes.len() <= roots.len() {
        return None;
    }

    let mut lines = Vec::new();
    let mut hidden = 0usize;
    for root in roots {
        push_line(
            &mut lines,
            &mut hidden,
            String::new(),
            root,
            names,
            &children,
        );
        walk(root, "", names, &children, &mut lines, &mut hidden);
    }
    if hidden > 0 {
        lines.push(format!("… {hidden} more in Wenlan"));
    }
    Some(lines.join("\n"))
}

fn walk(
    node: &PageMapNode,
    prefix: &str,
    names: &HashMap<String, String>,
    children: &HashMap<Option<&str>, Vec<&PageMapNode>>,
    lines: &mut Vec<String>,
    hidden: &mut usize,
) {
    if node.collapsed {
        return;
    }
    let Some(kids) = children.get(&Some(node.id.as_str())) else {
        return;
    };
    for (index, kid) in kids.iter().enumerate() {
        let last = index + 1 == kids.len();
        let branch = format!("{prefix}{}", if last { "└─ " } else { "├─ " });
        push_line(lines, hidden, branch, kid, names, children);
        let next = format!("{prefix}{}", if last { "   " } else { "│  " });
        walk(kid, &next, names, children, lines, hidden);
    }
}

fn push_line(
    lines: &mut Vec<String>,
    hidden: &mut usize,
    branch: String,
    node: &PageMapNode,
    names: &HashMap<String, String>,
    children: &HashMap<Option<&str>, Vec<&PageMapNode>>,
) {
    if lines.len() >= MAX_TREE_LINES {
        *hidden += 1;
        return;
    }
    let mut line = branch;
    if node.status == "suggested" {
        line.push_str("(AI suggestion) ");
    }
    line.push_str(&node_name(node, names));
    if node.collapsed {
        let folded = descendants(node, children);
        if folded > 0 {
            line.push_str(&format!(" (+{folded})"));
        }
    }
    lines.push(line);
}

fn descendants(node: &PageMapNode, children: &HashMap<Option<&str>, Vec<&PageMapNode>>) -> usize {
    children
        .get(&Some(node.id.as_str()))
        .map(|kids| kids.iter().map(|kid| 1 + descendants(kid, children)).sum())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        id: &str,
        parent: Option<&str>,
        rank: f64,
        kind: &str,
        label: Option<&str>,
    ) -> PageMapNode {
        PageMapNode {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            rank,
            ref_kind: kind.to_string(),
            ref_id: format!("ref-{id}"),
            label: label.map(str::to_string),
            status: "active".to_string(),
            pinned: false,
            placed: false,
            collapsed: false,
            x: None,
            y: None,
            width: None,
            height: None,
            ref_state: RefState::Live,
        }
    }

    fn map(nodes: Vec<PageMapNode>) -> PageMapResponse {
        PageMapResponse {
            page_id: "page_1".to_string(),
            revision: 1,
            map_schema: 1,
            viewport: None,
            nodes,
            edges: vec![],
            independent_ideas: true,
        }
    }

    #[test]
    fn draws_the_map_as_a_text_tree_without_ids() {
        let mut suggested = node("c", Some("a"), 1.0, "idea", Some("Wikis are dead"));
        suggested.status = "suggested".to_string();
        let names = HashMap::from([
            ("page:ref-root".to_string(), "New tagline".to_string()),
            (
                "memory:ref-b".to_string(),
                "You do the thinking".to_string(),
            ),
        ]);
        let tree = render(
            &map(vec![
                node("root", None, 0.0, "page", None),
                node("a", Some("root"), 0.0, "section", Some("Lines")),
                node("b", Some("a"), 0.0, "memory", None),
                suggested,
                node("d", Some("root"), 1.0, "entity", None),
            ]),
            &names,
        )
        .expect("a mapped page has a tree");
        assert_eq!(
            tree,
            "New tagline\n\
             ├─ Lines\n\
             │  ├─ You do the thinking\n\
             │  └─ (AI suggestion) Wikis are dead\n\
             └─ Untitled"
        );
        assert!(!tree.contains("ref-"), "no internal ids: {tree}");
    }

    #[test]
    fn keeps_collapsed_branches_closed_and_stays_small() {
        let mut folded = node("a", Some("root"), 0.0, "idea", Some("Folded"));
        folded.collapsed = true;
        let mut nodes = vec![
            node("root", None, 0.0, "idea", Some("Root")),
            folded,
            node("a1", Some("a"), 0.0, "idea", Some("Hidden")),
        ];
        for i in 0..40 {
            nodes.push(node(
                &format!("n{i}"),
                Some("root"),
                1.0 + i as f64,
                "idea",
                Some("Idea"),
            ));
        }
        let tree = render(&map(nodes), &HashMap::new()).unwrap();
        assert!(tree.contains("Folded (+1)"));
        assert!(!tree.contains("Hidden"));
        let lines: Vec<&str> = tree.lines().collect();
        assert_eq!(lines.len(), MAX_TREE_LINES + 1);
        assert_eq!(lines.last(), Some(&"… 18 more in Wenlan"));
    }

    #[test]
    fn a_map_with_only_its_root_has_no_tree() {
        assert_eq!(
            render(
                &map(vec![node("root", None, 0.0, "page", None)]),
                &HashMap::new()
            ),
            None
        );
        assert_eq!(render(&map(vec![]), &HashMap::new()), None);
    }
}
