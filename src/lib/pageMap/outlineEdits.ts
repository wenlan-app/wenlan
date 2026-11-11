// SPDX-License-Identifier: AGPL-3.0-only
//
// Edits made on a page's mind map, applied to the page's own Markdown.
//
// The boxes drawn from the page are the page, so renaming one, adding one or
// deleting one changes the page itself. The daemon says which body line each
// box was read from (`PageOutlineNode.line`); these functions change that line
// and the lines that belong with it, and nothing else. The caller writes the
// result back with the page version the map was read from, so an edit made
// against a page that has since changed is refused rather than misplaced.

/** What kind of Markdown line stands behind a box, as far as editing goes. */
export type LineShape =
  | { kind: "heading"; level: number }
  | { kind: "list"; indent: string; marker: string; checkbox: string }
  | { kind: "bold"; indent: string };

export type OutlineEdit =
  | { op: "rename"; line: number; text: string }
  /** A new item at the end of a heading's own content. */
  | { op: "addUnder"; line: number; text: string }
  /** A new item right after this item, or a new heading after this heading's section. */
  | { op: "addAfter"; line: number; text: string }
  | { op: "delete"; line: number }
  /**
   * Move a line and everything that belongs with it next to another line, or
   * into a heading. Items move among items, headings among headings; a heading
   * moved under another heading becomes one level deeper.
   */
  | { op: "move"; line: number; target: number; place: MovePlace };

/** Where a moved line lands relative to its target. */
export type MovePlace = "before" | "after" | "under";

