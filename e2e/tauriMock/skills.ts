// SPDX-License-Identifier: AGPL-3.0-only
// Fixture answers for the skills.sh commands (app/src/skill_registry_commands.rs).
// Names and counts are examples, not real skills.sh data.

const COMMIT = "4f1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c";

const ROWS = [
  { source: "acme/agent-skills", skillId: "meeting-notes", name: "meeting-notes", installs: 12400, verdict: "pass" },
  { source: "acme/agent-skills", skillId: "minutes-to-tasks", name: "minutes-to-tasks", installs: 3100, verdict: "pass" },
  { source: "team/standup", skillId: "standup-summary", name: "standup-summary", installs: 890, verdict: "pass" },
  { source: "someone/tools", skillId: "notes-sync", name: "notes-sync", installs: 410, verdict: "warn" },
  // Its notes file is longer than a preview shows, so it cannot be installed.
  { source: "team/standup", skillId: "long-playbook", name: "long-playbook", installs: 120, verdict: "pass" },
];

const LONG_SKILL_MD = `---
name: long-playbook
description: Run a short daily stand-up.
---

Follow notes/playbook.md.
`;

const LONG_NOTES = `# Playbook

Start every stand-up with what changed since yesterday.
Keep each update under one minute.
`;

const SKILL_MD = `---
name: meeting-notes
description: Turn a meeting transcript into decisions, action items and owners.
---

1. Read the whole transcript first.
2. List decisions, each with who made it.
3. List action items with an owner and a due date if one was said.
4. Keep the speaker's words when quoting.
`;

const LOCAL = [
  { name: "code-review", path: "~/.claude/skills/code-review", description: "How this team reviews a pull request." },
  { name: "release-notes", path: "~/.claude/skills/release-notes", description: "Write release notes from merged changes." },
  { name: "sql-style", path: "~/.agents/skills/sql-style", description: "House style for SQL queries." },
];

export class SkillsFixture {
  private installed: Record<string, unknown>[] = [];
  private local = [...LOCAL];

  supports(command: string): boolean {
    return command.startsWith("skills_");
  }

  invoke(command: string, args: Record<string, unknown> | undefined): unknown {
    switch (command) {
      case "skills_search": return { via: "skills.sh", rows: ROWS };
      case "skills_preview": {
        const skillId = String(args?.skillId ?? "meeting-notes");
        const tooLong = skillId === "long-playbook";
        return {
          source: String(args?.source ?? "acme/agent-skills"), skillId, commit: COMMIT, repoPath: `skills/${skillId}`,
          name: skillId, description: tooLong ? "Run a short daily stand-up." : "Turn a meeting transcript into decisions, action items and owners.",
          files: tooLong
            ? [{ path: "SKILL.md", text: LONG_SKILL_MD, cut: false }, { path: "notes/playbook.md", text: LONG_NOTES, cut: true }]
            : [{ path: "SKILL.md", text: SKILL_MD, cut: false }],
          cannotInstall: tooLong ? "notes/playbook.md is too long to show in full (65536 characters at most), so Wenlan refuses this skill" : null,
          runnable: [],
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
      case "skills_scan_local": return this.local;
      case "skills_import_local": {
        const names = (args?.names as string[] | undefined) ?? [];
        for (const name of names) {
          const found = this.local.find((s) => s.name === name);
          if (!found) continue;
          this.installed = [...this.installed, {
            name, source: "local", skillId: name, repoPath: found.path, commit: "", hash: "h", verdict: "warn",
            scope: { kind: "all" }, description: found.description, installedAt: 1, updatedAt: 1, additions: "",
          }];
        }
        this.local = this.local.filter((s) => !names.includes(s.name));
        return { imported: names, failed: [] };
      }
      case "skills_set_additions": {
        this.installed = this.installed.map((s) => s.name === args?.name ? { ...s, additions: args?.text } : s);
        return { skill: this.installed.find((s) => s.name === args?.name), tools: [], skipped: [] };
      }
      case "skills_remove": this.installed = this.installed.filter((s) => s.name !== args?.name); return null;
      default: return null;
    }
  }
}
