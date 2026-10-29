// SPDX-License-Identifier: AGPL-3.0-only
/**
 * The example wiki in the shapes the daemon would answer with, in one app
 * language. Built once per language and kept.
 */
import type {
  Entity, EntityDetail, IndexedFileInfo, KnowledgeGraph, MemoryItem, Page, PageCitation,
  PageLinksResponse, PageSourceWithMemory, RegisteredSource, SearchResult, Space,
} from "../tauri";
import { EXAMPLE_WIKI } from "./content";
import type { ExampleSource, ExampleText } from "./types";

export type ExampleLocale = keyof ExampleText;

export interface ExampleWikiData {
  readonly pages: Page[];
  readonly memories: MemoryItem[];
  readonly spaces: Space[];
  readonly entities: Entity[];
  readonly entityDetails: Map<string, EntityDetail>;
  readonly registeredSources: RegisteredSource[];
  readonly indexedFiles: IndexedFileInfo[];
  readonly graph: KnowledgeGraph;
  readonly pageSources: Map<string, PageSourceWithMemory[]>;
  readonly pageLinks: Map<string, PageLinksResponse>;
}

const OCTOBER = Date.UTC(2026, 9, 1) / 1000;
const DAY = 86_400;
const at = (day: number) => OCTOBER + (day - 1) * DAY + 15 * 3600;
const iso = (day: number, minutesEarlier = 0) => new Date((at(day) - minutesEarlier * 60) * 1000).toISOString();
/** Pages were last written the day after the Oct 21 sync. */
const WRITTEN_DAY = 22;

/** The folder the notes, interviews and PDF came from. */
const NOTES_FOLDER = "~/Kestrel team";

const MEMORY_TYPE: Record<ExampleSource["kind"], string> = {
  notes: "fact", chat: "decision", session: "fact", pdf: "fact", web: "fact", interview: "fact",
};

/** A space's name in the app language; the content names spaces by id. */
function spaceName(id: string, locale: ExampleLocale): string {
  return EXAMPLE_WIKI.spaces.find((space) => space.id === id)?.name[locale] ?? id;
}

function memoryOf(source: ExampleSource, locale: ExampleLocale): MemoryItem {
  return {
    source_id: source.id,
    title: source.title[locale],
    content: source.body[locale],
    summary: null,
    source_text: source.body[locale],
    memory_type: MEMORY_TYPE[source.kind],
    domain: null,
    space: spaceName(source.space, locale),
    source_agent: source.agent,
    confidence: 0.95,
    confirmed: true,
    stability: "confirmed",
    pinned: false,
    supersedes: null,
    last_modified: at(source.day),
    chunk_count: 1,
  };
}

function fileOf(source: ExampleSource, locale: ExampleLocale): IndexedFileInfo {
  const title = source.title[locale];
  const web = source.kind === "web";
  const fileName = source.kind === "pdf" ? title : `${title}.md`;
  return {
    source: web ? "webpage" : source.kind === "chat" || source.kind === "session" ? "memory" : "file",
    source_id: web ? "https://example.com/budgetly/pricing" : source.kind === "chat" || source.kind === "session" ? source.id : `${NOTES_FOLDER}/${fileName}`,
    title,
    url: web ? "https://example.com/budgetly/pricing" : null,
    summary: source.body[locale],
    chunk_count: 1,
    last_modified: at(source.day),
    space: spaceName(source.space, locale),
    source_agent: source.agent,
    confirmed: true,
  };
}

