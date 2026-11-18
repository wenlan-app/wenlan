// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Page, SkillLine } from "../../../lib/tauri";
import { i18n } from "../../../i18n";
import { howIWorkHeading, howIWorkPageTitle, isHowIWorkPage, maintainedSourceCount, readRulesPageHistory, toolBadges } from "./howIWorkModel";
import { TOOL_LOGOS } from "./toolLogos";

describe("how-I-work page detection", () => {
  it("treats a list row without a body as an ordinary page", () => {
    const row = { title: "Reading list" } as unknown as Pick<Page, "content" | "title">;
    expect(isHowIWorkPage(row)).toBe(false);
  });
});

describe("how-I-work page title", () => {
  const rulesPage = (space: string) => ({ title: `How I work — ${space}`, space, content: `# How I work — ${space}\n` });

  it("UX-SKILL-2: heads the page plainly, and names its Space wherever it is listed, in each language", () => {
    const t = i18n.getFixedT("en");
    expect(howIWorkHeading(rulesPage("Wenlan"), t)).toBe("How I work");
    expect(howIWorkHeading(rulesPage("Wenlan"), i18n.getFixedT("zh-Hant"))).toBe("我的做事方式");
    expect(howIWorkHeading(rulesPage("Wenlan"), i18n.getFixedT("zh-Hans"))).toBe("我的做事方式");
    // Two Spaces' pages are never listed under the same name.
    expect(howIWorkPageTitle(rulesPage("Wenlan"), t)).toBe("How I work · Wenlan");
    expect(howIWorkPageTitle(rulesPage("Client"), t)).toBe("How I work · Client");
    expect(howIWorkPageTitle(rulesPage("Wenlan"), i18n.getFixedT("zh-Hant"))).toBe("我的做事方式 · Wenlan");
    // A pinned row keeps only the stored title, and reads the same.
    expect(howIWorkPageTitle({ title: "How I work — Wenlan" }, t)).toBe("How I work · Wenlan");
    expect(howIWorkPageTitle({ title: "Git", space: null, content: "- a rule\n" }, t)).toBe("Git");
    expect(howIWorkHeading({ title: "Git", space: null, content: "- a rule\n" }, t)).toBe("Git");
  });
});

describe("rules page status line", () => {
  const line = (id: string, source: string): SkillLine => ({ id, kind: "rule", text: id, source_page_id: source, source_page_title: source, source_revision: 1, source_revision_at: 1, source_quote: id });

  it("UX-SKILL-2: counts the distinct source pages of the lines Wenlan maintains, never the page's own bullets", () => {
    expect(maintainedSourceCount([line("a", "p1"), line("b", "p1"), line("c", "p2"), line("own:0:x", "this-page")])).toBe(2);
    expect(maintainedSourceCount([line("own:0:x", "this-page")])).toBe(0);
  });

  const v = (version: number, at: number, edited_by: string, text: string) => ({ version, at, edited_by, text });
  const STOP = "<!-- wenlan:not-for-ai-tools -->";

  it("UX-SKILL-2: takes the rules write that still stands, and counts only the person's own edits", () => {
    const history = [
      v(1, 10, "agent_create", "# H\n- a"),
      v(2, 20, "agent_refresh", "# H\n- a\n- b"),
      v(3, 30, "manual_edit", "# H\n- a\n- b\n- mine"),
      v(4, 40, "agent_refresh", "# H\n- a\n- b\n- mine\n- c"),
    ];
    expect(readRulesPageHistory(history)).toEqual({ youEdited: true, rulesWrittenAt: 40 });
    // Undo last night puts back the text from before the last rules write:
    // that write no longer stands, the one before it does, and Undo is not an edit.
    expect(readRulesPageHistory([...history.slice(0, 2), v(3, 50, "manual_edit", "# H\n- a")])).toEqual({ youEdited: false, rulesWrittenAt: null });
    expect(readRulesPageHistory([...history, v(5, 50, "manual_edit", "# H\n- a\n- b\n- mine")])).toEqual({ youEdited: true, rulesWrittenAt: 20 });
  });

  it("UX-SKILL-2: Stop, Start and a teammate's edit are not the person editing", () => {
    const history = [
      v(1, 10, "agent_refresh", "# H\n- a"),
      v(2, 20, "manual_edit", `# H\n\n${STOP}\n- a`),
      v(3, 30, "manual_edit", "# H\n- a"),
      v(4, 40, "shared_person:member-2", "# H\n- a\n- theirs"),
    ];
    expect(readRulesPageHistory(history)).toEqual({ youEdited: false, rulesWrittenAt: 10 });
    expect(readRulesPageHistory([...history, v(5, 50, "own_device", "# H\n- a\n- theirs\n- mine")]).youEdited).toBe(true);
    expect(readRulesPageHistory([])).toEqual({ youEdited: false, rulesWrittenAt: null });
  });
});

describe("tool badges", () => {
  it("UX-SKILL-11: gives each tool a short badge from its name, and no two the same", () => {
    expect(toolBadges(["Claude Code", "Codex", "Cursor", "Windsurf"])).toEqual(["CC", "C", "Cu", "W"]);
    const many = toolBadges(["Muse Code", "Muse Cloud", "Mu Ce", "VS Code", "Continue"]);
    expect(new Set(many).size).toBe(many.length);
  });

  it("UX-SKILL-11: has a logo for each tool whose rules file Wenlan writes, except VS Code, which shows a badge", () => {
    for (const id of ["codex", "copilot-cli", "windsurf", "zed", "kiro", "claude-code", "gemini-cli", "antigravity", "opencode", "cursor"]) {
      expect(TOOL_LOGOS[id], id).toBeDefined();
      expect(TOOL_LOGOS[id].paths.length, id).toBeGreaterThan(0);
    }
    expect(TOOL_LOGOS.vscode).toBeUndefined();
    expect(toolBadges(["VS Code"])).toEqual(["VC"]);
  });

  it("UX-SKILL-11: draws a logo in its brand colour only where it has 3:1 contrast with the chip, else in the text colour", () => {
    const luminance = (hex: string) => {
      const [r, g, b] = [1, 3, 5].map((at) => parseInt(hex.slice(at, at + 2), 16) / 255).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
      return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (a: string, b: string) => { const [x, y] = [luminance(a), luminance(b)].sort((m, n) => n - m); return (x + 0.05) / (y + 0.05); };
    for (const [id, logo] of Object.entries(TOOL_LOGOS)) {
      for (const [shown, surface] of [[logo.light, "#FFFFFF"], [logo.dark, "#202430"]] as const) {
        // Black and near-black brands always take the text colour, which is
        // that black in the light theme and stays visible in the dark one.
        if (shown === null) expect(luminance(logo.brand) < 0.01 || contrast(logo.brand, surface) < 3, `${id} on ${surface}`).toBe(true);
        else { expect(shown).toBe(logo.brand); expect(contrast(shown, surface), `${id} on ${surface}`).toBeGreaterThanOrEqual(3); }
      }
    }
  });
});
