// SPDX-License-Identifier: Apache-2.0
//! Write-integrity helpers: shrink-guard, damage-guard, count-guarded
//! find/replace, and substitution-only enrichment.
//!
//! All functions in this module are pure (no I/O, no env reads, no side
//! effects). Env reads (`WENLAN_MERGE_SHRINK_GUARD`) live at the call site in
//! `post_write.rs` so this module stays trivially testable.

/// Returns `true` iff replacing `old` with `new` is NOT a catastrophic shrink.
///
/// Catastrophic shrink is defined as: `new.chars().count() as f64 <
/// old.chars().count() as f64 * threshold`.
///
/// Special cases:
/// - Empty `old` → `true` (nothing to lose; guards against div-by-zero).
/// - Growth (new longer than old) → always `true`.
/// - `threshold = 0.0` → always `true` (guard effectively disabled).
/// - `threshold = 1.0` → any shrink is rejected.
///
/// Uses [`str::chars().count()`], **not** byte length, so multibyte UTF-8
/// characters are counted correctly (AGENTS.md UTF-8 byte-index rule).
///
/// Reference value: `llm_wiki BODY_SHRINK_THRESHOLD = 0.7`.
pub fn body_shrink_ok(old: &str, new: &str, threshold: f64) -> bool {
    let old_len = old.chars().count();
    if old_len == 0 {
        return true;
    }
    let new_len = new.chars().count() as f64;
    new_len >= old_len as f64 * threshold
}

/// Replace ALL occurrences of `find` in `content` with `replace`, but ONLY
/// if the actual occurrence count equals `expected`.
///
/// Returns `Some(result)` on success, `None` on refusal.
///
/// Refusal conditions:
/// - `find` is empty → `None`.
/// - Actual occurrence count ≠ `expected` → `None` (refuse — don't guess).
///
/// This is the **basic-memory count-guard** pattern: callers must supply the
/// exact count they observed when building the replacement; any concurrent
/// mutation that changes the count causes a safe no-op refusal.
///
/// # UNWIRED — forward-looking primitive
///
/// This function is not yet wired into any production path. It ships as a
/// tested primitive for a future `edit_memory` HTTP route (analogous to
/// basic-memory's count-guarded replace). Do not retrofit into existing paths
/// without an explicit task.
#[allow(dead_code)]
pub fn find_replace_guarded(
    content: &str,
    find: &str,
    replace: &str,
    expected: usize,
) -> Option<String> {
    if find.is_empty() {
        return None;
    }
    let actual = content.matches(find).count();
    if actual != expected {
        return None;
    }
    Some(content.replace(find, replace))
}

/// Apply a list of (find, replace) substitutions to `content` in order.
///
/// Each substitution is applied sequentially (left-to-right) via
/// [`str::replace`]. No free-form rewriting — only exact string substitutions.
///
/// # UNWIRED — forward-looking primitive
///
/// Intended for substitution-only auto-linker enrichment (e.g. rewriting raw
/// entity names to `[[wikilink]]` form across a page body) once a production
/// caller exists. Ships unwired so the primitive can be tested independently.
/// Do not wire into existing paths without an explicit task.
#[allow(dead_code)]
pub fn substitute_terms(content: &str, subs: &[(String, String)]) -> String {
    let mut result = content.to_string();
    for (find, replace) in subs {
        result = result.replace(find.as_str(), replace.as_str());
    }
    result
}

