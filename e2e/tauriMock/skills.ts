// SPDX-License-Identifier: AGPL-3.0-only
// Fixture answers for the skills.sh commands (app/src/skill_registry_commands.rs).
// Names and counts are examples, not real skills.sh data.

const COMMIT = "4f1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c";

const ROWS = [
  { source: "acme/agent-skills", skillId: "meeting-notes", name: "meeting-notes", installs: 12400, verdict: "pass" },
  { source: "acme/agent-skills", skillId: "minutes-to-tasks", name: "minutes-to-tasks", installs: 3100, verdict: "pass" },
  { source: "team/standup", skillId: "standup-summary", name: "standup-summary", installs: 890, verdict: "pass" },
  { source: "someone/tools", skillId: "notes-sync", name: "notes-sync", installs: 410, verdict: "warn" },
];

const SKILL_MD = `---
name: meeting-notes
description: Turn a meeting transcript into decisions, action items and owners.
---

1. Read the whole transcript first.
2. List decisions, each with who made it.
3. List action items with an owner and a due date if one was said.
4. Keep the speaker's words when quoting.
`;

export class SkillsFixture {
  private installed: Record<string, unknown>[] = [];

  supports(command: string): boolean {
    return command.startsWith("skills_");
  }

  invoke(command: string, args: Record<string, unknown> | undefined): unknown {
    switch (command) {
      case "skills_search": return { via: "skills.sh", rows: ROWS };
      case "skills_preview": {
        const skillId = String(args?.skillId ?? "meeting-notes");
        return {
          source: String(args?.source ?? "acme/agent-skills"), skillId, commit: COMMIT, repoPath: `skills/${skillId}`,
          name: skillId, description: "Turn a meeting transcript into decisions, action items and owners.",
          files: [{ path: "SKILL.md", text: SKILL_MD }], runnable: [],
          safety: { verdict: "pass", audits: ["Gen Agent Trust Hub", "Socket", "Snyk", "Runlayer", "ZeroLeaks"].map((provider) => ({ provider, status: "pass", summary: "" })) },
          sourceUrl: `https://github.com/acme/agent-skills/tree/${COMMIT}/skills/${skillId}`,
          skillsShUrl: `https://skills.sh/acme/agent-skills/${skillId}`, installedAs: null,
        };
      }
      case "skills_install": {
        const skill = {
          name: String(args?.skillId), source: String(args?.source), skillId: String(args?.skillId), repoPath: String(args?.repoPath),
          commit: String(args?.commit), hash: "h", verdict: "pass", scope: args?.scope, description: "Turn a meeting transcript into decisions, action items and owners.",
          installedAt: 1, updatedAt: 1,
        };
        this.installed = [...this.installed.filter((s) => s.name !== skill.name), { ...skill, additions: "" }];
        return { skill, tools: ["~/.claude/skills/meeting-notes", "~/.agents/skills/meeting-notes"], skipped: [] };
      }
      case "skills_installed": return this.installed;
      case "skills_refresh": return [];
      case "skills_set_additions": {
        this.installed = this.installed.map((s) => s.name === args?.name ? { ...s, additions: args?.text } : s);
        return { skill: this.installed.find((s) => s.name === args?.name), tools: [], skipped: [] };
      }
      case "skills_remove": this.installed = this.installed.filter((s) => s.name !== args?.name); return null;
      default: return null;
    }
  }
}