export function buildExampleWiki(locale: ExampleLocale): ExampleWikiData {
  const memories = EXAMPLE_WIKI.sources.map((source) => memoryOf(source, locale));
  const memoryById = new Map(memories.map((memory) => [memory.source_id, memory]));
  const titleById = new Map(EXAMPLE_WIKI.pages.map((page) => [page.id, page.title[locale]]));
  const idByTitle = new Map(EXAMPLE_WIKI.pages.map((page) => [page.title[locale], page.id]));

  const pages: Page[] = [];
  const pageSources = new Map<string, PageSourceWithMemory[]>();
  const pageLinks = new Map<string, PageLinksResponse>();
  for (const [index, page] of EXAMPLE_WIKI.pages.entries()) {
    // Written one after another, the launch plan last, so it leads the list.
    const written = iso(WRITTEN_DAY, index * 4);
    const citations: PageCitation[] = [];
    const cited: string[] = [];
    const lines = [`# ${page.title[locale]}`];
    for (const section of page.sections) {
      if (section.heading) lines.push(`## ${section.heading[locale]}`);
      // Chinese sentences sit together without spaces; English ones with one.
      const join = locale === "en" ? " " : "";
      const paragraph = section.sentences.map((sentence) => {
        const markers = sentence.sources.map((sourceId) => {
          const marker = citations.length + 1;
          citations.push({
            occurrence: marker, marker, source_kind: "memory", locator: sourceId, score: 0.9,
            status: sentence.verified === false ? "unverified" : "verified", scope: "sentence",
          });
          if (!cited.includes(sourceId)) cited.push(sourceId);
          return `[${marker}]`;
        });
        const text = sentence.text[locale];
        // Chinese puts the source before the full stop, so it never runs into the next sentence.
        const stop = locale === "en" ? null : /[。！？]$/.exec(text);
        return stop ? `${text.slice(0, -1)}${markers.join("")}${stop[0]}` : `${text}${markers.join("")}`;
      }).join(join);
      lines.push(paragraph);
    }
    const content = `${lines.join("\n\n")}\n`;
    pages.push({
      id: page.id,
      title: page.title[locale],
      summary: null,
      content,
      entity_id: null,
      domain: null,
      space: spaceName(page.space, locale),
      source_memory_ids: cited,
      version: 1,
      status: "active",
      creation_kind: "distilled",
      created_at: written,
      last_compiled: written,
      last_modified: written,
      citations,
    });
    pageSources.set(page.id, cited.map((sourceId) => ({
      source: { page_id: page.id, memory_source_id: sourceId, linked_at: at(WRITTEN_DAY) },
      memory: memoryById.get(sourceId) ?? null,
    })));
    const outbound = [...content.matchAll(/\[\[([^\]]+)\]\]/g)].map((match) => ({
      label: match[1]!,
      target_page_id: idByTitle.get(match[1]!) ?? null,
      target_title: match[1]!,
    }));
    pageLinks.set(page.id, { outbound, inbound: [] });
  }
  // Inbound links are the outbound links read the other way.
  for (const [fromId, links] of pageLinks) {
    for (const link of links.outbound) {
      if (!link.target_page_id) continue;
      pageLinks.get(link.target_page_id)?.inbound.push({ source_page_id: fromId, label: titleById.get(fromId) ?? "" });
    }
  }

  const spaces: Space[] = EXAMPLE_WIKI.spaces.map((space, index) => ({
    id: `ex-space-${space.id.toLowerCase()}`,
    name: space.name[locale],
    description: space.description[locale],
    suggested: false,
    starred: false,
    sort_order: index,
    memory_count: EXAMPLE_WIKI.sources.filter((source) => source.space === space.id).length,
    entity_count: 0,
    created_at: at(1),
    updated_at: at(WRITTEN_DAY),
  }));

  const entities: Entity[] = EXAMPLE_WIKI.entities.map((entity) => ({
    id: entity.id,
    name: entity.name,
    entity_type: entity.type,
    domain: null,
    space: null,
    source_agent: null,
    confidence: 0.9,
    confirmed: true,
    created_at: at(3),
    updated_at: at(WRITTEN_DAY),
    memory_count: entity.pages.length,
    status: "established",
    established_by: null,
  }));
  const entityDetails = new Map<string, EntityDetail>(entities.map((entity) => [entity.id, { entity, observations: [], relations: [] }]));

  const registeredSources: RegisteredSource[] = [{
    id: "ex-source-team", source_type: "obsidian", path: NOTES_FOLDER, status: "Active",
    last_sync: at(WRITTEN_DAY), file_count: EXAMPLE_WIKI.sources.filter((source) => ["notes", "interview", "pdf"].includes(source.kind)).length,
    memory_count: memories.length,
  }];
  const indexedFiles = EXAMPLE_WIKI.sources.map((source) => fileOf(source, locale));

  const graph: KnowledgeGraph = {
    entities,
    relations: [],
    memories: memories.map((memory) => ({
      source_id: memory.source_id, title: memory.title, memory_type: memory.memory_type,
      space: memory.space ?? null, confirmed: true, last_modified: memory.last_modified,
    })),
    memory_links: [],
    pages: pages.map((page) => ({
      id: page.id, title: page.title, space: page.space ?? null, creation_kind: "distilled", entity_id: null, last_modified: page.last_modified,
    })),
    page_links: [
      ...[...pageLinks].flatMap(([fromId, links]) => links.outbound.filter((link) => link.target_page_id).map((link) => ({
        from: { kind: "page" as const, id: fromId }, to: { kind: "page" as const, id: link.target_page_id! }, link_type: "wikilink" as const,
      }))),
      ...EXAMPLE_WIKI.entities.flatMap((entity) => entity.pages.map((pageId) => ({
        from: { kind: "page" as const, id: pageId }, to: { kind: "entity" as const, id: entity.id }, link_type: "about" as const,
      }))),
    ],
  };

  return { pages, memories, spaces, entities, entityDetails, registeredSources, indexedFiles, graph, pageSources, pageLinks };
}

/** Pages and sources whose text holds every word of the query. */
export function searchExampleWiki(data: ExampleWikiData, query: string): SearchResult[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  const hit = (text: string) => words.every((word) => text.toLowerCase().includes(word));
  return data.memories.filter((memory) => hit(`${memory.title} ${memory.content}`)).map((memory, index) => ({
    id: `${memory.source_id}-0`, content: memory.content, source: "memory", source_id: memory.source_id, title: memory.title,
    url: null, chunk_index: 0, last_modified: memory.last_modified, score: 1 - index * 0.01,
    memory_type: memory.memory_type ?? undefined, space: memory.space, source_agent: memory.source_agent ?? undefined, confirmed: true,
  }));
}

export function searchExamplePages(data: ExampleWikiData, query: string): Page[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [];
  return data.pages.filter((page) => words.every((word) => `${page.title} ${page.content}`.toLowerCase().includes(word)));
}
