//! Two people's edits of one page, made at the same time from the same
//! version, put together line by line (a three-way merge). Where both
//! changed the same lines differently there is no merge, and the page keeps
//! both as two pages.

use std::collections::HashMap;

/// `ours` and `theirs`, both edits of `base`, as one text: each changed
/// region is taken from the side that changed it. `None` when both changed
/// the same region differently.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Option<String> {
    if ours == theirs || theirs == base {
        return Some(ours.to_string());
    }
    if ours == base {
        return Some(theirs.to_string());
    }
    let base: Vec<&str> = base.split('\n').collect();
    let ours: Vec<&str> = ours.split('\n').collect();
    let theirs: Vec<&str> = theirs.split('\n').collect();
    let in_ours: HashMap<usize, usize> = crate::authorship::lcs_pairs(&base, &ours)
        .into_iter()
        .collect();
    let in_theirs: HashMap<usize, usize> = crate::authorship::lcs_pairs(&base, &theirs)
        .into_iter()
        .collect();
    let mut out: Vec<&str> = Vec::new();
    let (mut b, mut o, mut t) = (0, 0, 0);
    loop {
        // The next base line both sides kept, after where each side is.
        let stable = (b..base.len()).find_map(|k| {
            let (ko, kt) = (*in_ours.get(&k)?, *in_theirs.get(&k)?);
            (ko >= o && kt >= t).then_some((k, ko, kt))
        });
        let (bk, ok, tk) = stable.unwrap_or((base.len(), ours.len(), theirs.len()));
        out.extend(region(&base[b..bk], &ours[o..ok], &theirs[t..tk])?);
        let Some((k, ko, kt)) = stable else {
            break;
        };
        out.push(base[k]);
        (b, o, t) = (k + 1, ko + 1, kt + 1);
    }
    Some(out.join("\n"))
}

/// One region between lines both sides kept.
fn region<'a>(base: &[&'a str], ours: &[&'a str], theirs: &[&'a str]) -> Option<Vec<&'a str>> {
    if ours == theirs || theirs == base {
        Some(ours.to_vec())
    } else if ours == base {
        Some(theirs.to_vec())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::merge3;

    #[test]
    fn edits_of_different_lines_come_together() {
        let base = "Menu\n\nFish.\nRice.\nCake.";
        let ours = "Menu\n\nFish with lemon.\nRice.\nCake.";
        let theirs = "Menu\n\nFish.\nRice.\nCake and tea.";
        assert_eq!(
            merge3(base, ours, theirs).as_deref(),
            Some("Menu\n\nFish with lemon.\nRice.\nCake and tea.")
        );
    }

    #[test]
    fn lines_added_on_both_sides_in_different_places_are_kept() {
        let base = "A\nB\nC";
        let ours = "A\nA2\nB\nC";
        let theirs = "A\nB\nC\nD";
        assert_eq!(
            merge3(base, ours, theirs).as_deref(),
            Some("A\nA2\nB\nC\nD")
        );
    }

    #[test]
    fn the_same_line_changed_differently_does_not_merge() {
        assert_eq!(merge3("A\nB\nC", "A\nB1\nC", "A\nB2\nC"), None);
    }

    #[test]
    fn the_same_change_on_both_sides_is_taken_once() {
        assert_eq!(
            merge3("A\nB", "A\nB!\nC", "A\nB!\nC").as_deref(),
            Some("A\nB!\nC")
        );
    }

    #[test]
    fn a_side_that_changed_nothing_takes_the_other() {
        assert_eq!(merge3("A", "A", "B").as_deref(), Some("B"));
        assert_eq!(merge3("A", "B", "A").as_deref(), Some("B"));
    }
}
