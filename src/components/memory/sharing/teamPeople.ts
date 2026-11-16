// SPDX-License-Identifier: AGPL-3.0-only
// In a team member's browser, the people they share a Space with, from the
// team server's `/team/v1/me`: what names a change by its writer. Empty in
// the desktop app and the personal web app.
import type { TFunction } from "i18next";
import { agentDisplayName } from "../../../lib/tauri";

export interface TeamPerson {
  id: string;
  name: string;
  owner?: boolean;
}

let me: string | null = null;
let people = new Map<string, TeamPerson>();

/** Set once the member's key is checked; `null` clears it (tests). */
export function setTeamPeople(memberId: string | null, list: readonly TeamPerson[] = []): void {
  me = memberId;
  people = new Map(list.map((person) => [person.id, person]));
}

// What the server records for the person at its own computer: on a team
// server, that is the team's owner. The same person writers the core counts
// (authorship.rs), so a version the owner restored or accepted is theirs.
const OWNER_WRITERS = new Set([
  "manual_edit", "person_create", "human", "fs_edit", "own_device",
  "revision_accept", "publish", "restore",
]);

/** Wenlan's own background organizing: nobody's AI tool. */
export const WENLAN_WRITERS = new Set(["distill", "re_distill", "page_growth", "refinery_merge", "citation_backfill", "link_rename"]);

function personName(t: TFunction, person: TeamPerson): string {
  return person.id === me ? t("sharing.you", { name: person.name }) : person.name;
}

/** A person's AI tool: "Your AI tool" only for the member signed in here. */
function personAi(t: TFunction, person: TeamPerson): string {
  return person.id === me ? t("team.changes.yourAi") : t("sharing.memberAi", { name: person.name });
}

const owner = () => [...people.values()].find((person) => person.owner);

/**
 * Who made a change, for a team member: the owner's own edits by the
 * owner's name, a member's by theirs ("Amy (you)" for the member's own).
 * `null` outside a member's browser or for a writer it cannot name.
 */
export function teamWriter(editedBy: string | null | undefined, t: TFunction): string | null {
  if (!me || !editedBy) return null;
  if (OWNER_WRITERS.has(editedBy)) {
    const found = owner();
    return found ? personName(t, found) : null;
  }
  const found = /^(shared_person|shared_ai):([A-Za-z0-9_-]+)$/.exec(editedBy);
  const person = found ? people.get(found[2]) : undefined;
  if (!found || !person) return null;
  return found[1] === "shared_ai" ? personAi(t, person) : personName(t, person);
}

/**
 * An AI tool's change, for a team member: a member's AI tool connected
 * through the team server (`<tool>@team.<member>`) by that member, and any
 * other AI writer on the server by the team's owner, whose server it is.
 * `agent` is the tool the change records, or the `ai:<tool>` of an archive.
 * `null` outside a member's browser, for a person's or Wenlan's own change,
 * or when the person cannot be named.
 */
export function teamAiWriter(t: TFunction, editedBy: string | null | undefined, agent?: string | null): string | null {
  if (!me || !editedBy || OWNER_WRITERS.has(editedBy) || WENLAN_WRITERS.has(editedBy)) return null;
  if (/^(shared_person|shared_ai):/.test(editedBy)) return null;
  const tool = editedBy.startsWith("ai:") ? editedBy.slice(3) : agent;
  const team = tool ? /@team\.([A-Za-z0-9_-]+)$/.exec(tool.trim()) : null;
  const person = team ? people.get(team[1]) : owner();
  return person ? personAi(t, person) : null;
}

/**
 * An AI tool's name as a change shows it. A member's AI tool connected
 * through the team server is recorded as `<tool>@team.<member>`: it reads as
 * that member's AI tool where the member is known, else as the tool alone,
 * never with the member id.
 */
export function writerAgentName(t: TFunction, agent: string | null | undefined): string | null {
  const name = agent?.trim();
  const team = name ? /^(.+)@team\.([A-Za-z0-9_-]+)$/.exec(name) : null;
  if (!team) return agentDisplayName(name);
  const person = people.get(team[2]);
  return person ? personAi(t, person) : agentDisplayName(team[1]);
}
