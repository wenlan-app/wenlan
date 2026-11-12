// SPDX-License-Identifier: AGPL-3.0-only
//
// Ticking a checklist item from a page's look (UX-LOOK-7): the look names the
// item by its text and which same-text item it is, and the app flips that
// one task line in the page's Markdown. The edit is the person's own,
// saved like any other and undoable.

const TASK_LINE = /^(\s*(?:[-*+]|\d+[.)])\s+\[)( |x|X)(\]\s+)(.*)$/;
const FENCE = /^\s*(```|~~~)/;

/** A task's text as a look sees it: links reduced to their words, no marks. */
export function plainTaskText(markdown: string): string {
  return markdown
    .replace(/\[\[([^\]|#]+)(?:#[^\]|]*)?(?:\|([^\]]+))?\]\]/g, (_, target: string, label?: string) => (label ?? target).trim())
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[*_`~]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

const CONTINUATION_STOP = /^\s*(?:$|[-*+]\s|\d+[.)]\s|#|>|```|~~~|\|)/;

/** A task's whole text: its first line and the lines that carry on from it. */
function itemText(lines: string[], start: number, first: string): string {
  let text = first;
  for (let j = start + 1; j < lines.length && !CONTINUATION_STOP.test(lines[j]); j++) text += ` ${lines[j].trim()}`;
  return text;
}

/**
 * `markdown` with the `occurrence`-th task (from 0) whose text is `text` set
 * to `done`, or null when there is no such task or it is already so.
 */
export function setTaskDone(markdown: string, text: string, occurrence: number, done: boolean): string | null {
  const want = plainTaskText(text);
  const lines = markdown.split("\n");
  let inFence = false;
  let seen = 0;
  for (let i = 0; i < lines.length; i++) {
    if (FENCE.test(lines[i])) { inFence = !inFence; continue; }
    if (inFence) continue;
    const match = TASK_LINE.exec(lines[i]);
    if (!match || plainTaskText(itemText(lines, i, match[4])) !== want) continue;
    if (seen++ !== occurrence) continue;
    const isDone = match[2] !== " ";
    if (isDone === done) return null;
    lines[i] = `${match[1]}${done ? "x" : " "}${match[3]}${match[4]}`;
    return lines.join("\n");
  }
  return null;
}
