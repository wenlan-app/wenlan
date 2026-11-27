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
    ["refinery_merge", { en: "Wenlan merged pages", "zh-Hans": "文澜合并页面", "zh-Hant": "文瀾合併頁面" }],
    ["revision_accept", { en: "You approved a revision", "zh-Hans": "你核准了修订", "zh-Hant": "你核准了修訂" }],
    ["citation_backfill", { en: "Wenlan filled in citations", "zh-Hans": "文澜补上引用", "zh-Hant": "文瀾補上引用" }],
    ["person_create", { en: "You created this page", "zh-Hans": "你创建了这一页", "zh-Hant": "你建立了這一頁" }],
    ["agent_create", { en: "Created by an agent", "zh-Hans": "Agent 创建", "zh-Hant": "Agent 建立" }],
    ["create", { en: "Created", "zh-Hans": "创建", "zh-Hant": "建立" }],
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

  it("names the AI tool that created a page, and falls back when it is unknown", () => {
    expect(revisionWriterLabel(tFor("en"), "agent_create", "claude-code")).toBe("Created by Claude Code");
    expect(revisionWriterLabel(tFor("zh-Hant"), "agent_create", "codex")).toBe("ChatGPT / Codex 建立");
    expect(revisionWriterLabel(tFor("zh-Hans"), "agent_create", "claude-code")).toBe("Claude Code 创建");
    expect(revisionWriterLabel(tFor("en"), "agent_create", null)).toBe("Created by an agent");
    expect(revisionWriterLabel(tFor("en"), "agent_create", "  ")).toBe("Created by an agent");
    expect(revisionWriterLabel(tFor("en"), "agent_create", "terminal")).toBe("Created by an agent");
    expect(revisionWriterLabel(tFor("en"), "agent_create", "tool:Wenlan")).toBe("Created by an agent");
  });

  it("UX-SKILL-14: a saved rule says where it was saved from, and a removed one where it was removed, never written by you", () => {
    const en = tFor("en");
    expect(revisionWriterLabel(en, "rule_save", "cursor", 24)).toBe("Rule saved from Cursor");
    expect(revisionWriterLabel(en, "rule_save", "terminal", 24)).toBe("Rule saved from the terminal");
    expect(revisionWriterLabel(en, "rule_save", "cursor")).toBe("Rule saved from Cursor");
    expect(revisionWriterLabel(en, "rule_save", "cursor", -24)).toBe("Rule removed in Cursor");
    expect(revisionWriterLabel(en, "rule_save", "terminal", -24)).toBe("Rule removed in the terminal");
    // The Remove the person pressed in the app.
    expect(revisionWriterLabel(en, "rule_save", "wenlan-app", -24)).toBe("You removed a rule");
    // A tool that calls itself the terminal, you or Wenlan is recorded apart
    // from them (`tool:<name>`), and is listed as an AI tool with the name
    // it gave in quotes: never as the terminal, the person or the app.
    expect(revisionWriterLabel(en, "rule_save", "tool:terminal", 24)).toBe("Rule saved from an AI tool that calls itself “terminal”");
    expect(revisionWriterLabel(en, "rule_save", "tool:terminal", -24)).toBe("Rule removed in an AI tool that calls itself “terminal”");
    expect(revisionWriterLabel(en, "rule_save", "tool:you", 24)).toBe("Rule saved from an AI tool that calls itself “you”");
    expect(revisionWriterLabel(en, "rule_save", "tool:wenlan-app", -24)).toBe("Rule removed in an AI tool that calls itself “wenlan-app”");
    expect(revisionWriterLabel(en, "rule_save", "tool:wenlan app", -24)).toBe("Rule removed in an AI tool that calls itself “wenlan app”");
    expect(revisionWriterLabel(tFor("zh-Hant"), "rule_save", "tool:terminal", 24)).toBe("自稱「terminal」的 AI 工具存的規則");
    expect(revisionWriterLabel(tFor("zh-Hans"), "rule_save", "tool:you", -24)).toBe("自称「you」的 AI 工具移除了规则");
    expect(revisionWriterLabel(en, "rule_save", "tool:", 24)).toBe("Rule saved");
    for (const lng of ["en", "zh-Hans", "zh-Hant"] as const) {
      for (const agent of ["tool:terminal", "tool:you", "tool:wenlan-app", "tool:wenlan app", "tool:the terminal"]) {
        for (const delta of [24, -24]) {
          const label = revisionWriterLabel(tFor(lng), "rule_save", agent, delta);
          expect(label).toMatch(/AI tool that calls itself “|自稱「|自称「/u);
          expect(label).not.toMatch(/^Rule (saved from|removed in) (the terminal|Terminal|You|Wenlan App)$|You removed|你移除|從終端機|从终端|在終端機|在终端/u);
        }
      }
    }
    expect(revisionWriterLabel(en, "rule_save", null, 24)).toBe("Rule saved");
    expect(revisionWriterLabel(en, "rule_save", " ", -1)).toBe("Rule removed");
    expect(revisionWriterLabel(tFor("zh-Hant"), "rule_save", "cursor", 24)).toBe("從 Cursor 存的規則");
    expect(revisionWriterLabel(tFor("zh-Hant"), "rule_save", "terminal", 24)).toBe("從終端機存的規則");
    expect(revisionWriterLabel(tFor("zh-Hant"), "rule_save", "wenlan-app", -24)).toBe("你移除了一條規則");
    expect(revisionWriterLabel(tFor("zh-Hans"), "rule_save", "cursor", -24)).toBe("在 Cursor 移除了规则");
    expect(revisionWriterLabel(tFor("zh-Hans"), "rule_save", "terminal", 24)).toBe("从终端存的规则");
    for (const lng of ["en", "zh-Hans", "zh-Hant"] as const) {
      for (const agent of ["cursor", "terminal", "wenlan-app", null]) {
        for (const delta of [24, -24]) {
          const label = revisionWriterLabel(tFor(lng), "rule_save", agent, delta);
          expect(label).not.toMatch(/written by you|你写的|你寫的|^Updated$|已更新/u);
        }
      }
    }
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
