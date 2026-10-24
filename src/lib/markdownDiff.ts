// SPDX-License-Identifier: AGPL-3.0-only
import { diffWords, type DiffSegment } from "./wordDiff";

/**
 * A diff of two Markdown documents that can still be rendered as Markdown.
 *
 * Both sides are cut into blocks (headings, paragraphs, list items, quotes,
 * code fences, tables). Unchanged blocks pass through as they are; a changed
 * block keeps its own Markdown prefix (`## `, `- `, `> `) and only its text is
 * word-diffed, so a heading stays a heading instead of showing its `#`. The
 * changed runs are wrapped in private-use marker characters, which
 * `rehypeDiffMarks` turns into `<ins>` / `<del>` after the Markdown is parsed.
 */

export const INS_OPEN = "";
export const INS_CLOSE = "";
export const DEL_OPEN = "";
export const DEL_CLOSE = "";

type BlockKind = "heading" | "paragraph" | "item" | "quote" | "code" | "table" | "rule";

interface Block {
  kind: BlockKind;
  /** Markdown before the text: `## `, `  - `, `1. `; empty for the rest. */
  prefix: string;
  /** The text the diff runs over (quote lines without their `> `). */
  body: string;
  /** Whole source of the block, used to tell blocks apart. */
  raw: string;
  /** A blank line separated it from the block before it. */
  blankBefore: boolean;
  /** Code fence: opening line, closing line, info string. */
  fence?: { open: string; close: string; lang: string };
}

const FENCE = /^(\s*)(`{3,}|~{3,})(.*)$/;
const HEADING = /^(#{1,6}[ \t]+)(.*)$/;
const ITEM = /^(\s*(?:[-*+]|\d{1,9}[.)])[ \t]+(?:\[[ xX]\][ \t]+)?)(.*)$/;
const QUOTE = /^\s*>/;
const RULE = /^\s*(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,})$/;
const TABLE = /^\s*\|/;

function startsBlock(line: string): boolean {
  return (
    FENCE.test(line) ||
    HEADING.test(line) ||
    ITEM.test(line) ||
    QUOTE.test(line) ||
    RULE.test(line) ||
    TABLE.test(line)
  );
}

export function splitBlocks(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, "\n").split("\n");
  const blocks: Block[] = [];
  let blank = false;
  let i = 0;
  const push = (block: Omit<Block, "blankBefore">) => {
    blocks.push({ ...block, blankBefore: blank && blocks.length > 0 });
    blank = false;
  };
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) {
      blank = true;
      i++;
      continue;
    }
    const fence = line.match(FENCE);
    if (fence) {
      const marker = fence[2];
      const body: string[] = [];
      let close = "";
      i++;
      while (i < lines.length) {
        const candidate = lines[i].trim();
        if (candidate.startsWith(marker[0].repeat(marker.length)) && !candidate.replace(/[`~]/g, "")) {
          close = lines[i];
          i++;
          break;
        }
        body.push(lines[i]);
        i++;
      }
      push({
        kind: "code",
        prefix: "",
        body: body.join("\n"),
        raw: [line, ...body, close].join("\n"),
        fence: { open: line, close: close || fence[1] + marker, lang: fence[3].trim() },
      });
      continue;
    }
    if (RULE.test(line)) {
      push({ kind: "rule", prefix: "", body: line, raw: line });
      i++;
      continue;
    }
    if (TABLE.test(line)) {
      const rows: string[] = [];
      while (i < lines.length && TABLE.test(lines[i])) rows.push(lines[i++]);
      const raw = rows.join("\n");
      push({ kind: "table", prefix: "", body: raw, raw });
      continue;
    }
    if (QUOTE.test(line)) {
      const rows: string[] = [];
      while (i < lines.length && QUOTE.test(lines[i])) rows.push(lines[i++]);
      push({
        kind: "quote",
        prefix: "> ",
        body: rows.map((row) => row.replace(/^\s*>[ \t]?/, "")).join("\n"),
        raw: rows.join("\n"),
      });
      continue;
    }
    const heading = line.match(HEADING);
    if (heading) {
      push({ kind: "heading", prefix: heading[1], body: heading[2], raw: line });
      i++;
      continue;
    }
    const item = line.match(ITEM);
    const rows = [item ? item[2] : line];
    const raw = [line];
    i++;
    // Continuation lines: anything that does not start a block of its own.
    while (i < lines.length && lines[i].trim() && !startsBlock(lines[i]) && !/^\s*=+\s*$/.test(lines[i])) {
      rows.push(lines[i].trim());
      raw.push(lines[i]);
      i++;
    }
    // `Title` over `===` or `---` is a heading, not a paragraph and a rule.
    const underline = !item && i < lines.length ? lines[i].match(/^\s*(=+|-+)\s*$/) : null;
    if (underline) {
      push({
        kind: "heading",
        prefix: underline[1][0] === "=" ? "# " : "## ",
        body: rows.map((row) => row.trim()).join(" "),
        raw: [...raw, lines[i]].join("\n"),
      });
      i++;
      continue;
    }
    push({
      kind: item ? "item" : "paragraph",
      prefix: item ? item[1] : "",
      body: rows.join("\n"),
      raw: raw.join("\n"),
    });
  }
  return blocks;
}

