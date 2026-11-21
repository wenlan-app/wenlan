// SPDX-License-Identifier: AGPL-3.0-only
import type {
  AdoptedSkillUpdate, PullbackCard, PullbackDecision, RulesAdoptionPlan,
  RulesSetupStatus, RulesSyncReport, ToolRulesDelivery,
} from "../src/lib/tauri";

export const rulesSetupStates = [
  { id: "synced", label: "In sync" },
  { id: "scope", label: "Rule-line review" },
  { id: "conflict", label: "Conflict review" },
  { id: "skill", label: "New skill version" },
  { id: "adopt", label: "Bring in" },
  { id: "restore", label: "Restore" },
  { id: "off", label: "Setup off" },
] as const;
export type RulesSetupState = typeof rulesSetupStates[number]["id"];

const tools: ToolRulesDelivery[] = [
  { id: "claude-code", display_name: "Claude Code", connected: true, method: "file", state: "delivered", rule_count: 12 },
  { id: "codex", display_name: "Codex", connected: true, method: "file", state: "delivered", rule_count: 12 },
  { id: "cursor", display_name: "Cursor", connected: true, method: "file", state: "delivered", rule_count: 12 },
  { id: "opencode", display_name: "OpenCode", connected: true, method: "file", state: "delivered", rule_count: 12 },
  { id: "gemini-cli", display_name: "Gemini CLI", connected: true, method: "file", state: "delivered", rule_count: 12 },
];

const scope: PullbackCard = {
  id: "scope-preview",
  kind: "scope",
  files: [".cursor/AGENTS.md"],
  diff: "--- .cursor/AGENTS.md\n+++ .cursor/AGENTS.md\n-Run tests before committing.\n+Run tests before committing and stop if they fail.\n",
  reason: "Kept for this tool. Share across tools?",
};
const conflict: PullbackCard = {
  id: "conflict-preview",
  kind: "conflict",
  files: [".cursor/commands/release.md"],
  diff: "--- .cursor/commands/release.md\n+++ .cursor/commands/release.md\n-List merged pull requests.\n+Group merged pull requests by type.\n",
  reason: "Cursor changed the release command after delivery.",
};

const adoption: RulesAdoptionPlan = {
  token: "preview-adoption",
  tools: [
    { name: "Claude Code", rule_files: 1, skills: 5, other_files: 0 },
    { name: "Codex", rule_files: 1, skills: 5, other_files: 2 },
    { name: "Cursor", rule_files: 1, skills: 0, other_files: 1 },
  ],
  files: [
    { path: ".cursor/AGENTS.md", added: 1, removed: 1, diff: scope.diff },
    { path: ".gemini/skills/release-notes", added: 0, removed: 0, diff: "Skipped. This skill folder contains a link." },
  ],
  skipped_files: [".gemini/skills/release-notes"],
  skipped_tools: ["Skipped OpenCode because its tool folder is absent."],
  skills: [{ path: ".codex/skills/release-notes", outcome: "plain_copy" }],
};
const updates: AdoptedSkillUpdate[] = [
  { name: "release-notes", upstream: { kind: "npm", package: "example-release-notes", version: "1.0.0", directory: "" }, status: { state: "available", revision: "1.1.0" } },
];
const report: RulesSyncReport = { written: [], blocked: [], proposals: [], message: "" };

// Fixture mutations change only this page's in-memory state.
let resolved = false;
let adoptionChanged: boolean | undefined;
export function rulesSetupInvoke(command: string, args: Record<string, unknown> | undefined, state: string | null): unknown {
  const proposal = !resolved && (state === "scope" ? scope : state === "conflict" ? conflict : null);
  const adopted = adoptionChanged ?? (state !== "adopt" && state !== "off");
  switch (command) {
    case "rules_setup_status": {
      const status: RulesSetupStatus = {
        enabled: state !== null && state !== "off",
        adopted,
        can_adopt: true,
        can_restore: adopted,
        in_sync: adopted && !proposal,
        proposals: proposal ? [proposal] : [],
        review_tools: proposal ? { [proposal.files[0]]: { id: "cursor", name: "Cursor" } } : {},
      };
      return status;
    }
    case "rules_setup_skill_updates": return state === "skill" ? updates : [];
    case "rules_setup_review_preview": {
      const decision = args?.decision as PullbackDecision;
      if (!proposal || proposal.id !== decision.id) throw new Error("Fixture proposal is gone");
      return { diff: proposal.diff, plan: proposal.reason };
    }
    case "rules_setup_review":
      resolved = true;
      return report;
    case "rules_setup_adopt_preview":
      return adopted ? { ...adoption, tools: [], files: [], skipped_files: [], skipped_tools: [], skills: [] } : adoption;
    case "rules_setup_adopt":
      adoptionChanged = true;
      return report;
    case "rules_setup_restore":
      adoptionChanged = false;
      return report;
    case "rules_setup_sync": return report;
    case "list_space_rules_suggestions": return state ? [] : undefined;
    case "get_rules_delivery": return state ? { tools } : undefined;
    default: return undefined;
  }
}
