// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { i18n } from "../../../i18n";
import { resources } from "../../../i18n/resources";
import type { PullbackAction, PullbackCard, RulesAdoptionPlan, RulesReviewList, RulesSetupStatus } from "../../../lib/tauri";
import { RulesSetupDiff } from "./RulesSetupDiff";
import { RulesSetupReviews } from "./RulesSetupReviews";
import { RulesSetupSettings } from "./RulesSetupSettings";
import { useRulesSetupReview } from "./useRulesSetup";
import { createRulesSetupFixture } from "../../../../preview/rulesSetupFixture";

const file = ".cursor/commands/release.md";
const other = ".codex/prompts/check.md";
const ruleFile = ".cursor/AGENTS.md";
const cursor = { id: "cursor", name: "Cursor" };
const codex = { id: "codex", name: "Codex" };
const raw = "Daemon-only text from /home/scratch/settings";
const diff = (path: string, before: string, after: string) =>
  `--- ${path}\n+++ ${path}\n@@ -1,1 +1,1 @@\n-${before}\n+${after}\n`;
const scope: PullbackCard = { id: "r1", kind: "scope", files: [ruleFile], reason: raw,
  reason_code: "scope_edit", diff: diff(ruleFile, "Before", "After") };
const conflict: PullbackCard = { ...scope, kind: "conflict", files: [file],
  diff: diff(file, "Before", "After"), reason_code: "personal_file_edit" };
const status: RulesSetupStatus = {
  enabled: true, adopted: true, can_adopt: true, can_restore: true, in_sync: false, proposals: [],
  review_tools: { [ruleFile]: cursor, [file]: cursor, [other]: codex },
};
const receipt = { written: [], blocked: [], proposals: [], message: "" };
const plan: RulesAdoptionPlan = {
  token: "approved-token",
  files: [
    { path: ruleFile, added: 1, removed: 1, diff: raw, tool: cursor,
      text_diff: { kind: "extracted", diff: diff(ruleFile, "原本的規則", "新的規則") } },
    { path: ".cursor/skills/local/examples/notes.txt", added: 1, removed: 1, diff: raw, tool: cursor,
      text_diff: { kind: "extracted", diff: diff(".cursor/skills/local/examples/notes.txt", "原本", "新版") } },
    { path: ".copilot/agents/release.agent.md", added: 0, removed: 0, diff: raw,
      tool: { id: "copilot-cli", name: "Copilot CLI" }, notice_codes: ["personal_name"] },
    { path: ".cursor/skills", added: 0, removed: 0, diff: raw, tool: cursor,
      notice_codes: ["linked_skills_root"] },
  ],
  skills: [
    { path: ".cursor/skills/local", tool: cursor, outcome: "plain_copy", byte_copy_reason: raw, byte_copy_reason_code: "skill_byte_copy" },
    { path: ".cursor/skills/shared", tool: cursor, outcome: "skipped", reason: raw, reason_code: "shared_skill" },
    { path: ".codex/skills/remote", tool: codex, outcome: "tracked_upstream",
      upstream: { kind: "github", repository: "example/remote", directory: "tools/skills", checkout: "C:\\scratch\\repo" } },
    { path: ".agents/skills/handmade", outcome: "plain_copy" },
  ],
  tools: [{ name: "Cursor", rule_files: 1, skills: 2, other_files: 0 }],
  skipped_files: [".cursor/skills", ".copilot/agents/release.agent.md"],
  skipped_tools: ["OpenCode has no tool folder at .config/opencode."],
  skipped_tool_codes: [{ code: "tool_missing", tools: ["OpenCode"] }],
  notes: [raw],
  note_codes: [{ code: "git_rule", tools: ["Cursor"] }],
};
function queue(setup: Pick<RulesSetupStatus, "proposals" | "review_tools">): RulesReviewList {
  return { review_tools: setup.review_tools, items: setup.proposals.map((card) => ({
    id: card.id, card, kind: "tool_edit",
    answers: card.kind === "scope" ? ["tool_only", "all_tools"] : ["tool_only", "all_tools", "revert"],
  })) };
}

function ToolReviews({ status }: { readonly status: RulesSetupStatus }) {
  return <RulesSetupReviews review={queue(status)} />;
}

const mock = vi.mocked(invoke);
const mount = (node: React.ReactNode) => render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>,
);

beforeEach(async () => {
  mock.mockReset();
  await i18n.changeLanguage("en");
});