/// True when a rewrite ends inside a prose sentence: the model ran out of
/// tokens or time. A last line that is a heading, list item, table row,
/// quote or code fence is left alone, because those often end without
/// punctuation.
pub fn looks_cut_off(body: &str) -> bool {
    let Some(last) = body.lines().map(str::trim).rfind(|line| !line.is_empty()) else {
        return false;
    };
    let structured = last.starts_with(['#', '|', '-', '*', '+', '>', '`'])
        || last
            .split_once(['.', ')'])
            .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if structured {
        return false;
    }
    let Some(end) = last.chars().last() else {
        return false;
    };
    !matches!(
        end,
        '.' | '!'
            | '?'
            | ':'
            | ';'
            | ')'
            | ']'
            | '}'
            | '"'
            | '\''
            | '`'
            | '*'
            | '_'
            | '~'
            | '…'
            | '。'
            | '！'
            | '？'
            | '：'
            | '；'
            | '）'
            | '」'
            | '』'
            | '》'
            | '”'
            | '’'
            | '】'
    )
}

/// Why a machine rewrite of a page must not land, or `None` when it may.
///
/// Every check asks whether *this write* introduced the damage, so a page
/// that already carried it (a heading-only last line, a quoted "Here is",
/// a paragraph a person repeated on purpose) does not lock the page.
/// Each class was found in real pages written by on-device models
/// (2026-11-04): lost characters shown as U+FFFD, a chat preamble as the
/// first line, a page body appended to itself, and rewrites that stopped
/// mid-sentence.
///
/// Broken characters, a preamble and a repeated paragraph are refused from
/// every AI writer. `background` is true for a background machine pass
/// (distill, re-distill, page growth, refinery merge): it is also refused
/// when it stops mid-sentence, leaves half a `[[link]]`, or loses most of the
/// page's Chinese, Japanese or Korean text. An agent writing for the person
/// may end a page without punctuation or translate it on request (UX-WHO-8).
pub fn rewrite_damage(old: &str, new: &str, background: bool) -> Option<&'static str> {
    if broken_char_count(new) > broken_char_count(old) {
        return Some("it contains broken characters");
    }
    if starts_with_preamble(new) && !starts_with_preamble(old) {
        return Some("it starts with a chat reply instead of the page");
    }
    if repeats_a_block(old, new) {
        return Some("it repeats a whole paragraph");
    }
    if !background {
        return None;
    }
    if looks_cut_off(new) && !looks_cut_off(old) {
        return Some("it stops mid-sentence");
    }
    if half_wiki_link(new).is_some() && half_wiki_link(old).is_none() {
        return Some("it leaves half a link");
    }
    if drops_most_cjk(old, new) {
        return Some("it drops most of the page's Chinese, Japanese or Korean text");
    }
    None
}

/// Pages with fewer CJK characters than this are too short to judge a loss.
pub const CJK_LOSS_MIN: usize = 20;

/// True for a Han, kana or Hangul character. Punctuation (`。`, `「`) is not
/// counted: a lossy rewrite drops the words, and punctuation alone says nothing.
pub fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF       // Hiragana, Katakana
        | 0x3400..=0x4DBF     // CJK Extension A
        | 0x4E00..=0x9FFF     // CJK Unified Ideographs
        | 0xF900..=0xFAFF     // CJK Compatibility Ideographs
        | 0x1100..=0x11FF     // Hangul Jamo
        | 0x3130..=0x318F     // Hangul Compatibility Jamo
        | 0xAC00..=0xD7AF     // Hangul Syllables
        | 0x20000..=0x3134F   // CJK Extensions B-H
    )
}

pub fn cjk_count(body: &str) -> usize {
    body.chars().filter(|&c| is_cjk(c)).count()
}

pub fn broken_char_count(body: &str) -> usize {
    body.chars().filter(|&c| c == '\u{FFFD}').count()
}

/// True when `new` keeps fewer than half of `old`'s CJK characters *and* CJK
/// makes up less than half the share of the page it did. The share test keeps
/// an honest shortening (a section cut, every script shrinking together) out:
/// only a write whose CJK vanished while the rest stayed trips it — the shape
/// of text lost to a bad encoding or a model that answered in another language.
pub fn drops_most_cjk(old: &str, new: &str) -> bool {
    let before = cjk_count(old);
    if before < CJK_LOSS_MIN {
        return false;
    }
    let after = cjk_count(new);
    if after * 2 >= before {
        return false;
    }
    let (old_len, new_len) = (old.chars().count(), new.chars().count());
    // after/new_len < (before/old_len) / 2, kept in integers.
    after * old_len * 2 < before * new_len
}

