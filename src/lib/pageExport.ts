import { safePublicSourceUrl } from "./publicSourceUrl";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { processCitations, citationSourceKey, type ProcessedCitations } from "./pageCitations";
import type { Page, PageCitation } from "./tauri";
import type { PageExportSource } from "./pageExportApi";

export type PageExportFormat = "markdown" | "html" | "bibtex";
export interface ZoteroBibRecord { sourceId: string; title: string; bibtex: string }
export interface PageExportSnapshot {
  page: Pick<Page, "id" | "title" | "content" | "version" | "last_modified" | "citations">;
  sources: PageExportSource[];
  kind?: "saved" | "pending_draft";
  bibliographyIncomplete?: boolean;
  zoteroDocuments: ZoteroBibRecord[];
}

interface ExportReference {
  key: string;
  number?: number;
  title: string;
  url: string | null;
  localFile: boolean;
}
interface ExportReferences {
  items: ExportReference[];
  numberByOccurrence: Map<number, number>;
}

/** Export links can travel with a snapshot only when their target is public or a fragment. */
export function safeExportUrl(value: string): string {
  const candidate = value.trim();
  if (candidate.startsWith("#") && /^#[\w:.-]+$/u.test(candidate)) return candidate;
  if (!/^(?:https?:|mailto:)/iu.test(candidate)) return "";
  try {
    const parsed = new URL(candidate);
    if (parsed.username || parsed.password) return "";
    if (parsed.protocol === "mailto:") return parsed.href;
    return safePublicSourceUrl(candidate) ?? "";
  } catch { return ""; }
}

/** Drop local and relative destinations while keeping the author's visible label. */
export function removeUnsafeMarkdownTargets(markdown: string): string {
  return markdown
    .replace(/\]\(\s*(<[^>]*>|[^)\s]+)(?:\s+[^)]*)?\)/gu, (whole, raw: string) => {
      const target = raw.startsWith("<") && raw.endsWith(">") ? raw.slice(1, -1) : raw;
      return safeExportUrl(target) ? whole : "]";
    })
    .replace(/^\s*\[[^\]]+\]:\s*(\S+).*$/gmu, (whole, raw: string) => {
      const target = raw.startsWith("<") && raw.endsWith(">") ? raw.slice(1, -1) : raw;
      return safeExportUrl(target) ? whole : "";
    })
    .replace(/<((?:file:|\/Users\/|\/home\/|[A-Za-z]:[\\/]|\\\\)[^>]+)>/giu, "");
}

function sourceForLocator(snapshot: PageExportSnapshot, citation: PageCitation): PageExportSource | null {
  return snapshot.sources.find((source) => source.locator === citation.locator) ?? null;
}

function zoteroRecord(snapshot: PageExportSnapshot, sourceId: string | null | undefined): ZoteroBibRecord | undefined {
  if (!sourceId) return undefined;
  return snapshot.zoteroDocuments.find(record => record.sourceId === sourceId);
}

