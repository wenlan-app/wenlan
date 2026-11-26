// SPDX-License-Identifier: AGPL-3.0-only
import type {
  AdoptedSkillUpdate, PullbackCard, PullbackDecision, RulesAdoptionPlan,
  RulesReviewItem, RulesSetupStatus, RulesSyncReport, ToolRulesDelivery,
} from "../src/lib/tauri";

export const rulesSetupStates = [
  { id: "synced", label: "In sync" },
  { id: "scope", label: "Rule-line review" },
  { id: "conflict", label: "Conflict review" },
  { id: "merged", label: "All review items" },
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

const cursor = { id: "cursor", name: "Cursor" };
const codex = { id: "codex", name: "Codex" };
const gemini = { id: "gemini-cli", name: "Gemini CLI" };

const scope: PullbackCard = {
  id: "scope-preview",
  kind: "scope",
  files: [".cursor/AGENTS.md"],
  diff: "--- .cursor/AGENTS.md\n+++ .cursor/AGENTS.md\n@@ -1,1 +1,1 @@\n-Run tests before committing.\n+Run tests before committing and stop if they fail.\n",
  reason: "Kept for this tool. Share across tools?",
  reason_code: "scope_edit",
};
const conflict: PullbackCard = {
  id: "conflict-preview",
  kind: "conflict",
  files: [".cursor/commands/release.md"],
  diff: "--- .cursor/commands/release.md\n+++ .cursor/commands/release.md\n@@ -1,1 +1,1 @@\n-List merged pull requests.\n+Group merged pull requests by type.\n",
  reason: "Cursor changed the release command after delivery.",
  reason_code: "personal_file_edit",
};

const settings = {
  token: "preview-settings",
  files: [{ path: ".codex/config.toml", added: 2, removed: 0, tool: codex,
    diff: "+++ .codex/config.toml\n+[mcp_servers.calendar]\n+command = \"calendar-server\"" }],
};

const adoption: RulesAdoptionPlan = {
  settings,
  token: "preview-adoption",
  rule_sources: ["~/notes/team-rules.md"],
  tools: [
    { name: "Claude Code", rule_files: 1, skills: 5, other_files: 0 },
    { name: "Codex", rule_files: 1, skills: 5, other_files: 2 },
    { name: "Cursor", rule_files: 1, skills: 0, other_files: 1 },
  ],
  files: [
    { path: ".cursor/AGENTS.md", added: 1, removed: 1, diff: scope.diff, text_diff: { kind: "file_text" }, tool: cursor },
    { path: ".codex/skills/release-notes/SKILL.md", added: 1, removed: 1, tool: codex,
      diff: "--- .codex/skills/release-notes/SKILL.md\n+++ .codex/skills/release-notes/SKILL.md\n@@ -1,1 +1,1 @@\n-Write a release summary.\n+Write a short release summary.\n",
      text_diff: { kind: "file_text" } },
    { path: ".gemini/skills", added: 0, removed: 0, diff: "Skipped. The skills directory is reached through a symlink.",
      tool: gemini, notice_codes: ["linked_skills_root"] },
  ],
  skipped_files: [".gemini/skills"],
  skipped_tools: ["Skipped OpenCode because its tool folder is absent."],
  skipped_tool_codes: [{ code: "tool_missing", tools: ["OpenCode"] }],
  skills: [
    { path: ".codex/skills/release-notes", tool: codex, outcome: "plain_copy",
      byte_copy_reason: "Cannot preserve the native skill during rendering.", byte_copy_reason_code: "skill_byte_copy" },
    { path: ".gemini/skills", tool: gemini, outcome: "skipped",
      reason: "The skills directory is reached through a symlink.", reason_code: "linked_skills_root" },
  ],
  notes: ["Skipped OpenCode because its tool folder is absent."],
  note_codes: [{ code: "tool_missing", tools: ["OpenCode"] }],
};
const updates: AdoptedSkillUpdate[] = [
  { name: "release-notes", upstream: { kind: "npm", package: "example-release-notes", version: "1.0.0", directory: "" }, status: { state: "available", revision: "1.1.0" } },
];
const report: RulesSyncReport = { written: [], blocked: [], proposals: [], message: "" };

// Fixture mutations change only this page's in-memory state.
export function createRulesSetupFixture() {
  const resolved = new Set<string>();
  let adoptionChanged: boolean | undefined;
  return (command: string, args: Record<string, unknown> | undefined, state: string | null): unknown => {
    const cards = (state === "merged" ? [scope, conflict] : state === "scope" ? [scope]
      : state === "conflict" ? [conflict] : []).filter((card) => !resolved.has(card.id));
    const reviewTools = Object.fromEntries(cards.map((card) => [card.files[0], cursor]));
    const items: RulesReviewItem[] = cards.map((card) => ({
      id: card.id, kind: "tool_edit", card,
      answers: card.kind === "scope" ? ["tool_only", "all_tools"] : ["tool_only", "all_tools", "revert"],
    }));
    if (state === "merged") {
      reviewTools["preview-proposal"] = codex;
      items.push({
        id: "preview-proposal", kind: "skill_proposal", answers: ["tool_only", "all_tools", "dismiss"],
        proposal: { id: "preview-proposal", name: "summarize", origin: { kind: "tool", tool: "codex" },
          scope: { kind: "shared" }, files: [{ path: "SKILL.md", text: "Summarize the changes before publishing." }] },
      }, { id: "preview-update", kind: "skill_update", update: updates[0], answers: ["dismiss"] },
      { id: "preview-settings", kind: "settings", plan: settings, answers: ["all_tools"] });
    }
    const adopted = adoptionChanged ?? (state !== "adopt" && state !== "off");
    switch (command) {
      case "rules_setup_status": {
        const status: RulesSetupStatus = {
          enabled: state !== null && state !== "off",
          adopted,
          can_adopt: true,
          can_restore: adopted,
          in_sync: adopted && cards.length === 0 && state !== "merged",
          proposals: cards,
          review_tools: reviewTools,
          last_checked_at: adopted ? 1795266000 : undefined,
          tools: tools.map((tool) => ({ id: tool.id, name: tool.display_name,
            state: !adopted || tool.id === "opencode" ? "not_managed"
              : cards.length && tool.id === "cursor" ? "needs_review" : "up_to_date",
            counts: { rules: 12, skills: 5, commands: 3, subagents: 2, mcp: 1 },
          })),
        };
        return status;
      }
      case "rules_setup_review_list": return { items: items.filter((item) => !resolved.has(item.id)), review_tools: reviewTools };
      case "rules_setup_skill_review": {
        const request = args?.request as { id: string; action: string };
        const item = items.find((item) => item.id === request.id);
        if (request.action === "preview" && item?.kind === "skill_proposal") return {
          status: "review_preview", proposal: item.proposal,
          delivery: { token: "preview-skill", files: [{ after: { permissions: "0600" } }], blocked: [] },
        };
        resolved.add(request.id);
        return { status: request.action === "accept" ? "saved" : "dismissed", delivery: report };
      }
      case "rules_setup_settings_apply":
        resolved.add("preview-settings");
        return report;
      case "rules_setup_dismiss_update":
        resolved.add("preview-update");
        return null;
      case "rules_setup_skill_updates": return state === "skill" ? updates : [];
      case "rules_setup_review_preview": {
        const decision = args?.decision as PullbackDecision;
        const proposal = cards.find((card) => card.id === decision.id);
        if (!proposal) throw new Error("Fixture proposal is gone");
        const code = decision.action === "revert" ? "restore_review"
          : proposal.kind === "scope" ? (decision.action === "all_tools" ? "share_rule" : "keep_rule")
          : (decision.action === "tool_only" ? "personal_only" : "personal_share");
        return { diff: proposal.diff, plan: proposal.reason, plan_codes: [{ code }] };
      }
      case "rules_setup_review":
        resolved.add((args?.decision as PullbackDecision).id);
        return report;
      case "rules_setup_adopt_preview":
        return adopted ? { token: adoption.token, files: [], skills: [] } : adoption;
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
  };
}

export const rulesSetupInvoke = createRulesSetupFixture();
