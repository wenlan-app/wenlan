// SPDX-License-Identifier: Apache-2.0
//! File names for files Wenlan writes (spec section 3, common rules).
//!
//! A name is the title made safe on every supported system: NFC, no
//! characters Windows refuses or a `[[link]]` cannot hold, no leading dot,
//! no trailing dots or spaces, no reserved
//! device name, and at most [`MAX_STEM_BYTES`] bytes so the whole path stays
//! under Windows' 260 limit. The full title always lives in the file itself.

use unicode_normalization::UnicodeNormalization;

/// Longest stem in UTF-8 bytes, before `.md`.
pub const MAX_STEM_BYTES: usize = 120;

/// Stems Windows treats as devices whatever the extension.
const RESERVED: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The stem for a file titled `title`, without its extension.
pub fn file_stem(title: &str) -> String {
    let mut stem: String = title
        .nfc()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            // A `[[link]]` cannot name a file with these (Obsidian).
            '#' | '^' | '[' | ']' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    stem = stem.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_at_char(&mut stem, MAX_STEM_BYTES);
    let trimmed = stem.trim_end_matches(['.', ' ']).len();
    stem.truncate(trimmed);
    // A leading dot hides a file in Obsidian and on macOS and Linux.
    let stem = stem.trim_start_matches(['.', ' ']).to_string();
    if stem.is_empty() {
        return "page".to_string();
    }
    if is_reserved(&stem) {
        return format!("{stem}-page");
    }
    stem
}

/// The stem used when `stem` is already taken by another file: the last six
/// characters of the id are added, so the name stays stable across runs.
pub fn collision_stem(stem: &str, id: &str) -> String {
    let tail: String = {
        let chars: Vec<char> = id.chars().collect();
        chars[chars.len().saturating_sub(6)..].iter().collect()
    };
    let mut base = stem.to_string();
    truncate_at_char(&mut base, MAX_STEM_BYTES - tail.len() - 1);
    let trimmed = base.trim_end_matches(['.', ' ']).len();
    base.truncate(trimmed);
    format!("{base}-{tail}")
}

/// Whether two stems name the same file on a case-insensitive file system
/// (macOS and Windows defaults).
pub fn same_file_name(a: &str, b: &str) -> bool {
    fold(a) == fold(b)
}

fn fold(stem: &str) -> String {
    stem.nfc().flat_map(char::to_lowercase).collect()
}

fn is_reserved(stem: &str) -> bool {
    let device = stem.split('.').next().unwrap_or(stem).trim_end();
    RESERVED.contains(&device.to_ascii_lowercase().as_str())
}

fn truncate_at_char(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}
