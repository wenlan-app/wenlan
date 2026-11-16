// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { i18n } from "../../../i18n";
import { changeWho } from "../space-detail/SpaceRecentChanges";
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

  it("names nothing outside a member's browser or for a stranger", () => {
    expect(teamWriter("shared_person:cy9", t)).toBeNull();
    setTeamPeople(null);
    expect(teamWriter("manual_edit", t)).toBeNull();
    expect(changeWho("manual_edit", undefined, t)).toBe("You");
  });
});