/// The first `[[` link opened and never closed on its line, as written: an
/// interrupted write leaves `[[mem_e5c607eb6` at the end. Links do not span
/// lines, so a `[[` with no `]]` before the next `[[` or the line end is cut.
/// A `[[` inside inline code (`` `[[` to link a note ``) is text, not a link.
pub fn half_wiki_link(body: &str) -> Option<&str> {
    body.lines()
        .flat_map(outside_inline_code)
        .find_map(half_link_in)
}

fn half_link_in(text: &str) -> Option<&str> {
    let mut rest = text;
    while let Some(open) = rest.find("[[") {
        let after = &rest[open + 2..];
        let close = after.find("]]");
        let next_open = after.find("[[");
        match (close, next_open) {
            (Some(c), Some(n)) if n < c => {
                return Some(link_fragment(&rest[open..open + 2 + n]));
            }
            (Some(c), _) => rest = &after[c + 2..],
            (None, _) => return Some(link_fragment(&rest[open..])),
        }
    }
    None
}

/// The parts of `line` outside inline code spans, each a slice of `line`. A
/// trailing unpaired backtick opens no span: the text after it is kept.
fn outside_inline_code(line: &str) -> Vec<&str> {
    let ticks: Vec<usize> = line.match_indices('`').map(|(at, _)| at).collect();
    let mut parts = Vec::new();
    let mut from = 0;
    for pair in ticks.chunks_exact(2) {
        parts.push(&line[from..pair[0]]);
        from = pair[1] + 1;
    }
    parts.push(&line[from..]);
    parts
}