/** Words plus their leading space, with Markdown delimiters as tokens of their
 * own, so `**b c**` → `**b d**` changes only `c`. Joins reproduce the input. */
function markdownTokens(text: string): string[] {
  return text.match(/\s*(?:\*\*|__|~~|`+|\]\([^)\s]*\)|[*_~[\]]|[^\s*_~`[\]]+)|\s+$/g) ?? [];
}

export function diffMarkdownWords(before: string, after: string): DiffSegment[] {
  return diffWords(before, after, markdownTokens);
}

function lcsPairs<T>(a: T[], b: T[], same: (x: T, y: T) => boolean): [number, number][] {
  const n = a.length;
  const m = b.length;
  const table: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      table[i][j] = same(a[i], b[j]) ? table[i + 1][j + 1] + 1 : Math.max(table[i + 1][j], table[i][j + 1]);
  const pairs: [number, number][] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (same(a[i], b[j])) {
      pairs.push([i, j]);
      i++;
      j++;
    } else if (table[i + 1][j] >= table[i][j + 1]) i++;
    else j++;
  }
  return pairs;
}

const blockKey = (block: Block) => `${block.kind}\u0000${block.prefix.trim()}\u0000${block.body.trim()}`;

function words(text: string): Set<string> {
  return new Set(text.toLowerCase().match(/[\p{L}\p{N}]+/gu) ?? []);
}

/** Close enough that a word diff reads better than "removed, then added". */
function similar(a: Block, b: Block): boolean {
  if (a.kind !== b.kind) return false;
  if (a.kind === "rule") return true;
  const x = words(a.body);
  const y = words(b.body);
  if (x.size === 0 || y.size === 0) return x.size === y.size;
  let shared = 0;
  for (const word of x) if (y.has(word)) shared++;
  return shared / Math.max(x.size, y.size) >= 0.34;
}

type Step =
  | { op: "same"; block: Block }
  | { op: "del"; block: Block }
  | { op: "ins"; block: Block }
  | { op: "change"; before: Block; after: Block };

/** Block steps in reading order: unchanged, removed, added, or reworded. */
export function blockSteps(before: string, after: string): Step[] {
  const a = splitBlocks(before);
  const b = splitBlocks(after);
  const anchors = lcsPairs(a, b, (x, y) => blockKey(x) === blockKey(y));
  const steps: Step[] = [];
  let i = 0;
  let j = 0;
  const gap = (iEnd: number, jEnd: number) => {
    // Pair removed and added blocks in order where they still read alike.
    let k = j;
    while (i < iEnd) {
      let match = -1;
      for (let candidate = k; candidate < jEnd; candidate++) {
        if (similar(a[i], b[candidate])) {
          match = candidate;
          break;
        }
      }
      if (match < 0) {
        steps.push({ op: "del", block: a[i++] });
        continue;
      }
      while (k < match) steps.push({ op: "ins", block: b[k++] });
      steps.push({ op: "change", before: a[i++], after: b[k++] });
    }
    while (k < jEnd) steps.push({ op: "ins", block: b[k++] });
    j = jEnd;
  };
  for (const [ai, bj] of anchors) {
    gap(ai, bj);
    steps.push({ op: "same", block: b[bj] });
    i = ai + 1;
    j = bj + 1;
  }
  gap(a.length, b.length);
  return steps;
}

/** Wrap a run in markers, keeping its leading whitespace outside and each line
 * marked on its own so line starts keep their Markdown meaning. */
function mark(text: string, open: string, close: string): string {
  return text
    .split("\n")
    .map((line) => {
      const lead = line.match(/^\s*/)?.[0] ?? "";
      const rest = line.slice(lead.length);
      return rest ? `${lead}${open}${rest}${close}` : line;
    })
    .join("\n");
}

type Side = "unified" | "before" | "after";

