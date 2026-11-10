// SPDX-License-Identifier: AGPL-3.0-only

/**
 * A page that keeps an original (`creation_kind = "original"`) is, in its
 * Markdown file: one line saying where it came from, the original itself
 * (HTML and SVG in a fenced block, Markdown as it is), then any notes added
 * later (UX-ART-1..3). The app shows the original as it was and the notes
 * below it; the first line is replaced by the source line it draws itself.
 */
export interface OriginalParts {
  /** What renders as the page it was: HTML or SVG. Null for Markdown or code. */
  readonly frame: { readonly kind: "html" | "svg"; readonly source: string } | null;
  /** Markdown to show where the frame is not (a Markdown original, or code). */
  readonly body: string;
  /** Notes added after the original. */
  readonly notes: string;
}

const PROVENANCE = /^(Kept from |Saved from |Saved by )/;

export function splitOriginal(content: string): OriginalParts {
  let rest = content.replace(/\r\n/g, "\n");
  const firstBreak = rest.indexOf("\n\n");
  const firstParagraph = firstBreak === -1 ? rest : rest.slice(0, firstBreak);
  if (PROVENANCE.test(firstParagraph)) rest = firstBreak === -1 ? "" : rest.slice(firstBreak + 2);

  const open = /^(`{3,})([\w+-]*)[ \t]*\n/.exec(rest);
  if (!open) return { frame: null, body: rest.trim(), notes: "" };
  const [opening, fence, language] = open;
  const closeAt = rest.indexOf(`\n${fence}`, opening.length - 1);
  if (closeAt === -1) return { frame: null, body: rest.trim(), notes: "" };
  const source = rest.slice(opening.length, closeAt + 1);
  const afterFence = rest.slice(closeAt + 1 + fence.length);
  // The closing fence must stand alone on its line.
  if (afterFence && !/^[ \t]*(\n|$)/.test(afterFence)) return { frame: null, body: rest.trim(), notes: "" };
  const notes = afterFence.replace(/^[ \t]*\n?/, "").trim();
  const kind = language.toLowerCase();
  if (kind === "html" || kind === "svg") {
    return { frame: { kind, source }, body: "", notes };
  }
  return { frame: null, body: rest.slice(0, closeAt + 1 + fence.length).trim(), notes };
}

/**
 * The words a person reads on an original's page: the original when it is
 * Markdown, then the notes. Never its first line (it names an internal file)
 * nor the HTML, SVG or code it keeps, which are not prose. Empty when there
 * is nothing else.
 */
export function originalProse(content: string): string {
  const { frame, body, notes } = splitOriginal(content);
  const prose = frame || /^`{3,}/.test(body) ? "" : body;
  return [prose, notes].filter(Boolean).join("\n\n");
}

/**
 * The page text a summary or excerpt is drawn from, so every card, list and
 * preview shows an original's real words rather than where it came from.
 * Other pages are returned as they are.
 */
export function readablePageContent(page: { readonly creation_kind?: string | null; readonly content?: string | null }): string {
  const content = page.content ?? "";
  return page.creation_kind === "original" ? originalProse(content) : content;
}

/** Reports the document's height to the page showing it, so the frame fits. */
const FIT_SCRIPT = `<script>(function(){function h(){var b=document.body;if(!b)return;var s=getComputedStyle(b);var v=b.scrollHeight+parseFloat(s.marginTop||0)+parseFloat(s.marginBottom||0);parent.postMessage({wenlanOriginalHeight:v},"*");}addEventListener("load",h);if(window.ResizeObserver&&document.body){new ResizeObserver(h).observe(document.body);}setTimeout(h,0);setTimeout(h,500);})();</script>`;

/** The document the frame loads: the original as it is, plus the fit script. */
export function frameDocument(frame: NonNullable<OriginalParts["frame"]>): string {
  if (frame.kind === "svg") {
    return `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0}svg{max-width:100%;height:auto;display:block}</style></head><body>${frame.source}${FIT_SCRIPT}</body></html>`;
  }
  const closing = frame.source.search(/<\/body\s*>/i);
  if (closing === -1) return `${frame.source}${FIT_SCRIPT}`;
  return `${frame.source.slice(0, closing)}${FIT_SCRIPT}${frame.source.slice(closing)}`;
}
