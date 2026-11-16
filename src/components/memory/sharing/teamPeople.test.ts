// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../../../i18n";
import { changeWho } from "../space-detail/SpaceRecentChanges";
import { historyItemLabel } from "../page/historyItems";
import type { PageHistoryItem } from "../../../lib/tauri";
import { setTeamPeople, teamWriter, writerAgentName } from "./teamPeople";

const t = i18n.t.bind(i18n);

beforeEach(async () => {
  await i18n.changeLanguage("en");
  setTeamPeople("amy1", [
    { id: "lin0", name: "Lin", owner: true },
    { id: "amy1", name: "Amy" },
    { id: "ben2", name: "Ben" },
  ]);
});
afterEach(() => setTeamPeople(null));

describe("names in a team member's browser", () => {
  it("names the owner's own edits, each member's, and the member's own as theirs", () => {
    expect(teamWriter("manual_edit", t)).toBe("Lin");
    expect(teamWriter("person_create", t)).toBe("Lin");
    expect(teamWriter("shared_person:ben2", t)).toBe("Ben");
    expect(teamWriter("shared_person:amy1", t)).toBe("Amy (you)");
    expect(teamWriter("shared_ai:ben2", t)).toBe("Ben's AI tool");
  });

  it("names a member's AI tool by the member, never by their id", () => {
    expect(writerAgentName(t, "claude-code@team.ben2")).toBe("Ben's AI tool");
    const unknown = writerAgentName(t, "claude-code@team.zz9") ?? "";
    expect(unknown).not.toContain("zz9");
    expect(changeWho("agent_refresh", undefined, t, "claude-code@team.ben2")).toBe("Ben's AI tool");
    expect(changeWho("shared_person:amy1", undefined, t)).toBe("Amy (you)");
  });

  // Recent changes, page history and the archive and restore rows, for each
  // writer: the owner by hand, the owner's AI tool, another member, and the
  // member signed in here. "You" and "Your AI tool" only for the last.
  const edit = (edited_by: string, agent?: string): PageHistoryItem =>
    ({ kind: "ai_edit", at: 1, version: 2, edited_by, agent } as PageHistoryItem);
  const lifecycle = (kind: "archived" | "restored", edited_by: string): PageHistoryItem =>
    ({ kind, at: 1, version: 2, edited_by } as PageHistoryItem);
  const cases: [string, string, string | undefined, string, string][] = [
    // writer, edited_by, agent, a change's name, an archive's actor
    ["owner by hand", "manual_edit", undefined, "Lin", "manual_edit"],
    ["owner's AI tool", "agent_refresh", "claude-code", "Lin's AI tool", "ai:claude-code"],
    ["another member", "shared_person:ben2", undefined, "Ben", "shared_person:ben2"],
    ["another member's AI tool", "agent_refresh", "claude-code@team.ben2", "Ben's AI tool", "ai:claude-code@team.ben2"],
    ["self", "shared_person:amy1", undefined, "Amy (you)", "shared_person:amy1"],
    ["self's AI tool", "agent_refresh", "claude-code@team.amy1", "Your AI tool", "ai:claude-code@team.amy1"],
  ];
  for (const [writer, editedBy, agent, name, actor] of cases) {
    it(`names the ${writer} the same in recent changes, history and archive rows`, () => {
      expect(changeWho(editedBy, undefined, t, agent)).toBe(name);
      expect(historyItemLabel(t, edit(editedBy, agent))).toBe(name);
      expect(historyItemLabel(t, lifecycle("archived", actor))).toBe(`${name} archived this page`);
      expect(historyItemLabel(t, lifecycle("restored", actor))).toBe(`${name} restored this page`);
    });
  }

  it("never says you or your for anyone else's change", () => {
    for (const editedBy of ["agent_refresh", "agent_create", "own_device_ai", "restore", "revision_accept", "publish"]) {
      const who = changeWho(editedBy, undefined, t, null);
      expect(who, editedBy).not.toMatch(/\b(you|your)\b/i);
    }
    expect(changeWho("distill", undefined, t)).toBe("Wenlan");
    // A page Wenlan made itself names nobody's tool.
    expect(changeWho("create", undefined, t, null)).toBe("Wenlan");
  });

  it("names nothing outside a member's browser or for a stranger", () => {
    expect(teamWriter("shared_person:cy9", t)).toBeNull();
    setTeamPeople(null);
    expect(teamWriter("manual_edit", t)).toBeNull();
    expect(changeWho("manual_edit", undefined, t)).toBe("You");
    expect(changeWho("agent_refresh", undefined, t, "claude-code")).toBe("Your AI tool");
  });
});