function segmentsText(segments: DiffSegment[], side: Side): string {
  return segments
    .map((segment, index) => {
      if (segment.kind === "same") return segment.text;
      if (segment.kind === "ins") {
        if (side === "before") return "";
        // Shown together, a removed word and its replacement need a space
        // between them, or "5" and "2" read as "52".
        const gap = side === "unified" && segments[index - 1]?.kind === "del" && !/^\s/.test(segment.text) ? " " : "";
        return gap + mark(segment.text, INS_OPEN, INS_CLOSE);
      }
      return side === "after" ? "" : mark(segment.text, DEL_OPEN, DEL_CLOSE);
    })
    .join("");
}

function withPrefix(block: Block, body: string): string {
  if (block.kind === "quote")
    return body
      .split("\n")
      .map((line) => `> ${line}`)
      .join("\n");
  if (block.kind === "item" || block.kind === "paragraph") {
    // Continuation lines sit under the item's text.
    const indent = " ".repeat(block.prefix.length);
    return block.prefix + body.split("\n").join(`\n${indent}`);
  }
  return block.prefix + body;
}

function codeBlock(block: Block, lines: string): string {
  const fence = block.fence!;
  // A changed diagram shows its source; markers inside it would not parse.
  const open = fence.lang.toLowerCase() === "mermaid" ? fence.open.replace(/mermaid\s*$/i, "") : fence.open;
  return `${open}\n${lines}\n${fence.close}`;
}

function linesDiff(before: string[], after: string[], side: Side): string[] {
  const pairs = lcsPairs(before, after, (x, y) => x === y);
  const out: string[] = [];
  let i = 0;
  let j = 0;
  const flush = (iEnd: number, jEnd: number) => {
    if (side !== "after") while (i < iEnd) out.push(mark(before[i++], DEL_OPEN, DEL_CLOSE));
    if (side !== "before") while (j < jEnd) out.push(mark(after[j++], INS_OPEN, INS_CLOSE));
    i = iEnd;
    j = jEnd;
  };
  for (const [bi, aj] of pairs) {
    flush(bi, aj);
    out.push(after[aj]);
    i = bi + 1;
    j = aj + 1;
  }
  flush(before.length, after.length);
  return out;
}

const isSeparatorRow = (row: string) => /^\s*\|?[\s:|-]+\|?\s*$/.test(row) && row.includes("-");

/** Wrap each table cell's text, leaving the pipes alone. */
function markRow(row: string, open: string, close: string): string {
  return row.replace(/(\|)([^|]*)(?=\|)/g, (_all, pipe: string, cell: string) =>
    cell.trim() ? `${pipe}${cell.replace(/\S(?:.*\S)?/, (text) => `${open}${text}${close}`)}` : pipe + cell,
  );
}

const cells = (row: string) => row.trim().replace(/^\|/, "").replace(/\|$/, "").split("|");

/** One edited row with only its changed cells marked, or null when the rows
 * have different cells and are shown as removed and added instead. */
function cellDiff(before: string, after: string, side: Side): string | null {
  const a = cells(before);
  const b = cells(after);
  if (a.length !== b.length) return null;
  const row = b.map((cell, k) => {
    if (cell.trim() === a[k].trim()) return side === "before" ? a[k] : cell;
    return ` ${segmentsText(diffMarkdownWords(a[k].trim(), cell.trim()), side)} `;
  });
  return `|${row.join("|")}|`;
}

function tableDiff(before: Block | null, after: Block | null, side: Side): string {
  const oldRows = before ? before.body.split("\n") : [];
  const newRows = after ? after.body.split("\n") : [];
  if (!before || !after) {
    const rows = before ? oldRows : newRows;
    const [open, close] = before ? [DEL_OPEN, DEL_CLOSE] : [INS_OPEN, INS_CLOSE];
    return rows.map((row) => (isSeparatorRow(row) ? row : markRow(row, open, close))).join("\n");
  }
  const head = (rows: string[]) => (rows.length > 1 && isSeparatorRow(rows[1]) ? rows.slice(0, 2) : rows.slice(0, 1));
  const header = side === "before" ? head(oldRows) : head(newRows);
  const oldBody = oldRows.slice(head(oldRows).length);
  const newBody = newRows.slice(head(newRows).length);
  const pairs = lcsPairs(oldBody, newBody, (x, y) => x.trim() === y.trim());
  const out = [...header];
  let i = 0;
  let j = 0;
  const flush = (iEnd: number, jEnd: number) => {
    // As many rows out as in: the same rows edited, so mark only their changed cells.
    if (iEnd - i === jEnd - j) {
      while (i < iEnd) {
        const was = oldBody[i++];
        const now = newBody[j++];
        const row = cellDiff(was, now, side);
        if (row) out.push(row);
        else {
          if (side !== "after") out.push(markRow(was, DEL_OPEN, DEL_CLOSE));
          if (side !== "before") out.push(markRow(now, INS_OPEN, INS_CLOSE));
        }
      }
    }
    if (side !== "after") while (i < iEnd) out.push(markRow(oldBody[i++], DEL_OPEN, DEL_CLOSE));
    if (side !== "before") while (j < jEnd) out.push(markRow(newBody[j++], INS_OPEN, INS_CLOSE));
    i = iEnd;
    j = jEnd;
  };
  for (const [bi, aj] of pairs) {
    flush(bi, aj);
    out.push(side === "before" ? oldBody[bi] : newBody[aj]);
    i = bi + 1;
    j = aj + 1;
  }
  flush(oldBody.length, newBody.length);
  return out.join("\n");
}

