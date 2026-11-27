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

// Keys kept on purpose: there "note" / 筆記 is not a Wenlan page.
const KEPT: Record<string, string> = {
  // The person's raw material before Wenlan turns it into pages.
  "askAi.writePages.kinds.note_one": "count of imported notes, beside chats and files",
  "askAi.writePages.kinds.note_other": "count of imported notes, beside chats and files",
  "exampleWiki.line": "the example person's notes and chats that became pages",
  "exampleWiki.title": "title of the example person's own material",
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
};

// 筆記憶 is 筆 (the measure word for memories) + 記憶, not 筆記.
const ZH_NOTE = /筆記(?!憶)|笔记/;
const LATIN_SPACE = /\bSpaces?\b/;
const EN_NOTE = /\bnotes?\b/i;

function offenders(locale: "en" | "zh-Hans" | "zh-Hant", test: (text: string) => boolean): string[] {
  return flatten(resources[locale].translation)
    .filter(([key, text]) => !(key in KEPT) && test(text))
    .map(([key, text]) => `${key}: ${text}`);
}

describe("page naming", () => {
  it("UX-WORDS-1 ux_words_1: Chinese calls a page 頁面/页面 and a Space 空間/空间", () => {
    for (const locale of ["zh-Hans", "zh-Hant"] as const) {
      expect(offenders(locale, text => ZH_NOTE.test(text)), `${locale} 筆記/笔记`).toEqual([]);
      expect(offenders(locale, text => LATIN_SPACE.test(text)), `${locale} Space`).toEqual([]);
    }
  });

  it("UX-WORDS-1 ux_words_1: English calls a page a page, not a note", () => {
    expect(offenders("en", text => EN_NOTE.test(text))).toEqual([]);
  });

  it("UX-WORDS-1 ux_words_1: every kept key still exists and still needs keeping", () => {
    const flat = new Map<string, string>(
      (["en", "zh-Hans", "zh-Hant"] as const).flatMap(locale =>
        flatten(resources[locale].translation).map(([key, text]): [string, string] => [`${locale}:${key}`, text]),
      ),
    );
    const stale = Object.keys(KEPT).filter(key => {
      const texts = ["en", "zh-Hans", "zh-Hant"].map(locale => flat.get(`${locale}:${key}`) ?? "");
      return !(EN_NOTE.test(texts[0]) || ZH_NOTE.test(texts[1]) || ZH_NOTE.test(texts[2]));
    });
    expect(stale).toEqual([]);
  });
});
