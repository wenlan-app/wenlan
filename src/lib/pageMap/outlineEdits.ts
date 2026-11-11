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
  | { op: "delete"; line: number };

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
