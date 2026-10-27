// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import type { PageChangelogEntry } from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import { prettyAgent, revisionSummary, revisionWriterLabel } from "./format";

const tFor = (lng: "en" | "zh-Hans" | "zh-Hant") => i18n.getFixedT(lng) as unknown as TFunction;

const entry = (over: Partial<PageChangelogEntry> = {}): PageChangelogEntry => ({
  version: 2,
  at: 1_700_000_000,
  edited_by: "distill",
  ...over,
});

describe("revisionWriterLabel", () => {
  const table: Array<[string, { en: string; "zh-Hans": string; "zh-Hant": string }]> = [
    ["manual_edit", { en: "You edited in the app", "zh-Hans": "你在 app 里编辑", "zh-Hant": "你在 app 裡編輯" }],
    ["fs_edit", { en: "You edited the file", "zh-Hans": "你在文件里编辑", "zh-Hant": "你在檔案裡編輯" }],
    ["distill", { en: "Wenlan compiled it from sources", "zh-Hans": "文澜从来源整理", "zh-Hant": "文瀾從來源整理" }],
    ["re_distill", { en: "Wenlan recompiled it", "zh-Hans": "文澜重新整理", "zh-Hant": "文瀾重新整理" }],
    ["page_growth", { en: "Wenlan added new sources", "zh-Hans": "文澜加入新来源", "zh-Hant": "文瀾加入新來源" }],
    ["refinery_merge", { en: "Wenlan merged pages", "zh-Hans": "文澜合并笔记", "zh-Hant": "文瀾合併筆記" }],
    ["revision_accept", { en: "You approved a revision", "zh-Hans": "你核准了修订", "zh-Hant": "你核准了修訂" }],
    ["citation_backfill", { en: "Wenlan filled in citations", "zh-Hans": "文澜补上引用", "zh-Hant": "文瀾補上引用" }],
  ];

  it.each(table)("maps %s in every locale", (slug, labels) => {
    for (const lng of ["en", "zh-Hans", "zh-Hant"] as const) {
      expect(revisionWriterLabel(tFor(lng), slug)).toBe(labels[lng]);
    }
  });

  it("names the agent for agent_refresh, and falls back when it is unknown", () => {
    expect(revisionWriterLabel(tFor("en"), "agent_refresh", "claude-code")).toBe("Updated by Claude Code");
    expect(revisionWriterLabel(tFor("zh-Hant"), "agent_refresh", "claude-code")).toBe("Claude Code 更新");
    expect(revisionWriterLabel(tFor("zh-Hans"), "agent_refresh", "claude-code")).toBe("Claude Code 更新");
    expect(revisionWriterLabel(tFor("en"), "agent_refresh", "my-bot")).toBe("Updated by My Bot");
    expect(revisionWriterLabel(tFor("en"), "agent_refresh")).toBe("Updated by an agent");
    expect(revisionWriterLabel(tFor("en"), "agent_refresh", "  ")).toBe("Updated by an agent");
    expect(revisionWriterLabel(tFor("zh-Hant"), "agent_refresh", null)).toBe("Agent 更新");
    expect(revisionWriterLabel(tFor("zh-Hans"), "agent_refresh", null)).toBe("Agent 更新");
  });

  it("never shows a raw slug for a writer it does not know", () => {
    for (const slug of ["page_write", "human", "migration_84", "user", "", "toString", "constructor", "__proto__", "hasOwnProperty"]) {
      const label = revisionWriterLabel(tFor("en"), slug);
      expect(label).toBe("Updated");
      expect(label).not.toContain(slug || "\u0000");
    }
    expect(revisionWriterLabel(tFor("zh-Hant"), "page_write")).toBe("已更新");
    expect(revisionWriterLabel(tFor("zh-Hans"), "page_write")).toBe("已更新");
  });
});

describe("revisionSummary", () => {
  it("localizes the numeric delta and omits zero parts", () => {
    const e = entry({ char_delta: 71, sources_added: 2, sources_removed: 0, delta_summary: "+mem_a; +71 chars" });
    expect(revisionSummary(tFor("en"), e, "en")).toBe("+71 characters · 2 sources added");
    expect(revisionSummary(tFor("zh-Hant"), e, "zh-Hant")).toBe("+71 字 · 新增 2 個來源");
    expect(revisionSummary(tFor("zh-Hans"), e, "zh-Hans")).toBe("+71 字 · 新增 2 个来源");
  });

  it("shows a negative delta with a minus, and removed sources", () => {
    const e = entry({ char_delta: -12, sources_added: 0, sources_removed: 1 });
    expect(revisionSummary(tFor("en"), e, "en")).toBe("-12 characters · 1 source removed");
    expect(revisionSummary(tFor("zh-Hant"), e, "zh-Hant")).toBe("-12 字 · 移除 1 個來源");
  });

  it("omits a zero character delta", () => {
    const e = entry({ char_delta: 0, sources_added: 1, sources_removed: 0 });
    expect(revisionSummary(tFor("en"), e, "en")).toBe("1 source added");
  });

  it("shows nothing when every part is zero", () => {
    const e = entry({ char_delta: 0, sources_added: 0, sources_removed: 0, delta_summary: "x" });
    expect(revisionSummary(tFor("en"), e, "en")).toBeNull();
  });

  it("falls back to the English delta_summary only for an English UI on old entries", () => {
    const old = entry({ delta_summary: "+mem_a; +71 chars (re_distill)" });
    expect(revisionSummary(tFor("en"), old, "en")).toBe("+mem_a; +71 chars (re_distill)");
    expect(revisionSummary(tFor("zh-Hant"), old, "zh-Hant")).toBeNull();
    expect(revisionSummary(tFor("zh-Hans"), old, "zh-Hans")).toBeNull();
    expect(revisionSummary(tFor("en"), entry(), "en")).toBeNull();
  });
});

describe("prettyAgent", () => {
  it("uses the single agentDisplayName map and keeps its null and unknown behavior", () => {
    expect(prettyAgent("claude-code")).toBe("Claude Code");
    expect(prettyAgent(" Claude-Code ")).toBe("Claude Code");
    expect(prettyAgent("codex")).toBe("ChatGPT / Codex");
    expect(prettyAgent("My Bot")).toBe("My Bot");
    expect(prettyAgent(null)).toBe("unknown agent");
    expect(prettyAgent(undefined)).toBe("unknown agent");
    expect(prettyAgent("")).toBe("unknown agent");
  });
});