it("UX-SKILL-24: shows a new scope card while Rules stays open without Sync", async () => {
  function OpenRulesReviews() {
    const review = useRulesSetupReview(true);
    return review.data ? <RulesSetupReviews review={review.data} /> : null;
  }
  vi.useFakeTimers();
  const rule = ".cursor/AGENTS.md";
  const card = {
    ...scope, files: [rule],
    diff: diff(rule, "Before", "Edited in Cursor"),
  };
  mock.mockResolvedValueOnce(queue(status)).mockResolvedValue(queue({
    proposals: [card], review_tools: { [rule]: cursor },
  }));
  const view = mount(<OpenRulesReviews />);
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(20); });
    expect(mock.mock.calls).toEqual([["rules_setup_review_list"]]);
    expect(screen.queryByRole("article")).toBeNull();

    await act(async () => { await vi.advanceTimersByTimeAsync(15_000); });
    expect(screen.getByRole("article")).toHaveTextContent("+ Edited in Cursor");
    expect(mock.mock.calls).toEqual([["rules_setup_review_list"], ["rules_setup_review_list"]]);
  } finally {
    view.unmount();
    vi.useRealTimers();
  }
});

describe("setup review", () => {
  it.each([
    ["en", "Use in all tools", "Keep in Cursor only", "No"],
    ["zh-Hant", "全部工具都用", "只留在 Cursor", "不要"],
    ["zh-Hans", "全部工具都用", "只留在 Cursor", "不要"],
  ])("UX-SKILL-20: shows the exact scope and conflict choices in order in %s", async (locale, all, only, no) => {
    await i18n.changeLanguage(locale);
    const review = queue({ ...status, proposals: [scope, { ...conflict, id: "r2" }] });
    // An older daemon can still advertise the hidden scope alias.
    review.items[0].answers.push("dismiss");
    mount(<RulesSetupReviews review={review} />);
    const [scopeCard, conflictCard] = screen.getAllByRole("article");
    const labels = (card: HTMLElement) => within(card).getAllByRole("button").map((button) => button.textContent);
    expect(labels(scopeCard)).toEqual([all, only]);
    expect(labels(conflictCard)).toEqual([all, only, no]);
  });

  it.each([
    [scope, "Use in all tools", "all_tools"],
    [scope, "Keep in Cursor only", "tool_only"],
    [conflict, "Use in all tools", "all_tools"],
    [conflict, "Keep in Cursor only", "tool_only"],
    [conflict, "No", "revert"],
  ] as [PullbackCard, string, PullbackAction][])(
    "UX-SKILL-20: previews %s choice %s and writes after confirmation",
    async (card, label, action) => {
      const user = userEvent.setup();
      mock.mockImplementation(async (command) => command === "rules_setup_review_preview"
        ? { plan: raw, plan_codes: [{ code: "restore_review" }], diff: diff(card.files[0], "Preview before", "Preview after") }
        : receipt);
      mount(<ToolReviews status={{ ...status, proposals: [card] }} />);
      const review = within(screen.getByRole("article"));
      expect(review.getByText("− Before")).toHaveAttribute("data-change", "removed");
      expect(review.getByText("+ After")).toHaveAttribute("data-change", "added");
      if (card.kind === "scope") {
        expect(review.getByText("This edit already applies in Cursor.")).toBeInTheDocument();
        expect(review.queryByText("This edit is already kept in its tool.")).not.toBeInTheDocument();
        expect(review.queryByText("Cursor")).not.toBeInTheDocument();
      } else {
        expect(review.getByText("A command or agent changed.")).toBeInTheDocument();
        expect(review.getByText("Cursor · release")).toBeInTheDocument();
        expect(review.getByText("No puts Cursor back to the earlier version")).toBeInTheDocument();
      }
      await user.click(review.getByRole("button", { name: label }));
      const dialog = within(await screen.findByRole("alertdialog"));
      const decision = { id: card.id, action };
      expect(invoke).toHaveBeenCalledWith("rules_setup_review_preview", { decision });
      expect(dialog.getByText("Restore the last approved version.")).toBeInTheDocument();
      expect(dialog.getByText(card.kind === "scope" ? "Cursor" : "Cursor · release")).toBeInTheDocument();
      expect(dialog.getByText("+ Preview after")).toBeInTheDocument();
      expect(invoke).not.toHaveBeenCalledWith("rules_setup_review", expect.anything());
      await user.click(dialog.getByRole("button", { name: action === "revert" ? "Restore" : label }));
      expect(mock.mock.calls.filter(([command]) => command === "rules_setup_review")).toEqual([["rules_setup_review", { decision }]]);
    },
  );

  it("UX-SKILL-20: uses the selected file in a competing edit and Escape cancels", async () => {
    mock.mockResolvedValue({ plan: raw, plan_codes: [{ code: "personal_only" }], diff: diff(other, "Before", "Selected") });
    const user = userEvent.setup();
    mount(<ToolReviews status={{ ...status, proposals: [{ ...conflict, files: [file, other] }] }} />);
    await user.selectOptions(screen.getByRole("combobox", { name: "Version to keep" }), other);
    await user.click(screen.getByRole("button", { name: "Keep in Codex only" }));
    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith("rules_setup_review_preview", { decision: { id: conflict.id, action: "tool_only", file: other } });
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_review", expect.anything());
  });

  it("UX-SKILL-20: localizes the restore consequence in Traditional Chinese", async () => {
    await i18n.changeLanguage("zh-Hant");
    mount(<ToolReviews status={{ ...status, proposals: [conflict] }} />);
    expect(screen.getByText("選「不要」，Cursor 改回原本的版本")).toBeInTheDocument();
  });

  it("UX-SKILL-20: a rejected preview leaves the review unapplied", async () => {
    const user = userEvent.setup();
    mock.mockRejectedValue(new Error("stale proposal"));
    mount(<ToolReviews status={{ ...status, proposals: [conflict] }} />);
    await user.click(screen.getByRole("button", { name: "No" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t open the preview.");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(mock.mock.calls.map(([command]) => command)).toEqual(["rules_setup_review_preview"]);
  });
});

describe("setup settings", () => {
  function answers(setup: RulesSetupStatus, adoption: RulesAdoptionPlan = plan) {
    mock.mockImplementation(async (command) => {
      if (command === "rules_setup_status") return setup;
      if (command === "rules_setup_review_list") return queue(setup);
      if (command === "rules_setup_adopt_preview") return adoption;
      return receipt;
    });
  }

  it.each([
    ["en", "Edit your shared rules in Wenlan from now on."],
    ["zh-Hant", "之後請在 Wenlan 編輯共用規則。"],
    ["zh-Hans", "今后请在 Wenlan 编辑共用规则。"],
  ])("UX-SKILL-24: shows the source migration notice once in %s", async (locale, notice) => {
    await i18n.changeLanguage(locale);
    answers({ ...status, rule_import_finished: true });
    const view = mount(<RulesSetupSettings />);
    expect(await screen.findByText(notice)).toHaveAttribute("role", "status");
    expect(screen.getAllByText(notice)).toHaveLength(1);
    view.unmount();

    answers(status);
    mount(<RulesSetupSettings />);
    await screen.findByTestId("rules-tools");
    expect(screen.queryByText(notice)).toBeNull();
  });

  it.each([
    ["en", "Bring in", "Status", "To review", "Restore"],
    ["zh-Hant", "搬進來", "狀態", "待確認", "還原"],
    ["zh-Hans", "搬进来", "状态", "待确认", "还原"],
  ])("UX-SKILL-25: names and orders the setup actions in %s", async (locale, adopt, standing, review, restore) => {
    await i18n.changeLanguage(locale);
    answers({ ...status, adopted: false, can_restore: false });
    mount(<RulesSetupSettings />);
    await screen.findByRole("button", { name: adopt });
    expect(screen.getAllByRole("button").map((button) => button.textContent)).toEqual([adopt, standing, review]);
    expect(screen.getByRole("button", { name: standing })).toHaveAttribute("aria-pressed", "true");

    answers(status);
    const view = mount(<RulesSetupSettings />);
    const adopted = within(view.container);
    await adopted.findByRole("button", { name: restore });
    expect(adopted.queryByRole("button", { name: adopt })).toBeNull();
    expect(adopted.getAllByRole("button").slice(0, 3).map((button) => button.textContent)).toEqual([standing, review, restore]);
  });

  it.each([
    ["en", "To review", "5 changes to review", ["12 rules", "5 skills", "3 commands", "2 subagents", "1 MCP server"]],
    ["zh-Hant", "待確認", "5 處改動待確認", ["12 條規則", "5 個技能", "3 個指令", "2 個子代理", "1 個 MCP 伺服器"]],
    ["zh-Hans", "待确认", "5 处改动待确认", ["12 条规则", "5 个技能", "3 条命令", "2 个子代理", "1 个 MCP 服务器"]],
  ] as const)("UX-SKILL-25: uses the merged 5-item count and localized tool totals in %s", async (locale, review, count, totals) => {
    await i18n.changeLanguage(locale);
    const fixture = createRulesSetupFixture();
    mock.mockImplementation(async (command) => fixture(command, undefined, "merged"));
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    const button = await screen.findByRole("button", { name: `${review} 5` });
    expect(within(screen.getByTestId("rules-tools")).getByRole("status")).toHaveTextContent(count);
    const cursorRow = screen.getAllByRole("listitem").find((row) => row.querySelector("strong")?.textContent === "Cursor");
    for (const total of totals) expect(cursorRow).toHaveTextContent(total);
    await user.click(button);
    expect(screen.getAllByRole("article")).toHaveLength(5);
  });

  it("UX-SKILL-22: includes settings and skips before sending one approved adoption token", async () => {
    answers({ ...status, adopted: false, can_restore: false }, {
      ...plan,
      settings: { token: "settings-token", files: [{ path: ".codex/config.toml", added: 1, removed: 0,
        diff: "+++ .codex/config.toml\n+[mcp_servers.calendar]", tool: status.review_tools[other] }] },
    });
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: "Bring in" }));
    let dialog = within(await screen.findByRole("alertdialog"));
    const toolSummary = dialog.getByText("1 rule file, 2 skills").closest("li")!;
    expect(within(toolSummary).getByText("Cursor")).toBeInTheDocument();
    expect(dialog.getByText("Kept elsewhere, left as is.")).toBeInTheDocument();
    expect(dialog.getByText("OpenCode · This tool’s setup is missing.")).toBeInTheDocument();
    expect(dialog.queryByText(/leaves the file in place/)).toBeNull();
    expect(dialog.getByText("MCP servers, hooks and permissions")).toBeVisible();
    expect(dialog.getByText("+ [mcp_servers.calendar]")).toBeVisible();
    expect(dialog.queryByText(/config\.toml/)).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_adopt", expect.anything());
    await user.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_adopt", expect.anything());
    await user.click(screen.getByRole("button", { name: "Bring in" }));
    dialog = within(await screen.findByRole("alertdialog"));
    await user.click(dialog.getByRole("button", { name: "Bring in" }));
    expect(mock.mock.calls.filter(([command]) => command === "rules_setup_adopt")).toEqual([["rules_setup_adopt", { token: plan.token }]]);
  });

  it.each([
    ["en", "Bring in", "Wenlan imports ~/notes/team rules.md once and leaves the file in place."],
    ["zh-Hans", "搬进来", "Wenlan 只导入一次 ~/notes/team rules.md，原文件会留在原处。"],
    ["zh-Hant", "搬進來", "Wenlan 只匯入一次 ~/notes/team rules.md，原始檔案會留在原處。"],
  ] as const)("UX-SKILL-22: names the discovered rules source in %s", async (locale, bringIn, copy) => {
    await i18n.changeLanguage(locale);
    answers({ ...status, adopted: false, can_restore: false }, { ...plan, rule_sources: ["~/notes/team rules.md"] });
    mount(<RulesSetupSettings />);
    await userEvent.click(await screen.findByRole("button", { name: bringIn }));
    expect(within(await screen.findByRole("alertdialog")).getByText(copy)).toBeVisible();
  });

  it("UX-SKILL-22: a plan containing only skips offers Close", async () => {
    answers({ ...status, adopted: false, can_restore: false }, { ...plan, files: plan.files.filter((entry) => plan.skipped_files?.includes(entry.path)) });
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: "Bring in" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("There are no new files to bring in.")).toBeInTheDocument();
    await user.click(dialog.getByRole("button", { name: "Close" }));
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_adopt", expect.anything());
  });

  it("UX-SKILL-22: restore explains later edits and requires a second press", async () => {
    answers(status);
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: "Restore" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("Edits you made since then stay where they are for review.")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_restore");
    await user.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_restore");
    await user.click(screen.getByRole("button", { name: "Restore" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restore" }));
    expect(mock.mock.calls.filter(([command]) => command === "rules_setup_restore")).toEqual([["rules_setup_restore"]]);
  });

  it("UX-SKILL-19: Status reads without running Sync now", async () => {
    answers(status);
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    const sync = await screen.findByRole("button", { name: "Sync now" });
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_sync");
    await user.click(sync);
    expect(mock.mock.calls.filter(([command]) => command === "rules_setup_sync")).toEqual([["rules_setup_sync"]]);
    await waitFor(() => expect(screen.getByRole("status", { name: "Tools checked for changes." })).toBeInTheDocument());
  });

  it("UX-SKILL-19: uses the action spacing and moves the selected styling between panels", async () => {
    const fixture = createRulesSetupFixture();
    mock.mockImplementation(async (command) => fixture(command, undefined, "synced"));
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    const sync = await screen.findByRole("button", { name: "Sync now" });
    expect(sync.closest(".rules-setup-actions")?.previousElementSibling).toHaveClass("rules-setup-plan-list");
    const standing = screen.getByRole("button", { name: "Status", pressed: true });
    const review = screen.getByRole("button", { name: "To review", pressed: false });
    const accent = "bg-[var(--mem-accent-indigo)]";
    expect(standing).toHaveClass(accent, "text-[var(--mem-text-on-accent)]");
    expect(review).not.toHaveClass(accent);
    await user.click(review);
    expect(review).toHaveAttribute("aria-pressed", "true");
    expect(review).toHaveClass(accent, "text-[var(--mem-text-on-accent)]");
    expect(standing).not.toHaveClass(accent);
    expect(screen.queryByRole("button", { name: "Sync now" })).not.toBeInTheDocument();
  });

  it("UX-SKILL-19: the off result hides Settings controls", async () => {
    answers({ ...status, enabled: false });
    mount(<RulesSetupSettings />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("rules_setup_status"));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(mock.mock.calls.map(([command]) => command)).toEqual(["rules_setup_status"]);
  });
});

describe("UX-SKILL-23: localized setup messages and labels without paths or filenames", () => {
  const messages = resources["zh-Hant"].translation.rulesSetup.messages;
  const english = resources.en.translation.rulesSetup.messages;
  const pathInWord = /[\w.~][\\/]\S|(?:^|\s)[\\/]\w/u;
  const assertSafe = (text: string) => {
    expect(text).not.toMatch(pathInWord);
    expect(text).not.toMatch(/\.(?:md|toml|txt)\b/iu);
    expect(text).not.toContain(raw);
  };

  it("UX-SKILL-23: renders every code through Traditional Chinese feedback", async () => {
    await i18n.changeLanguage("zh-Hant");
    const codes = Object.keys(messages) as (keyof typeof messages)[];
    mock.mockImplementation(async (command) => command === "rules_setup_status" ? status
      : command === "rules_setup_review_list" ? queue(status) : {
      ...receipt,
      blocked: codes.map((code) => `${raw}: ${english[code]}`),
      blocked_codes: codes.map((code) => ({ code })),
    });
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: i18n.t("rulesSetup.sync") }));
    const feedback = await screen.findByRole("status", { name: i18n.t("rulesSetup.setupRemaining") });
    expect(within(feedback).getAllByRole("listitem").map((line) => line.textContent)).toEqual(codes.map((code) => messages[code]));
    for (const code of codes) {
      expect(feedback.textContent).not.toContain(english[code]);
    }
    assertSafe(feedback.textContent ?? "");
  });

  it("UX-SKILL-23: uses the same fallback for unknown or missing review, plan and feedback codes", async () => {
    await i18n.changeLanguage("zh-Hant");
    mock.mockImplementation(async (command) => {
      if (command === "rules_setup_status") return status;
      if (command === "rules_setup_review_list") return queue(status);
      if (command === "rules_setup_review_preview") return { plan: raw, diff: "", plan_codes: [{ code: "toString" }] };
      return { ...receipt, blocked: [raw, raw], blocked_codes: [{ code: "future_status" }] };
    });
    const user = userEvent.setup();
    mount(<>
      <ToolReviews status={{ ...status, proposals: [{ ...conflict, reason_code: undefined }] }} />
      <RulesSetupSettings />
    </>);
    expect(within(screen.getByRole("article")).getByText(messages.unknown)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("rulesSetup.no") }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(messages.unknown)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await user.click(await screen.findByRole("button", { name: i18n.t("rulesSetup.sync") }));
    const feedback = await screen.findByRole("status", { name: i18n.t("rulesSetup.setupRemaining") });
    expect(within(feedback).getAllByText(messages.unknown)).toHaveLength(2);
    assertSafe(document.body.textContent ?? "");
  });

  it("UX-SKILL-23: shows plain diff rows and labels the source with only its tool and item", async () => {
    await i18n.changeLanguage("zh-Hant");
    const windowsFile = "C:\\scratch\\.codex\\agents\\reviewer.toml";
    const reviewTools = { [file]: cursor, [windowsFile]: codex };
    const changes = diff(file, "參考 .cursor/settings", "改用 \"/home/with spaces/new.md\"")
      + diff(windowsFile, "原本", "-- 參考 C:\\Users\\private\\folder")
      + "\\ No newline at end of file\n"
      + raw;
    const competing = { ...conflict, files: [file, windowsFile], diff: changes, reason_code: "competing_personal_files" };
    mock.mockResolvedValue({ diff: changes, plan: raw, plan_codes: [{ code: "restore_review" }] });
    const user = userEvent.setup();
    mount(<ToolReviews status={{ ...status, proposals: [competing], review_tools: reviewTools }} />);
    expect(screen.getByText(messages.competing_personal_files)).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Cursor · release" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Codex · reviewer" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("rulesSetup.no") }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(messages.restore_review)).toBeInTheDocument();
    for (const panel of [screen.getByRole("article"), dialog]) {
      const preview = panel.querySelector<HTMLDivElement>(".rules-setup-diff")!;
      expect(preview).not.toHaveTextContent(/@@|Cursor|Codex/u);
      expect(within(preview).getByText("− 參考 ［已隱藏路徑］")).toHaveAttribute("data-change", "removed");
      expect(within(preview).getByText("+ -- 參考 ［已隱藏路徑］")).toHaveAttribute("data-change", "added");
      expect(within(preview).queryByText(messages.no_final_newline)).not.toBeInTheDocument();
      assertSafe(panel.textContent ?? "");
    }
    expect(within(dialog).getByText("Cursor · release")).toBeInTheDocument();
    expect(within(dialog).getByText("Codex · reviewer")).toBeInTheDocument();
  });

  it("UX-SKILL-23: shows the newline note only when the newline is the sole change", () => {
    const newlineOnly = diff(ruleFile, "Same text", "Same text").replace("+Same text\n", "\\ No newline at end of file\n+Same text\n");
    const { container, rerender } = render(<RulesSetupDiff diff={newlineOnly} />);
    expect(container.querySelector(".rules-setup-diff")).toHaveTextContent(/^No newline at the end\.$/u);
    rerender(<RulesSetupDiff diff={newlineOnly + diff(file, "-- Existing marker", "++ New marker")}
      codes={["no_final_newline", "wider_read"]} />);
    const preview = container.querySelector(".rules-setup-diff")!;
    expect(preview.querySelector("[data-change=removed]")).toHaveTextContent("− -- Existing marker");
    expect(preview.querySelector("[data-change=added]")).toHaveTextContent("+ ++ New marker");
    expect(preview.querySelector("[data-change=added]")).toHaveAttribute("dir", "auto");
    expect(preview).toHaveTextContent(english.wider_read);
    expect(preview).not.toHaveTextContent(/No newline|Same text/u);
  });

  it("UX-SKILL-23: localizes every adoption section and labels only tools and items", async () => {
    await i18n.changeLanguage("zh-Hant");
    mock.mockImplementation(async (command) => command === "rules_setup_status" ? { ...status, adopted: false }
      : command === "rules_setup_review_list" ? queue(status) : plan);
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: i18n.t("rulesSetup.adoptOpen") }));
    const dialog = await screen.findByRole("alertdialog");
    for (const key of ["fileChanges", "skillSources", "planNotes"] as const) {
      await user.click(within(dialog).getByText(i18n.t(`rulesSetup.${key}`)));
    }
    expect(within(dialog).getByRole("heading", { name: "Cursor" })).toBeVisible();
    expect(within(dialog).getByRole("heading", { name: "Cursor · local" })).toBeVisible();
    expect(within(dialog).getByText("Copilot CLI · release")).toBeVisible();
    expect(within(dialog).getByText("這個技能放在別處，先不動。")).toBeVisible();
    expect(within(dialog).getByText(messages.shared_skill)).toBeVisible();
    expect(within(dialog).getByText("Cursor · shared")).toBeVisible();
    expect(within(dialog).getByText("Codex · remote")).toBeVisible();
    expect(within(dialog).getByText(`${i18n.t("rulesSetup.sharedSource")} · handmade`)).toBeVisible();
    expect(within(dialog).getByText(`OpenCode · ${messages.tool_missing}`)).toBeVisible();
    expect(within(dialog).getByText(`Cursor · ${messages.git_rule}`)).toBeVisible();
    expect(within(dialog).getByText(new RegExp(messages.skill_byte_copy))).toBeVisible();
    expect(within(dialog).getByText(/example · remote/u)).toBeVisible();
    assertSafe(dialog.textContent ?? "");
  });
});