const HEADING = /^ {0,3}(#{1,6})[ \t]+(.*?)[ \t]*(?:#+[ \t]*)?$/;
const LIST = /^( {0,3})([-*+]|\d{1,9}[.)])([ \t]+)(\[[ xX]\][ \t]+)?(.*)$/;
const BOLD = /^( {0,3})(\*\*|__)(.+?)\2(.*)$/;
const FENCE = /^ {0,3}(```|~~~)/;

/** The shape of `raw`, or null when the line cannot be edited from the map. */
export function lineShape(raw: string | undefined): LineShape | null {
  if (raw === undefined) return null;
  const heading = HEADING.exec(raw);
  if (heading) return { kind: "heading", level: heading[1].length };
  const list = LIST.exec(raw);
  if (list) return { kind: "list", indent: list[1], marker: list[2] + list[3], checkbox: list[4] ?? "" };
  if (BOLD.test(raw)) return { kind: "bold", indent: BOLD.exec(raw)![1] };
  return null;
}

/**
 * The words a rename replaces on `raw`: a heading's text, a list item's text
 * (or its bold lead), a bold lead's words. Null when the line is not editable.
 */
export function editableText(raw: string | undefined): string | null {
  const shape = lineShape(raw);
  if (!shape || raw === undefined) return null;
  if (shape.kind === "heading") return HEADING.exec(raw)![2];
  if (shape.kind === "list") {
    const rest = LIST.exec(raw)![5];
    const lead = BOLD.exec(rest);
    return lead ? lead[3] : rest;
  }
  return BOLD.exec(raw)![3];
}

/** One line, flattened so a typed name cannot start a new block. */
function oneLine(text: string): string {
  return text.replace(/\s+/g, " ").trim();
}

/** Lines that are headings, ignoring anything inside a fenced code block. */
function headingLevels(lines: readonly string[]): Map<number, number> {
  const out = new Map<number, number>();
  let fenced = false;
  lines.forEach((raw, i) => {
    if (FENCE.test(raw)) {
      fenced = !fenced;
      return;
    }
    if (fenced) return;
    const m = HEADING.exec(raw);
    if (m) out.set(i, m[1].length);
  });
  return out;
}

/** First line after `from` that is a heading at `level` or above (any level when null). */
function nextHeading(lines: readonly string[], from: number, level: number | null): number {
  const headings = headingLevels(lines);
  for (let i = from + 1; i < lines.length; i++) {
    const at = headings.get(i);
    if (at !== undefined && (level === null || at <= level)) return i;
  }
  return lines.length;
}

/** Where an item's own block ends: its line, its wrapped and nested lines. */
function itemEnd(lines: readonly string[], line: number, shape: LineShape): number {
  let i = line + 1;
  if (shape.kind === "bold") {
    while (i < lines.length && lines[i].trim() !== "" && !HEADING.test(lines[i])) i++;
    return i;
  }
  const own = shape.kind === "list" ? shape.indent.length : 0;
  while (i < lines.length) {
    const raw = lines[i];
    if (raw.trim() === "") {
      // A blank line belongs to the item only when its nested lines go on.
      const next = lines[i + 1];
      if (next !== undefined && next.trim() !== "" && indentOf(next) > own) {
        i++;
        continue;
      }
      break;
    }
    if (indentOf(raw) <= own) break;
    i++;
  }
  return i;
}

function indentOf(raw: string): number {
  return raw.length - raw.trimStart().length;
}

/** The next marker in an ordered list, or the same bullet. */
function nextMarker(marker: string): string {
  const m = /^(\d+)([.)])(\s+)$/.exec(marker);
  return m ? `${Number(m[1]) + 1}${m[2]}${m[3]}` : marker;
}

/**
 * The body with `edit` applied, or null when the line behind it is not one
 * the map can edit (a table row, a paragraph, a line that has moved).
 */
export function applyOutlineEdit(body: string, edit: OutlineEdit): string | null {
  const newline = body.includes("\r\n") ? "\r\n" : "\n";
  const lines = body.split(/\r?\n/);
  const raw = lines[edit.line];
  const shape = lineShape(raw);
  if (!shape) return null;

  if (edit.op === "move") {
    const moved = moveBlock(lines, edit.line, shape, edit.target, edit.place);
    return moved ? moved.join(newline) : null;
  }

  if (edit.op === "rename") {
    const text = oneLine(edit.text);
    if (!text) return null;
    if (shape.kind === "heading") {
      lines[edit.line] = `${"#".repeat(shape.level)} ${text}`;
    } else if (shape.kind === "list") {
      const rest = LIST.exec(raw)![5];
      const lead = BOLD.exec(rest);
      // "- **Lead.** the rest" keeps the rest; the box shows only the lead.
      lines[edit.line] = lead
        ? `${shape.indent}${shape.marker}${shape.checkbox}${lead[2]}${text}${lead[2]}${lead[4]}`
        : `${shape.indent}${shape.marker}${shape.checkbox}${text}`;
    } else {
      const m = BOLD.exec(raw)!;
      lines[edit.line] = `${m[1]}${m[2]}${text}${m[2]}${m[4]}`;
    }
    return lines.join(newline);
  }

  if (edit.op === "delete") {
    const end = shape.kind === "heading"
      ? nextHeading(lines, edit.line, shape.level)
      : itemEnd(lines, edit.line, shape);
    lines.splice(edit.line, end - edit.line);
    // Two blank lines left where the block was become one.
    if (edit.line > 0 && lines[edit.line - 1]?.trim() === "" && lines[edit.line]?.trim() === "") {
      lines.splice(edit.line, 1);
    }
    return lines.join(newline);
  }

  const text = oneLine(edit.text);
  if (!text) return null;

  if (edit.op === "addAfter") {
    if (shape.kind !== "heading") return addItemAfter(lines, edit.line, shape, text, newline);
    // A new heading at the same level, after everything this one holds.
    const end = nextHeading(lines, edit.line, shape.level);
    let at = end;
    while (at > edit.line + 1 && lines[at - 1].trim() === "") at--;
    const block = ["", `${"#".repeat(shape.level)} ${text}`];
    if (end < lines.length) block.push("");
    lines.splice(at, end - at, ...block);
    return lines.join(newline);
  }


  // addUnder: only a heading holds items.
  if (shape.kind !== "heading") return null;
  const own = nextHeading(lines, edit.line, null);
  // Join the list already there, if the heading's own content ends in one.
  let last = own;
  while (last > edit.line + 1 && lines[last - 1].trim() === "") last--;
  for (let i = last - 1; i > edit.line; i--) {
    const item = lineShape(lines[i]);
    if (item?.kind === "list" && item.indent === "") {
      const end = itemEnd(lines, i, item);
      if (end >= last) {
        lines.splice(end, 0, `${nextMarker(item.marker)}${item.checkbox ? "[ ] " : ""}${text}`);
        return lines.join(newline);
      }
      break;
    }
    if (indentOf(lines[i]) === 0 && lines[i].trim() !== "") break;
  }
  const block = ["", `- ${text}`];
  if (last < lines.length && lines[last]?.trim() !== "") block.push("");
  lines.splice(last, 0, ...block);
  return lines.join(newline);
}

/** A new item right after this one, in the same list or as the next bold lead. */
function addItemAfter(
  lines: string[],
  line: number,
  shape: Exclude<LineShape, { kind: "heading" }>,
  text: string,
  newline: string,
): string {
  const end = itemEnd(lines, line, shape);
  const added =
    shape.kind === "list"
      ? `${shape.indent}${nextMarker(shape.marker)}${shape.checkbox ? "[ ] " : ""}${text}`
      : `${shape.indent}**${text}**`;
  // A paragraph item needs a blank line on each side to stay its own paragraph.
  lines.splice(end, 0, ...(shape.kind === "list" ? [added] : ["", added]));
  return lines.join(newline);
}

/** Where the block a line owns ends: a heading's section, an item's lines. */
function blockEnd(lines: readonly string[], line: number, shape: LineShape): number {
  return shape.kind === "heading" ? nextHeading(lines, line, shape.level) : itemEnd(lines, line, shape);
}

/** Drop blank lines at both ends of a block. */
function trimBlank(block: string[]): string[] {
  let start = 0;
  let end = block.length;
  while (start < end && block[start].trim() === "") start++;
  while (end > start && block[end - 1].trim() === "") end--;
  return block.slice(start, end);
}

/** Where a heading's own content ends: before its first subheading or the next heading. */
function ownContentEnd(lines: readonly string[], heading: number): number {
  let end = nextHeading(lines, heading, null);
  while (end > heading + 1 && lines[end - 1].trim() === "") end--;
  return end;
}

/** Collapse a run of blank lines around `at` to one, and none at the very top. */
function squeezeBlanks(lines: string[], at: number): void {
  let start = Math.min(at, lines.length);
  while (start > 0 && lines[start - 1].trim() === "") start--;
  let end = start;
  while (end < lines.length && lines[end].trim() === "") end++;
  if (end - start > 1) lines.splice(start + 1, end - start - 1);
}

/** Re-level every heading in a heading's block by the same step. */
function relevel(block: string[], delta: number): string[] {
  if (delta === 0) return block;
  let fenced = false;
  return block.map((raw) => {
    if (FENCE.test(raw)) fenced = !fenced;
    if (fenced) return raw;
    const m = HEADING.exec(raw);
    if (!m) return raw;
    const level = Math.min(6, Math.max(1, m[1].length + delta));
    return `${"#".repeat(level)} ${m[2]}`;
  });
}

/** A list item's block given a new indent and marker, its nested lines shifted along. */
function reindentItem(block: string[], from: Extract<LineShape, { kind: "list" }>, indent: string, marker: string): string[] {
  const first = LIST.exec(block[0])!;
  const shift = indent.length - from.indent.length;
  const head = `${indent}${marker}${first[4] ?? ""}${first[5]}`;
  const rest = block.slice(1).map((raw) => {
    if (raw.trim() === "") return raw;
    if (shift >= 0) return " ".repeat(shift) + raw;
    const cut = Math.min(-shift, indentOf(raw));
    return raw.slice(cut);
  });
  return [head, ...rest];
}

function moveBlock(
  source: readonly string[],
  line: number,
  shape: LineShape,
  target: number,
  place: MovePlace,
): string[] | null {
  const targetShape = lineShape(source[target]);
  if (!targetShape || target === line) return null;
  const end = blockEnd(source, line, shape);
  // A line cannot go inside what it carries along.
  if (target > line && target < end) return null;
  if (shape.kind === "heading" && targetShape.kind !== "heading") return null;
  if (shape.kind !== "heading" && targetShape.kind !== "heading" && place === "under") return null;
  if (shape.kind !== "heading" && targetShape.kind === "heading" && place !== "under") return null;
  if (shape.kind !== "heading" && targetShape.kind !== "heading" && shape.kind !== targetShape.kind) return null;

  let block = trimBlank(source.slice(line, end));
  const lines = source.slice();
  lines.splice(line, end - line);
  squeezeBlanks(lines, line);
  // The target's own line, now that the block is out.
  const removed = source.length - lines.length;
  const at = target > line ? target - removed : target;
  if (lineShape(lines[at])?.kind !== targetShape.kind) return null;

  if (shape.kind === "heading" && targetShape.kind === "heading") {
    const level = place === "under" ? targetShape.level + 1 : targetShape.level;
    if (level > 6) return null;
    block = relevel(block, level - shape.level);
    const insertAt = place === "before" ? at : nextHeading(lines, at, targetShape.level);
    return insertBlock(lines, insertAt, block, true);
  }

  if (place === "under" && targetShape.kind === "heading") {
    const ownEnd = ownContentEnd(lines, at);
    if (shape.kind === "list") {
      // Join the list the heading's own content ends in, as adding does.
      for (let i = ownEnd - 1; i > at; i--) {
        const item = lineShape(lines[i]);
        if (item?.kind === "list" && item.indent === "") {
          if (itemEnd(lines, i, item) >= ownEnd) {
            const joined = reindentItem(block, shape, "", nextMarker(item.marker));
            lines.splice(ownEnd, 0, ...joined);
            return lines;
          }
          break;
        }
        if (indentOf(lines[i]) === 0 && lines[i].trim() !== "") break;
      }
      return insertBlock(lines, ownEnd, reindentItem(block, shape, "", "- "), true);
    }
    return insertBlock(lines, ownEnd, block, true);
  }

  // An item next to another item of the same kind.
  if (shape.kind === "list" && targetShape.kind === "list") {
    const marker = place === "before" ? targetShape.marker : nextMarker(targetShape.marker);
    const moved = reindentItem(block, shape, targetShape.indent, marker);
    const insertAt = place === "before" ? at : itemEnd(lines, at, targetShape);
    lines.splice(insertAt, 0, ...moved);
    return lines;
  }
  const insertAt = place === "before" ? at : itemEnd(lines, at, targetShape);
  return insertBlock(lines, insertAt, block, true);
}

/** Put a block in as its own paragraph, a blank line on each side. */
function insertBlock(lines: string[], at: number, block: string[], spaced: boolean): string[] {
  if (!spaced) {
    lines.splice(at, 0, ...block);
    return lines;
  }
  const before = at > 0 && lines[at - 1].trim() !== "" ? [""] : [];
  const after = at < lines.length && lines[at].trim() !== "" ? [""] : [];
  lines.splice(at, 0, ...before, ...block, ...after);
  return lines;
}
