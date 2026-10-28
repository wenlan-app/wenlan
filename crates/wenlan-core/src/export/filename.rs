// SPDX-License-Identifier: Apache-2.0
use sha2::{Digest, Sha256};

/// Bound UTF-8 bytes (and therefore UTF-16 units), reserving collision suffixes.
/// Keep complete titles in page metadata rather than filesystem components.
pub(super) fn bounded_basename(slug: &str, page_id: &str) -> String {
    if slug.is_empty() {
        return "page".into();
    }
    if slug.len() <= 180 {
        return slug.into();
    }
    let mut end = 140;
    while !slug.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}-{:x}", &slug[..end], Sha256::digest(page_id.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_names_write_and_do_not_collide() {
        let dir = tempfile::tempdir().unwrap();
        for title in ["a".repeat(300), "繁中頁面".repeat(90), "🙂".repeat(100)] {
            let first = bounded_basename(&title, "first");
            let second = bounded_basename(&title, "second");
            assert_ne!(first, second);
            assert!(first.len() <= 205);
            let path = dir.path().join(format!("{first}.md"));
            std::fs::write(&path, "complete title lives in content").unwrap();
            assert!(path.is_file());
        }
    }
}