function bibtexUrl(snapshot: PageExportSnapshot, sourceId: string | null | undefined): string | null {
  const entry = zoteroRecord(snapshot, sourceId)?.bibtex ?? "";
  const match = /\burl\s*=\s*[{"']([^}"']+)[}"']/iu.exec(entry);
  return match ? safeExportUrl(match[1]) || null : null;
}

function references(snapshot: PageExportSnapshot, processed: ProcessedCitations): ExportReferences {
  const citations = [...processed.byOccurrence.entries()].sort(([a], [b]) => a - b);
  const byKey = new Map<string, ExportReference>();
  const numberByOccurrence = new Map<number, number>();
  for (const [occurrence, citation] of citations) {
    if (citation.source_kind === "authored") continue;
    const memory = sourceForLocator(snapshot, citation);
    const indexedZoteroSource = memory?.source_agent === "zotero" ? memory : null;
    // Several citation kinds can point at the same indexed Zotero item. Use
    // the stable item id as the export key so its bibliography appears once.
    const key = indexedZoteroSource ? `memory\u0000${indexedZoteroSource.source_id}` : citationSourceKey(citation);
    const existing = byKey.get(key);
    if (existing) {
      if (existing.number !== undefined) numberByOccurrence.set(occurrence, existing.number);
      continue;
    }
    const number = byKey.size + 1;
    if (citation.source_kind === "external_file" && !indexedZoteroSource) {
      byKey.set(key, { key, number, title: "Local file reference (path omitted)", url: null, localFile: true });
      numberByOccurrence.set(occurrence, number);
      continue;
    }
    const zotero = indexedZoteroSource ? zoteroRecord(snapshot, indexedZoteroSource.source_id) : undefined;
    const url = indexedZoteroSource
      ? safeExportUrl(indexedZoteroSource.url ?? "") || bibtexUrl(snapshot, indexedZoteroSource.source_id)
      : citation.source_kind === "external_url" ? safeExportUrl(citation.locator) || null : bibtexUrl(snapshot, memory?.source_id ?? citation.locator);
    byKey.set(key, { key, number, title: zotero?.title || memory?.title || (citation.source_kind === "external_url" ? safeExportUrl(citation.locator) || "Saved webpage" : "Saved source"), url, localFile: false });
    numberByOccurrence.set(occurrence, number);
  }
  for (const row of snapshot.sources) {
    const key = `memory\u0000${row.source_id}`;
    if (byKey.has(key)) continue;
    const zotero = zoteroRecord(snapshot, row.source_id);
    byKey.set(key, { key, title: zotero?.title || row.title || "Saved source", url: safeExportUrl(row.url ?? "") || bibtexUrl(snapshot, row.source_id), localFile: false });
  }
  return { items: [...byKey.values()].sort((a, b) => (a.number ?? Number.MAX_SAFE_INTEGER) - (b.number ?? Number.MAX_SAFE_INTEGER) || a.title.localeCompare(b.title)), numberByOccurrence };
}

function bodyWithMarkdownFootnotes(processed: ProcessedCitations, refs: ExportReference[], numberByOccurrence: Map<number, number>): string {
  const numberByCitation = new Map(refs.filter(ref => ref.number !== undefined).map(ref => [ref.number!, ref.number!]));
  const body = processed.content.replace(/\[(\d+)\]\(#citation:(\d+)\)/gu, (_match, occurrence: string) => {
    const number = numberByOccurrence.get(Number(occurrence));
    return number && numberByCitation.has(number) ? `[^wenlan-source-${number}]` : "";
  });
  return removeUnsafeMarkdownTargets(body);
}

function markdownReference(ref: ExportReference): string {
  if (ref.localFile) return ref.title;
  return ref.url ? `${ref.title} — ${ref.url}` : ref.title;
}

function omitLeadingTitle(markdown: string, title: string): string {
  const match = /^(?:\uFEFF)?# ([^\n]+)\n(?:\s*\n)?/u.exec(markdown);
  return match?.[1]?.trim() === title.trim() ? markdown.slice(match[0].length).trimStart() : markdown;
}

function serializeMarkdown(snapshot: PageExportSnapshot): string {
  const processed = processCitations(snapshot.page.content, snapshot.page.citations);
  const { items: refs, numberByOccurrence } = references(snapshot, processed);
  const date = snapshot.page.last_modified || "Unknown date";
  const sections = [
    `# ${snapshot.page.title}`,
    snapshot.kind === "pending_draft" ? `> Pending draft based on saved version ${snapshot.page.version} — not adopted · ${date}` : `> Exported from saved version ${snapshot.page.version} · ${date}`,
    bodyWithMarkdownFootnotes({ ...processed, content: omitLeadingTitle(processed.content, snapshot.page.title) }, refs, numberByOccurrence),
  ];
  if (snapshot.bibliographyIncomplete) sections.push("> Some bibliography entries were unavailable when this version was captured.");
  if (refs.length) sections.push(`## Sources\n\n${refs.map(ref => ref.number
    ? `[^wenlan-source-${ref.number}]: ${markdownReference(ref)}`
    : `- ${markdownReference(ref)}`).join("\n")}`);
  return `${sections.filter(Boolean).join("\n\n").trim()}\n`;
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");
}

function bodyWithHtmlCitations(processed: ProcessedCitations, refs: ExportReference[], numberByOccurrence: Map<number, number>): string {
  const numbers = new Set(refs.filter(ref => ref.number !== undefined).map(ref => ref.number));
  return removeUnsafeMarkdownTargets(processed.content).replace(/\[(\d+)\]\(#citation:(\d+)\)/gu, (_match, occurrence: string) => {
    const number = numberByOccurrence.get(Number(occurrence));
    return number && numbers.has(number) ? `[${number}](#citation-${number})` : "";
  });
}

function serializeHtml(snapshot: PageExportSnapshot): string {
  const processed = processCitations(snapshot.page.content, snapshot.page.citations);
  const { items: refs, numberByOccurrence } = references(snapshot, processed);
  const markdown = bodyWithHtmlCitations({ ...processed, content: omitLeadingTitle(processed.content, snapshot.page.title) }, refs, numberByOccurrence);
  const body = renderToStaticMarkup(createElement(ReactMarkdown, {
    remarkPlugins: [remarkGfm],
    urlTransform: safeExportUrl,
    components: { a: ({ href, children }: { href?: string; children?: React.ReactNode }) => href
      ? createElement("a", { href, ...(href.startsWith("#") ? {} : { rel: "noopener noreferrer" }) }, children)
      : createElement("span", null, children) },
  }, markdown));
  const sourceList = refs.length ? `<section><h2>Sources</h2><ol>${refs.map(ref => {
    const label = escapeHtml(ref.title);
    const target = ref.url ? `<a href="${escapeHtml(ref.url)}" rel="noopener noreferrer">${label}</a>` : label;
    return `<li${ref.number ? ` id="citation-${ref.number}"` : ""}>${target}</li>`;
  }).join("")}</ol></section>` : "";
  const date = escapeHtml(snapshot.page.last_modified || "Unknown date");
  const bibliographyNotice = snapshot.bibliographyIncomplete ? "<p>Some bibliography entries were unavailable when this version was captured.</p>" : "";
  const provenance = snapshot.kind === "pending_draft" ? `Pending draft based on saved version ${snapshot.page.version} — not adopted` : `Exported from saved version ${snapshot.page.version}`;
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${escapeHtml(snapshot.page.title)}</title></head><body><main><header><h1>${escapeHtml(snapshot.page.title)}</h1><p>${provenance} · ${date}</p></header><article>${body}</article>${bibliographyNotice}${sourceList}</main></body></html>\n`;
}

function stripLocalBibFields(value: string): string {
  return value.replace(/^\s*(?:file|localfile|pdf)\s*=\s*(?:\{[^}]*\}|"[^"]*"|[^,\n]+)\s*,?\s*$/gimu, "")
    .replace(/^\s*url\s*=\s*(?:\{([^}]*)\}|"([^"]*)")\s*,?\s*$/gimu, (line, braceValue: string | undefined, quoteValue: string | undefined) => {
      const target = braceValue ?? quoteValue ?? "";
      return safeExportUrl(target) ? line : "";
    })
    .replace(/(?:file:\/\/\/[^\s}"]+|\/Users\/[^\s}"]+|\/home\/[^\s}"]+|(?<![\w])[A-Za-z]:[\\/][^\s}"]+)/giu, "")
    .trim();
}

function serializeBibTeX(snapshot: PageExportSnapshot): string {
  const entries = snapshot.zoteroDocuments.map(record => stripLocalBibFields(record.bibtex)).filter(entry => /@\w+\s*\{/u.test(entry));
  const unique = [...new Set(entries)];
  if (!unique.length) throw new Error("This page has no Zotero bibliography entries to export.");
  return `${snapshot.kind === "pending_draft" ? `% Pending draft based on saved version ${snapshot.page.version} — not adopted\n\n` : ""}${unique.join("\n\n")}\n`;
}

export function serializePageExport(snapshot: PageExportSnapshot, format: PageExportFormat): string {
  if (format === "markdown") return serializeMarkdown(snapshot);
  if (format === "html") return serializeHtml(snapshot);
  return serializeBibTeX(snapshot);
}

export function pageExportFilename(title: string, format: PageExportFormat): string {
  const extension = format === "markdown" ? "md" : format === "html" ? "html" : "bib";
  const stem = title.normalize("NFKC").replace(/[^\p{L}\p{N} _.-]+/gu, "").trim().replace(/[ .]+/gu, "-").slice(0, 80) || "page";
  return `${stem}.${extension}`;
}
