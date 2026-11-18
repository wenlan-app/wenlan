// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Page, SkillLine } from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import { howIWorkPageTitle, isHowIWorkPage, lastRulesWrite, maintainedSourceCount, toolBadges } from "./howIWorkModel";
import { TOOL_LOGOS } from "./toolLogos";

describe("how-I-work page detection", () => {
  it("treats a list row without a body as an ordinary page", () => {
    const row = { title: "Reading list" } as unknown as Pick<Page, "content" | "title">;
    expect(isHowIWorkPage(row)).toBe(false);
  });
});

describe("how-I-work page title", () => {
  const rulesPage = (space: string) => ({ title: `How I work — ${space}`, space, content: `# How I work — ${space}\n` });

  it("UX-SKILL-2: names every Space's page plainly, with no Space after it, in each language", async () => {
    const t = i18n.getFixedT("en");
    expect(howIWorkPageTitle(rulesPage("Wenlan"), t)).toBe("How I work");
    // Two Spaces' pages now share the name.
    expect(howIWorkPageTitle(rulesPage("Other"), t)).toBe("How I work");
    expect(howIWorkPageTitle(rulesPage("Wenlan"), i18n.getFixedT("zh-Hant"))).toBe("我的做事方式");
    expect(howIWorkPageTitle(rulesPage("Wenlan"), i18n.getFixedT("zh-Hans"))).toBe("我的做事方式");
    expect(howIWorkPageTitle({ title: "Git", space: null, content: "- a rule\n" }, t)).toBe("Git");
  });
});

describe("rules page status line", () => {
  const line = (id: string, source: string): SkillLine => ({ id, kind: "rule", text: id, source_page_id: source, source_page_title: source, source_revision: 1, source_revision_at: 1, source_quote: id });

  it("UX-SKILL-2: counts the distinct source pages of the lines Wenlan maintains, never the page's own bullets", () => {
    expect(maintainedSourceCount([line("a", "p1"), line("b", "p1"), line("c", "p2"), line("own:0:x", "this-page")])).toBe(2);
    expect(maintainedSourceCount([line("own:0:x", "this-page")])).toBe(0);
  });

  it("UX-SKILL-2: takes the latest rules write from the page's history", () => {
    expect(lastRulesWrite([{ at: 10, edited_by: "agent_refresh" }, { at: 30, edited_by: "user" }, { at: 20, edited_by: "agent_refresh" }])).toBe(20);
    expect(lastRulesWrite([{ at: 30, edited_by: "user" }])).toBeNull();
  });
});

describe("tool badges", () => {
  it("UX-SKILL-11: gives each tool a short badge from its name, and no two the same", () => {
    expect(toolBadges(["Claude Code", "Codex", "Cursor", "Windsurf"])).toEqual(["CC", "C", "Cu", "W"]);
    const many = toolBadges(["Muse Code", "Muse Cloud", "Mu Ce", "VS Code", "Continue"]);
    expect(new Set(many).size).toBe(many.length);
  });

  it("UX-SKILL-11: has a logo for each tool whose rules file Wenlan writes", () => {
    for (const id of ["codex", "copilot-cli", "windsurf", "zed", "kiro", "claude-code", "gemini-cli", "antigravity", "opencode", "cursor"]) {
      expect(TOOL_LOGOS[id], id).toBeDefined();
      expect(TOOL_LOGOS[id].paths.length, id).toBeGreaterThan(0);
    }
  });
});