fn link_fragment(s: &str) -> &str {
    match s.char_indices().nth(48) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Openers a model uses when it answers instead of writing the page.
const PREAMBLE_OPENERS: &[&str] = &[
    "here is ",
    "here's ",
    "here are ",
    "sure,",
    "sure!",
    "sure.",
    "certainly",
    "below is ",
    "i've updated",
    "i have updated",
    "以下是",
    "好的，",
    "好的,",
    "当然",
    "當然",
];

fn starts_with_preamble(body: &str) -> bool {
    let Some(first) = body.lines().map(str::trim).find(|l| !l.is_empty()) else {
        return false;
    };
    let first = first.to_lowercase();
    PREAMBLE_OPENERS.iter().any(|p| first.starts_with(p))
}

/// Paragraphs shorter than this are allowed to repeat (short labels,
/// "TBD", a recurring list item).
const REPEAT_MIN_CHARS: usize = 80;

/// True when `new` holds a long paragraph more times than `old` does and
/// at least twice.
fn repeats_a_block(old: &str, new: &str) -> bool {
    fn counts(body: &str) -> std::collections::HashMap<&str, usize> {
        let mut map = std::collections::HashMap::new();
        for para in body.split("\n\n").map(str::trim) {
            if para.chars().count() >= REPEAT_MIN_CHARS {
                *map.entry(para).or_insert(0) += 1;
            }
        }
        map
    }
    let before = counts(old);
    counts(new)
        .into_iter()
        .any(|(para, n)| n > 1 && n > before.get(para).copied().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_damage_flags_what_the_write_introduced() {
        let page = "## Notes\n\nHybrid search fuses keyword and vector results.";
        assert_eq!(rewrite_damage(page, page, true), None);
        assert_eq!(
            rewrite_damage(page, "混合搜尋\u{FFFD}結果。", true),
            Some("it contains broken characters")
        );
        assert_eq!(
            rewrite_damage(page, "Here is the updated page:\n\n## Notes\n\nText.", true),
            Some("it starts with a chat reply instead of the page")
        );
        assert_eq!(
            rewrite_damage(page, "以下是更新後的頁面：\n\n內容。", true),
            Some("it starts with a chat reply instead of the page")
        );
        let para = "Hybrid search fuses keyword and vector results, then reranks them by how close each one is to the question.";
        assert_eq!(
            rewrite_damage(para, &format!("{para}\n\n{para}"), true),
            Some("it repeats a whole paragraph")
        );
        assert_eq!(
            rewrite_damage(page, "## Notes\n\nHybrid search fuses keyword and", true),
            Some("it stops mid-sentence")
        );
        assert_eq!(
            rewrite_damage(page, "## Notes\n\nHybrid search fuses keyword and", false),
            None
        );
    }

    #[test]
    fn rewrite_damage_ignores_what_the_page_already_had() {
        let preamble = "Here is what I learned about search.\n\nMore.";
        assert_eq!(rewrite_damage(preamble, preamble, true), None);
        let para = "Hybrid search fuses keyword and vector results, then reranks them by how close each one is to the question.";
        let twice = format!("{para}\n\n{para}");
        assert_eq!(
            rewrite_damage(&twice, &format!("{twice}\n\nNew line."), true),
            None
        );
        assert_eq!(
            rewrite_damage("Draft without an end", "Draft still without an end", true),
            None
        );
        assert_eq!(rewrite_damage("a\u{FFFD}b.", "a\u{FFFD}b c.", true), None);
    }

    // UX-WHO-8: an AI write never leaves a page broken. These pin the
    // half-link and CJK-loss checks, and who they apply to.
    const ZH: &str = "混合搜尋把關鍵字和向量的結果合在一起，再依照和問題的距離重新排序，這樣找到的內容比較準確。";

    #[test]
    fn ux_who_8_a_background_pass_that_drops_most_cjk_is_refused() {
        let page = format!("## 搜尋\n\n{ZH}");
        // Same length, the Chinese replaced by question marks: lossy encoding.
        let lost: String = page
            .chars()
            .map(|c| if is_cjk(c) { '?' } else { c })
            .collect();
        assert_eq!(
            rewrite_damage(&page, &lost, true),
            Some("it drops most of the page's Chinese, Japanese or Korean text")
        );
        // An English answer in place of the Chinese page trips it too.
        assert_eq!(
            rewrite_damage(
                &page,
                "## Search\n\nHybrid search fuses keyword and vector results, then reranks them.",
                true
            ),
            Some("it drops most of the page's Chinese, Japanese or Korean text")
        );
    }

    #[test]
    fn ux_who_8_an_agent_translation_lands_but_broken_characters_do_not() {
        let page = format!("## 搜尋\n\n{ZH}");
        // The person asked their AI tool for an English version: it lands.
        assert_eq!(
            rewrite_damage(
                &page,
                "## Search\n\nHybrid search fuses keyword and vector results, then reranks them.",
                false
            ),
            None
        );
        // Broken characters are refused from an agent too.
        let broken = page.replacen("搜尋", "搜\u{FFFD}", 1);
        assert_eq!(
            rewrite_damage(&page, &broken, false),
            Some("it contains broken characters")
        );
    }

    #[test]
    fn ux_who_8_a_background_pass_that_leaves_half_a_link_is_refused() {
        let page = "## Notes\n\nSee [[mem_abc]] for the decision.";
        let half = "## Notes\n\nSee [[mem_ab for the decision.";
        assert_eq!(
            rewrite_damage(page, half, true),
            Some("it leaves half a link")
        );
        // An agent may write about the `[[` syntax itself.
        assert_eq!(rewrite_damage(page, half, false), None);
        // A half link the page already had does not lock it.
        assert_eq!(
            rewrite_damage(half, &format!("{half}\n\nMore."), true),
            None
        );
    }

    #[test]
    fn rewrite_damage_lets_an_honest_cjk_shortening_through() {
        let page = format!("## 搜尋\n\n{ZH}\n\n## 排序\n\n{ZH}\n\n## 其他\n\n{ZH}");
        // Two of three sections cut: CJK falls by two thirds, but so does the page.
        let shorter = format!("## 搜尋\n\n{ZH}");
        assert_eq!(rewrite_damage(&page, &shorter, true), None);
        // Mixed page that adds English keeps its Chinese.
        let added = format!("{page}\n\nA short English note.");
        assert_eq!(rewrite_damage(&page, &added, true), None);
        // Too little CJK to judge.
        assert_eq!(
            rewrite_damage("Uses 搜尋 here.", "Uses search here.", true),
            None
        );
    }

    #[test]
    fn cjk_count_skips_punctuation_and_counts_kana_and_hangul() {
        assert_eq!(cjk_count("中文，「引號」。"), 4);
        assert_eq!(cjk_count("ひらがなカタカナ한국어"), 11);
        assert_eq!(cjk_count("plain ascii"), 0);
    }

    #[test]
    fn half_wiki_link_finds_an_unclosed_link() {
        assert_eq!(
            half_wiki_link("See [[mem_e5c607eb6"),
            Some("[[mem_e5c607eb6")
        );
        assert_eq!(
            half_wiki_link("A [[mem_aaa and [[mem_bbb]] later."),
            Some("[[mem_aaa and ")
        );
        assert_eq!(half_wiki_link("A [[mem_aaa]] and [[Page|label]]."), None);
        // A link never spans lines.
        assert_eq!(half_wiki_link("A [[mem_aaa\n]] next"), Some("[[mem_aaa"));
        assert_eq!(half_wiki_link("no links"), None);
        // `[[` quoted in inline code is text, but a half link beside it counts.
        assert_eq!(half_wiki_link("Type `[[` to link a note."), None);
        assert_eq!(half_wiki_link("Type `[[` then [[mem_ab"), Some("[[mem_ab"));
    }

    // ── body_shrink_ok ────────────────────────────────────────────────────────

    #[test]
    fn shrink_ok_at_floor_exactly() {
        // 7 >= 10 * 0.7 = 7.0 → true (at the floor, not below)
        assert!(body_shrink_ok("aaaaaaaaaa", "aaaaaaa", 0.7));
    }

    #[test]
    fn shrink_ok_below_floor() {
        // 6 < 10 * 0.7 = 7.0 → false
        assert!(!body_shrink_ok("aaaaaaaaaa", "aaaaaa", 0.7));
    }

    #[test]
    fn shrink_ok_growth_always_passes() {
        assert!(body_shrink_ok("short", "a much much longer body", 0.7));
    }

    #[test]
    fn shrink_ok_empty_old_is_always_true() {
        assert!(body_shrink_ok("", "anything", 0.7));
        assert!(body_shrink_ok("", "", 0.7));
    }

    #[test]
    fn shrink_ok_multibyte_utf8_uses_char_count_not_bytes() {
        // "ααααα" = 5 chars, 10 bytes; "αααα" = 4 chars, 8 bytes.
        // 4 >= 5 * 0.7 = 3.5 → true by char count.
        let old = "ααααα";
        let new_str = "αααα";
        assert_eq!(old.chars().count(), 5, "pin: old is 5 chars");
        assert_eq!(old.len(), 10, "pin: old is 10 bytes (2 bytes per α)");
        assert_eq!(new_str.chars().count(), 4, "pin: new is 4 chars");
        assert_eq!(new_str.len(), 8, "pin: new is 8 bytes");
        // 4 chars >= 5 chars * 0.7 = 3.5 → true
        assert!(body_shrink_ok(old, new_str, 0.7));
        // threshold=0.9: 4 < 5*0.9=4.5 → false (pinned to char count)
        assert!(!body_shrink_ok(old, new_str, 0.9));
    }

    #[test]
    fn shrink_ok_threshold_one_rejects_any_shrink() {
        assert!(!body_shrink_ok("hello", "hell", 1.0));
        assert!(body_shrink_ok("hello", "hello", 1.0));
        assert!(body_shrink_ok("hello", "hello world", 1.0));
    }

    #[test]
    fn shrink_ok_threshold_zero_accepts_everything() {
        assert!(body_shrink_ok("hello world this is long", "x", 0.0));
        assert!(body_shrink_ok("hello", "", 0.0));
    }

    // ── find_replace_guarded ─────────────────────────────────────────────────

    #[test]
    fn find_replace_guarded_expected_count_matches() {
        let content = "foo bar foo baz foo";
        let result = find_replace_guarded(content, "foo", "qux", 3).unwrap();
        assert_eq!(result, "qux bar qux baz qux");
    }

    #[test]
    fn find_replace_guarded_zero_occurrences_expected_zero() {
        let content = "hello world";
        let result = find_replace_guarded(content, "zzz", "qqq", 0).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn find_replace_guarded_wrong_count_returns_none() {
        let content = "foo bar foo baz foo";
        assert!(find_replace_guarded(content, "foo", "qux", 2).is_none());
    }

    #[test]
    fn find_replace_guarded_zero_actual_nonzero_expected_returns_none() {
        let content = "hello world";
        assert!(find_replace_guarded(content, "zzz", "qqq", 1).is_none());
    }

    #[test]
    fn find_replace_guarded_empty_find_returns_none() {
        assert!(find_replace_guarded("hello", "", "x", 0).is_none());
        assert!(find_replace_guarded("hello", "", "x", 1).is_none());
    }

    #[test]
    fn find_replace_guarded_multibyte_utf8() {
        let content = "αβγ αβγ";
        let result = find_replace_guarded(content, "αβγ", "xyz", 2).unwrap();
        assert_eq!(result, "xyz xyz");
    }

    #[test]
    fn find_replace_guarded_single_occurrence() {
        let content = "The quick brown fox";
        let result = find_replace_guarded(content, "fox", "cat", 1).unwrap();
        assert_eq!(result, "The quick brown cat");
    }

    // ── substitute_terms ─────────────────────────────────────────────────────

    #[test]
    fn substitute_terms_applies_in_order() {
        // Pass 1 ("foo"→"bar"): "foo and bar" → "bar and bar"
        // Pass 2 ("bar"→"baz"): "bar and bar" → "baz and baz"
        let subs = vec![
            ("foo".to_string(), "bar".to_string()),
            ("bar".to_string(), "baz".to_string()),
        ];
        let result = substitute_terms("foo and bar", &subs);
        assert_eq!(result, "baz and baz");
    }

    #[test]
    fn substitute_terms_empty_pairs_unchanged() {
        let result = substitute_terms("hello world", &[]);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn substitute_terms_no_match_unchanged() {
        let subs = vec![("zzz".to_string(), "qqq".to_string())];
        let result = substitute_terms("hello world", &subs);
        assert_eq!(result, "hello world");
    }

    #[test]
    fn substitute_terms_multibyte_utf8() {
        let subs = vec![("αβγ".to_string(), "xyz".to_string())];
        let result = substitute_terms("αβγ and αβγ", &subs);
        assert_eq!(result, "xyz and xyz");
    }

    #[test]
    fn substitute_terms_overlapping_sequential_semantics() {
        // Sequential semantics: each pair is a full-pass replacement.
        // Pass 1 ("FOO"=>"BAR"): "FOO and BAR" => "BAR and BAR"
        // Pass 2 ("BAR"=>"BAZ"): "BAR and BAR" => "BAZ and BAZ"
        // (Uppercase avoids substring collision with "and".)
        let subs = vec![
            ("FOO".to_string(), "BAR".to_string()),
            ("BAR".to_string(), "BAZ".to_string()),
        ];
        let result = substitute_terms("FOO and BAR", &subs);
        assert_eq!(result, "BAZ and BAZ");
    }
}
