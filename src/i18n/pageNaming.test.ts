// SPDX-License-Identifier: AGPL-3.0-only
// One name for a Wenlan page (docs/ux/spec.md UX-WORDS-1): "page/pages",
// 頁面 / 页面. Chinese says 空間 / 空间, never the English word "Space".
import { describe, expect, it } from "vitest";

import { resources } from "./resources";

function flatten(value: unknown, prefix = ""): Array<[string, string]> {
  if (typeof value === "string") return [[prefix, value]];
  if (!value || typeof value !== "object") return [];
  return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) =>
    flatten(child, prefix ? `${prefix}.${key}` : key),
  );
}

type Rule = "note" | "wiki" | "doc";
// Keys kept on purpose, each only for the word it was kept for: a key kept
// for "note" is still checked for "wiki", "Space" and 文章/文檔/文档.
const KEPT: Record<Rule, Record<string, string>> = {
  note: {
    // The person's raw material before Wenlan turns it into pages.
    "askAi.writePages.kinds.note_one": "count of imported notes, beside chats and files",
    "askAi.writePages.kinds.note_other": "count of imported notes, beside chats and files",
    "exampleWiki.line": "the example person's notes and chats that became pages",
    "setup.notesFirst.welcome.folder.hint": "files in the person's folder",
    "librarySync.phoneInbox": "quick notes a phone drops into the inbox",
    "repoWiki.consent": "pages citing the person's notes are kept out of a repo",
    "repoWiki.leftOut_one": "pages citing the person's notes are kept out of a repo",
    "repoWiki.leftOut_other": "pages citing the person's notes are kept out of a repo",
    "appearance.sample.row3": "sample page title in the appearance preview",
    // Notes that live in another app (Obsidian vault, Apple Notes, Zotero).
    "sourceAccess.linkLine1": "Obsidian notes",
    "sourceAccess.linkDoneBody": "Obsidian notes",
    "sourceAccess.askTitle": "Obsidian notes",
    "sourceContainer.vaultWrites": "notes in the person's vault",
    "sourceContainer.aiVaultHeading": "notes in the person's vault",
    "sourceContainer.aiFromFile": "notes in the person's vault",
    "sourceContainer.aiFromDefault": "notes in the person's vault",
    "sourceContainer.aiVaultDefault": "notes in the person's vault",
    "sourceContainer.aiPdfNote": "notes in the person's vault",
    "sourceContainer.suggestTitle": "notes in the person's vault",
    "sourceContainer.suggestAccepted": "notes in the person's vault",
    "sourceContainer.suggestStale": "notes in the person's vault",
    "sourceContainer.suggestFailed": "notes in the person's vault",
    "trayPanel.noteSuggestions_one": "notes in the person's vault",
    "trayPanel.noteSuggestions_other": "notes in the person's vault",
    "sourcePaste.noteTitle": "a note pasted from another app, e.g. Apple Notes",
    "pdfReader.failed": "Zotero item notes",
    "pdfReader.originalText": "Zotero item notes",
    // "note" as a remark, not a page.
    "sourceAccess.reflections": "the person's remarks on a source",
    "sourceAccess.reflectionLabel": "the person's remarks on a source",
    "sourceAccess.addReflection": "the person's remarks on a source",
    "findSkills.localHint_one": "the person's own remarks added to a skill",
    "findSkills.localHint_other": "the person's own remarks added to a skill",
    "aboutWenlan.releaseNotes": "release notes",
  },
  wiki: {
    "repoWiki.docs.notChecked": "names the /wenlan wiki command",
  },
  doc: {
    // Zotero papers and source-library documents are not Wenlan pages.
    "zoteroDisconnect.papers": "Zotero papers",
    "sourceAccess.zoteroHint": "Zotero papers",
    "zoteroSource.importSelected": "Zotero papers",
    "zoteroSource.chooseArticles": "Zotero papers",
    "zoteroSource.loadingArticles": "Zotero papers",
    "zoteroSource.articlesFailed": "Zotero papers",
    "zoteroSource.noArticles": "Zotero papers",
    "zoteroSource.articlesLabel": "Zotero papers",
    "sourceAccess.pickerFilter": "filter for documents among sources",
    "sourceLibrary.loadingDocuments": "documents in the source library",
    "sourceLibrary.documentError": "documents in the source library",
    "sourceLibrary.retryDocuments": "documents in the source library",
    "sourceLibrary.document": "documents in the source library",
    // The Docs kind next to Skills and Rules; its name is a pending decision.
    "sidebar.pages": "the Docs kind label, pending decision",
    "pages.overview.title": "reuses the Docs kind label, pending decision",
    "aboutWenlan.description": "names the Docs kind beside skills and rules",
  },
};


// 筆記憶 is 筆 (the measure word for memories) + 記憶, not 筆記.
const ZH_NOTE = /筆記(?!憶)|笔记/;
const ZH_DOC = /文章|文檔|文档/;
const LATIN_SPACE = /\bSpaces?\b/;
const EN_NOTE = /\bnotes?\b/i;
const WIKI = /\bwikis?\b/i;

type Locale = "en" | "zh-Hans" | "zh-Hant";
const LOCALES: readonly Locale[] = ["en", "zh-Hans", "zh-Hant"];

/** Strings in `locale` that `pattern` matches, minus the keys kept for `rule`. */
function offenders(locale: Locale, pattern: RegExp, rule?: Rule): string[] {
  const kept = rule ? KEPT[rule] : {};
  return flatten(resources[locale].translation)
    .filter(([key, text]) => !(key in kept) && pattern.test(text))
    .map(([key, text]) => `${key}: ${text}`);
}

describe("page naming", () => {
  it("UX-WORDS-1 ux_words_1: Chinese calls a page 頁面/页面 and a Space 空間/空间", () => {
    for (const locale of ["zh-Hans", "zh-Hant"] as const) {
      expect(offenders(locale, ZH_NOTE, "note"), `${locale} 筆記/笔记`).toEqual([]);
      expect(offenders(locale, ZH_DOC, "doc"), `${locale} 文章/文檔/文档`).toEqual([]);
      expect(offenders(locale, LATIN_SPACE), `${locale} Space`).toEqual([]);
      expect(offenders(locale, WIKI, "wiki"), `${locale} wiki`).toEqual([]);
    }
  });

  it("UX-WORDS-1 ux_words_1: English calls a page a page, not a note or a wiki", () => {
    expect(offenders("en", EN_NOTE, "note"), "en note").toEqual([]);
    expect(offenders("en", WIKI, "wiki"), "en wiki").toEqual([]);
  });

  it("UX-WORDS-1 ux_words_1: every kept key still exists and still needs keeping", () => {
    const patterns: Record<Rule, RegExp[]> = { note: [EN_NOTE, ZH_NOTE], wiki: [WIKI], doc: [ZH_DOC] };
    const texts = (key: string) =>
      LOCALES.map(locale => flatten(resources[locale].translation).find(([k]) => k === key)?.[1] ?? "");
    const stale = (Object.keys(KEPT) as Rule[]).flatMap(rule =>
      Object.keys(KEPT[rule])
        .filter(key => !texts(key).some(text => patterns[rule].some(pattern => pattern.test(text))))
        .map(key => `${rule}: ${key}`),
    );
    expect(stale).toEqual([]);
  });
});
