// SPDX-License-Identifier: AGPL-3.0-only
// When a page is renamed, the daemon rewrites the [[links]] other pages make
// to it. A page open in another editor with unsaved text would then see a
// newer version it did not write. This module lets that editor recognise the
// change as the rename's own link rewrite, so it can carry the same rewrite
// into its draft instead of stopping on a conflict.
import { pageTitleKey, trimLikeDaemon } from "../editor/pageTitleKey";

export interface TextEdit {
  readonly from: number;
  readonly to: number;
  readonly insert: string;
  /** The whole link, `[[` to `]]`, this edit retargets. */
  readonly scope?: readonly [from: number, to: number];
}

/** The daemon's wikilink pattern (`sources::obsidian::WIKILINK_RE`). */
const WIKILINK = /(!?)\[\[([^\]|#]+)(?:#([^\]|]+))?(?:\|([^\]]+))?\]\]/g;
/** Characters that end or split a wikilink target (`WIKILINK_TARGET_BREAKERS`). */
const TARGET_BREAKERS = /[[\]|#]/;

/** A blank line starting here: LF or CRLF line endings. */
const BLANK_LINE = /^(?:\n\n|\r\n\r\n)/;

type Range = readonly [start: number, end: number];

/** Fenced code blocks, as `sources::obsidian::code_block_ranges` finds them. */
function fencedRanges(content: string): Range[] {
  const ranges: Range[] = [];
  let offset = 0;
  let open: { start: number; fence: string } | null = null;
  for (const line of content.split("\n")) {
    const trimmed = line.trimStart();
    if (!open) {
      if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
        open = { start: offset, fence: trimmed.slice(0, 3) };
      }
    } else if (trimmed.startsWith(open.fence)) {
      ranges.push([open.start, offset + line.length]);
      open = null;
    }
    offset += line.length + 1;
  }
  if (open) ranges.push([open.start, content.length]);
  return ranges;
}

const inside = (ranges: readonly Range[], at: number) =>
  ranges.find(([start, end]) => at >= start && at < end);

/** Inline code spans outside fences (`wikilinks::inline_code_ranges`). */
function inlineCodeRanges(content: string, fenced: readonly Range[]): Range[] {
  const ranges: Range[] = [];
  let at = 0;
  while (at < content.length) {
    const fence = inside(fenced, at);
    if (fence) { at = Math.max(fence[1], at + 1); continue; }
    if (content[at] === "\\") { at += 2; continue; }
    if (content[at] !== "`") { at += 1; continue; }
    const start = at;
    while (at < content.length && content[at] === "`") at += 1;
    const run = at - start;
    let scan = at;
    let close: number | null = null;
    while (scan < content.length) {
      // A blank line ends a span, whether the text uses LF or CRLF, so the
      // stored source and the editor's LF document read the same spans.
      if (inside(fenced, scan) || BLANK_LINE.test(content.slice(scan, scan + 4))) break;
      if (content[scan] !== "`") { scan += 1; continue; }
      const runStart = scan;
      while (scan < content.length && content[scan] === "`") scan += 1;
      if (scan - runStart === run) { close = scan; break; }
    }
    if (close !== null) { ranges.push([start, close]); at = close; }
  }
  return ranges;
}

/**
 * The edits that point every link naming `oldTitle` at `newTitle`, the way the
 * daemon's rename rewrite does for a bare `[[Old]]`, `[[Old|label]]`,
 * `[[Old#heading]]` or `![[Old]]`. Links in code, and escaped `\[[`, stay as
 * written. A `[[Space/Old]]` link is left alone; if the daemon rewrote one,
 * the result will not match and the editor falls back to its conflict.
 */
export function renamedLinkEdits(content: string, oldTitle: string, newTitle: string): TextEdit[] {
  const oldKey = pageTitleKey(oldTitle);
  const target = trimLikeDaemon(newTitle);
  if (!oldKey || !target || TARGET_BREAKERS.test(target)) return [];
  const fenced = fencedRanges(content);
  const inline = inlineCodeRanges(content, fenced);
  const edits: TextEdit[] = [];
  for (const match of content.matchAll(WIKILINK)) {
    const start = match.index;
    if (inside(fenced, start)) continue;
    const open = start + match[1].length;
    if (open > 0 && content[open - 1] === "\\") continue;
    if (inside(inline, open)) continue;
    const raw = match[2];
    const trimmed = trimLikeDaemon(raw);
    if (!trimmed || TARGET_BREAKERS.test(trimmed) || pageTitleKey(trimmed) !== oldKey) continue;
    if (trimmed === target) continue;
    const from = open + 2 + raw.indexOf(trimmed);
    edits.push({ from, to: from + trimmed.length, insert: target, scope: [start, start + match[0].length] });
  }
  return edits;
}

/** `content` with `edits` (ascending, non-overlapping) applied. */
export function applyTextEdits(content: string, edits: readonly TextEdit[]): string {
  let out = "";
  let copied = 0;
  for (const edit of edits) {
    out += content.slice(copied, edit.from) + edit.insert;
    copied = edit.to;
  }
  return out + content.slice(copied);
}

/** Every edit a rewrite makes to a document, computed from that document. */
export type LinkRewrite = (content: string) => TextEdit[];

interface RecordedRename {
  readonly oldTitle: string;
  readonly newTitle: string;
  readonly at: number;
}

/** How long a rename can still explain a page change another editor sees. */
const RENAME_WINDOW_MS = 5 * 60_000;
const recent: RecordedRename[] = [];

/** Remember a rename this app just made, before the pages it touched refresh. */
export function recordRenamedLinks(oldTitle: string, newTitle: string, now = Date.now()): void {
  if (pageTitleKey(oldTitle) === "" || oldTitle === newTitle) return;
  recent.push({ oldTitle, newTitle, at: now });
  while (recent.length > 8) recent.shift();
}

/** Forget every recorded rename (tests). */
export function clearRenamedLinks(): void {
  recent.length = 0;
}

/**
 * The rename rewrite that turns `baseline` into `canonical`, when a rename made
 * here in the last few minutes accounts for the whole difference; else null.
 * Anything else in the change (a real edit made elsewhere) returns null.
 */
export function renameRewriteExplaining(
  baseline: string,
  canonical: string,
  now = Date.now(),
): LinkRewrite | null {
  if (baseline === canonical) return null;
  // Newest first: the rename the person just made is the likeliest cause.
  for (const rename of [...recent].reverse()) {
    if (now - rename.at > RENAME_WINDOW_MS) continue;
    // The rewrite keeps answering for the rest of the window (an editor
    // replays it after undo), then stops: an old rename changes nothing.
    const deadline = Date.now() + (rename.at + RENAME_WINDOW_MS - now);
    const rewrite: LinkRewrite = (content) =>
      Date.now() > deadline ? [] : renamedLinkEdits(content, rename.oldTitle, rename.newTitle);
    const edits = rewrite(baseline);
    if (edits.length > 0 && applyTextEdits(baseline, edits) === canonical) return rewrite;
  }
  return null;
}
