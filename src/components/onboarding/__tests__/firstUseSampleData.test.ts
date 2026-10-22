// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect } from "vitest";
import {
  getSampleData,
  resolveSampleLocale,
  type SampleLocale,
} from "../firstUseSampleData";

const LOCALES: SampleLocale[] = ["en", "zh-Hans", "zh-Hant"];

describe("sample dataset integrity", () => {
  it.each(LOCALES)("keeps every %s citation quote inside its source body", (locale) => {
    const data = getSampleData(locale);
    const bodies = new Map(data.sources.map((source) => [source.id, source.body]));
    // Two captured AI sessions and one file: the page is the result, never a source.
    expect(data.sources.map((source) => source.type)).toEqual(["memory", "memory", "file"]);
    for (const citation of data.citations) {
      const body = bodies.get(citation.sourceId);
      expect(body, citation.id).toBeDefined();
      const joined = (body ?? []).join("\n");
      expect(
        joined.includes(citation.quote),
        `${locale}:${citation.id}`,
      ).toBe(true);
    }
  });

  it.each(LOCALES)("resolves every %s paragraph citation to a real quote", (locale) => {
    const data = getSampleData(locale);
    const quotes = new Set(data.citations.map((citation) => citation.id));
    for (const paragraph of data.page.paragraphs) {
      expect(paragraph.citationIds.length).toBeGreaterThan(0);
      for (const id of paragraph.citationIds) {
        expect(quotes.has(id), `${locale}:${id}`).toBe(true);
      }
    }
    expect(data.ai.question).toBeTruthy();
    expect(data.ai.answer).toContain(data.page.title);
  });

  it.each(LOCALES)("gives %s a current-state part and a what-changed part, each line cited", (locale) => {
    const data = getSampleData(locale);
    const headed = data.page.paragraphs.filter((paragraph) => paragraph.heading);
    expect(headed).toHaveLength(2);
    expect(data.page.paragraphs[0]?.heading).toBeTruthy();
    // Citation ids are unique per paragraph so each chip is individually addressable.
    const used = data.page.paragraphs.flatMap((paragraph) => paragraph.citationIds);
    expect(new Set(used).size).toBe(used.length);
    // The what-changed part draws on an AI session, not only the latest file.
    const sourceIdByCitation = new Map(data.citations.map((citation) => [citation.id, citation.sourceId]));
    const changed = data.page.paragraphs.slice(data.page.paragraphs.indexOf(headed[1]!));
    expect(changed.length).toBeGreaterThan(0);
    for (const paragraph of changed) {
      const types = paragraph.citationIds.map((id) => {
        const sourceId = sourceIdByCitation.get(id);
        return data.sources.find((source) => source.id === sourceId)?.type;
      });
      expect(types).toContain("memory");
    }
  });

  it.each(LOCALES)("gives %s distinct own-data commands per client", (locale) => {
    const data = getSampleData(locale);
    const { chatgpt, codex, claude } = data.commands;
    expect(chatgpt.recall).toMatch(/@/);
    expect(codex.recall.startsWith("/recall")).toBe(true);
    expect(claude.recall.startsWith("/recall")).toBe(true);
    expect(chatgpt.recall).not.toBe(codex.recall);
    // The ChatGPT web connector cannot write, so it offers no handoff command.
    expect(chatgpt.handoff).toBeNull();
    expect(chatgpt.recall).toBeTruthy();
    expect(chatgpt.brief).toBeTruthy();
    for (const commands of [codex, claude]) {
      expect(commands.recall).toBeTruthy();
      expect(commands.brief).toBeTruthy();
    }
  });

  it.each(LOCALES)("keeps the %s ChatGPT connector read-only with no handoff", (locale) => {
    const data = getSampleData(locale);
    // The ChatGPT web connector cannot write: an explicit null keeps the UI
    // from rendering (or faking) a save command.
    expect(data.commands.chatgpt.handoff).toBeNull();
    // Local clients keep full recall/handoff/brief support.
    for (const commands of [data.commands.codex, data.commands.claude]) {
      expect(commands.handoff).toBeTruthy();
    }
  });
});

describe("resolveSampleLocale", () => {
  it.each([
    [undefined, "en"],
    ["en", "en"],
    ["en-US", "en"],
    ["fr", "en"],
    ["zh", "zh-Hans"],
    ["zh-CN", "zh-Hans"],
    ["zh-Hans", "zh-Hans"],
    ["zh-TW", "zh-Hant"],
    ["zh-HK", "zh-Hant"],
    ["zh-Hant", "zh-Hant"],
    ["zh-Hant-HK", "zh-Hant"],
  ])("maps %s to %s", (input, expected) => {
    expect(resolveSampleLocale(input)).toBe(expected);
  });
});
