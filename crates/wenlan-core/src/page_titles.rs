use crate::{db::MemoryDB, error::WenlanError, read_scope::ReadScope};

const TITLE_MATCH_NOTES: usize = 2_000;
const SMALL_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "nor", "of", "on", "or", "per",
    "the", "to", "via", "vs", "with",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SameTitlePage {
    pub page_id: String,
    pub title: String,
    pub protected: bool,
}

/// The key two titles share when they name the same subject: lowercase
/// words, with years, dates and version numbers left out.
pub fn title_key(title: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    for word in title.split(|c: char| !c.is_alphanumeric()) {
        let letters: String = word
            .chars()
            .filter(|c| !c.is_numeric())
            .flat_map(char::to_lowercase)
            .collect();
        let had_digits = letters.chars().count() < word.chars().count();
        if letters.is_empty() || (had_digits && letters.chars().count() <= 1) {
            continue;
        }
        words.push(letters);
    }
    words.join(" ")
}

/// Give an all-lowercase ASCII title ordinary title casing. Mixed-case and
/// non-ASCII titles are left byte-for-byte alone because their casing carries
/// information the daemon cannot safely reconstruct.
pub fn title_case_if_lowercase_ascii(title: &str) -> String {
    let has_ascii_letter = title.chars().any(|c| c.is_ascii_alphabetic());
    if !has_ascii_letter
        || title.chars().any(|c| c.is_ascii_uppercase())
        || title.chars().any(|c| c.is_alphabetic() && !c.is_ascii())
    {
        return title.to_string();
    }

    let chars: Vec<char> = title.chars().collect();
    let mut out = String::with_capacity(title.len());
    let mut word_index = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        if !chars[i].is_ascii_alphabetic() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len()
            && (chars[i].is_ascii_alphabetic()
                || (chars[i] == '\'' && i + 1 < chars.len() && chars[i + 1].is_ascii_alphabetic()))
        {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let small = SMALL_WORDS.contains(&word.as_str());
        if word_index == 0 || !small {
            let mut letters = word.chars();
            if let Some(first) = letters.next() {
                out.extend(first.to_uppercase());
                out.extend(letters);
            }
        } else {
            out.push_str(&word);
        }
        word_index += 1;
    }
    out
}

/// The visible note whose title (years and versions aside) a new note would
/// repeat. Prefer the destination Space. Automatic writers can decline
/// human-edited or review-locked matches instead of mutating their sources.
pub async fn same_title_note(
    db: &MemoryDB,
    scope: &ReadScope,
    space: Option<&str>,
    title: &str,
    skip_protected: bool,
) -> Result<Option<SameTitlePage>, WenlanError> {
    let key = title_key(title);
    if key.is_empty() {
        return Ok(None);
    }
    let mut notes = db.notes_titled(title).await?;
    notes.extend(db.recent_notes(TITLE_MATCH_NOTES).await?);
    let mut seen = std::collections::HashSet::new();
    let mut protected_here = None;
    let mut elsewhere = None;
    let mut protected_elsewhere = None;
    for (page_id, note_title, page_space) in notes {
        if !seen.insert(page_id.clone())
            || !scope.matches(page_space.as_deref())
            || title_key(&note_title) != key
        {
            continue;
        }
        let protected = db
            .get_page(&page_id)
            .await?
            .is_some_and(|page| page.user_edited || page.review_required);
        let page = SameTitlePage {
            page_id,
            title: note_title,
            protected,
        };
        if page_space.as_deref() == space {
            if !skip_protected || !protected {
                return Ok(Some(page));
            }
            if protected_here.is_none() {
                protected_here = Some(page);
            }
            continue;
        }
        if protected && skip_protected {
            if protected_elsewhere.is_none() {
                protected_elsewhere = Some(page);
            }
        } else if elsewhere.is_none() {
            elsewhere = Some(page);
        }
    }
    Ok(protected_here.or(elsewhere).or(protected_elsewhere))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_case_fixes_only_all_lowercase_ascii_titles() {
        assert_eq!(
            title_case_if_lowercase_ascii("kitchen renovation"),
            "Kitchen Renovation"
        );
        assert_eq!(
            title_case_if_lowercase_ascii("the art of rust"),
            "The Art of Rust"
        );
        assert_eq!(
            title_case_if_lowercase_ascii("notes from the road"),
            "Notes from the Road"
        );
        assert_eq!(
            title_case_if_lowercase_ascii("state-of-the-art"),
            "State-of-the-Art"
        );
        assert_eq!(title_case_if_lowercase_ascii("don't panic"), "Don't Panic");
        assert_eq!(
            title_case_if_lowercase_ascii("iPhone setup"),
            "iPhone setup"
        );
        assert_eq!(title_case_if_lowercase_ascii("里斯本 trip"), "里斯本 trip");
    }

    #[test]
    fn title_keys_ignore_case_years_and_versions() {
        assert_eq!(title_key("Kitchen Renovation 2023"), "kitchen renovation");
        assert_eq!(title_key("kitchen renovation v2"), "kitchen renovation");
    }
}
