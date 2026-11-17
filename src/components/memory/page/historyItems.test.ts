// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";
import { i18n } from "../../../i18n";
import type { SharedStatus } from "../../../lib/tauri";
import { historyItemLabel, historyRows } from "./historyItems";

const tFor = (lng: "en" | "zh-Hans" | "zh-Hant") => i18n.getFixedT(lng) as unknown as TFunction;

describe("UX-WHO-2 a change the person asked for shows who made it and their words", () => {
  const item = {
    kind: "ai_edit" as const,
    at: 1_700_000_000,
    version: 5,
    edited_by: "person_asked",
    agent: "claude-code",
    person_asked: "Remove the sentence about January.",
  };

  it("labels it as made at the person's request, in every language", () => {
    expect(historyItemLabel(tFor("en"), item)).toBe("Claude Code changed it at your request");
    expect(historyItemLabel(tFor("zh-Hans"), item)).toBe("Claude Code 应你的要求修改");
    expect(historyItemLabel(tFor("zh-Hant"), item)).toBe("Claude Code 應你的要求修改");
  });

  it("quotes the person's words", () => {
    expect(tFor("en")("pageHistory.yourWords", { quote: item.person_asked })).toBe(
      "You asked: “Remove the sentence about January.”",
    );
    expect(tFor("zh-Hant")("pageHistory.yourWords", { quote: "刪掉一月那句" })).toBe("你說：「刪掉一月那句」");
  });

  it("keeps the quote when only the changelog is served", () => {
    const rows = historyRows(undefined, [
      { version: 5, at: 1, edited_by: "person_asked", agent: "claude-code", person_asked: "Remove it." },
    ]);
    expect(rows[0].kind).toBe("ai_edit");
    expect(rows[0].person_asked).toBe("Remove it.");
  });
});

describe("UX-HIST-1 putting an earlier version back is listed as restored version N", () => {
  it("names the version it restored", () => {
    const item = { kind: "ai_edit" as const, at: 1, version: 7, edited_by: "agent_refresh", restored_from: 3 };
    expect(historyItemLabel(tFor("en"), item)).toBe("Restored version 3");
    expect(historyItemLabel(tFor("zh-Hans"), item)).toBe("还原成第 3 版");
    expect(historyItemLabel(tFor("zh-Hant"), item)).toBe("還原成第 3 版");
  });
});

describe("an archive or a restore says who did it, or reads whole without a name", () => {
  const shares = [{
    kind: "space", target: "Product", folder: "", host: "https://team.example.com", read_only: false, hosted: true,
    members: [{ id: "amy", name: "Amy" }],
  }] as unknown as SharedStatus[];
  const row = (kind: "archived" | "restored", edited_by: string | null) =>
    ({ kind, at: 1_700_000_000, version: 3, edited_by });

  it("names a member, you, or an AI tool", () => {
    expect(historyItemLabel(tFor("en"), row("archived", "shared_person:amy"), shares)).toBe("Amy archived this page");
    expect(historyItemLabel(tFor("en"), row("restored", "manual_edit"), shares)).toBe("You restored this page");
    expect(historyItemLabel(tFor("zh-Hant"), row("archived", "manual_edit"), shares)).toBe("你封存了這篇筆記");
    expect(historyItemLabel(tFor("zh-Hans"), row("restored", "shared_person:amy"), shares)).toBe("Amy恢复了这篇笔记");
    expect(historyItemLabel(tFor("en"), row("archived", "ai:claude-code"))).toBe("Claude Code archived this page");
  });

  it("an old row with nobody recorded is a full sentence with no blank", () => {
    expect(historyItemLabel(tFor("en"), row("archived", null))).toBe("This page was archived");
    expect(historyItemLabel(tFor("en"), row("restored", null))).toBe("This page was restored");
    expect(historyItemLabel(tFor("zh-Hant"), row("archived", null))).toBe("這篇筆記已封存");
    expect(historyItemLabel(tFor("zh-Hant"), row("restored", null))).toBe("這篇筆記已還原");
    expect(historyItemLabel(tFor("zh-Hans"), row("archived", null))).toBe("这篇笔记已归档");
  });
});
