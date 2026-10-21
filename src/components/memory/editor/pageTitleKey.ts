// SPDX-License-Identifier: AGPL-3.0-only
// Plain text helpers with no editor imports, so the page view can use them
// without loading CodeMirror before the editor asks for it.

/**
 * The whitespace the daemon trims (Rust `str::trim`, Unicode White_Space).
 * JS `trim()` differs: it also strips U+FEFF and keeps U+0085.
 */
const DAEMON_EDGE_SPACE =
  /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+|[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]+$/g;

/** Trims `text` exactly as the daemon does before it compares a link label. */
export function trimLikeDaemon(text: string): string {
  return text.replace(DAEMON_EDGE_SPACE, "");
}

/** The daemon's page-title identity (`page_title_key`): trimmed, then lowercased. */
export function pageTitleKey(title: string): string {
  return trimLikeDaemon(title).toLowerCase();
}

/** The text a `[[` menu pick writes between the brackets: `Space/Title` for a note in another Space, else its title. */
export function linkCompletionText(note: { title: string; linkText?: string }): string {
  return trimLikeDaemon(note.linkText ?? note.title);
}