function renderWhole(block: Block, open: string, close: string, side: Side): string {
  switch (block.kind) {
    case "code":
      return codeBlock(block, block.body.split("\n").map((line) => mark(line, open, close)).join("\n"));
    case "table":
      return tableDiff(open === DEL_OPEN ? block : null, open === INS_OPEN ? block : null, side);
    case "rule":
      return block.raw;
    default:
      return withPrefix(block, mark(block.body, open, close));
  }
}

function renderChange(before: Block, after: Block, side: Side): string {
  switch (after.kind) {
    case "code":
      return codeBlock(after, linesDiff(before.body.split("\n"), after.body.split("\n"), side).join("\n"));
    case "table":
      return tableDiff(before, after, side);
    case "rule":
      return after.raw;
    default: {
      const shown = side === "before" ? before : after;
      return withPrefix(shown, segmentsText(diffMarkdownWords(before.body, after.body), side));
    }
  }
}

/** Markdown for one view of the diff: both sides marked, or one side alone. */
export function markdownDiff(before: string, after: string, side: Side): string {
  const out: string[] = [];
  let previous: Block | null = null;
  const emit = (block: Block, markdown: string) => {
    if (previous) {
      // Keep each document's own spacing, and never glue two list items of
      // different lists or a paragraph onto a list item.
      const tight = !block.blankBefore && block.kind === "item" && previous.kind === "item";
      out.push(tight ? "\n" : "\n\n");
    }
    out.push(markdown);
    previous = block;
  };
  blockSteps(before, after).forEach((step, index) => {
    // An unchanged leading `# Title` repeats the title the dialog already
    // shows above the diff; a renamed one stays, marked.
    if (index === 0 && step.op === "same" && step.block.kind === "heading" && step.block.prefix.trim() === "#")
      return;
    if (step.op === "same") emit(step.block, step.block.raw);
    else if (step.op === "del") {
      if (side !== "after") emit(step.block, renderWhole(step.block, DEL_OPEN, DEL_CLOSE, side));
    } else if (step.op === "ins") {
      if (side !== "before") emit(step.block, renderWhole(step.block, INS_OPEN, INS_CLOSE, side));
    } else emit(side === "before" ? step.before : step.after, renderChange(step.before, step.after, side));
  });
  return out.join("");
}

interface HastLike {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastLike[];
}

const MARKERS = /([-])/;

/**
 * Rehype plugin: turn the marker characters into `<ins data-diff>` and
 * `<del data-diff>` around every piece of text between them, across element
 * boundaries (a run may open inside `**…**` and close after it).
 */
export function rehypeDiffMarks() {
  return (tree: HastLike) => {
    let mode: "ins" | "del" | null = null;
    const walk = (node: HastLike) => {
      if (!node.children) return;
      const next: HastLike[] = [];
      for (const child of node.children) {
        if (child.type !== "text" || !child.value) {
          walk(child);
          next.push(child);
          continue;
        }
        for (const piece of child.value.split(MARKERS)) {
          if (piece === INS_OPEN) mode = "ins";
          else if (piece === DEL_OPEN) mode = "del";
          else if (piece === INS_CLOSE || piece === DEL_CLOSE) mode = null;
          else if (piece) {
            const text: HastLike = { type: "text", value: piece };
            next.push(
              mode
                ? { type: "element", tagName: mode, properties: { dataDiff: mode }, children: [text] }
                : text,
            );
          }
        }
      }
      node.children = next;
    };
    walk(tree);
  };
}
