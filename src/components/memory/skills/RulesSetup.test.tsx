// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { i18n } from "../../../i18n";
import type { PullbackAction, PullbackCard, RulesAdoptionPlan, RulesSetupStatus } from "../../../lib/tauri";
import { RulesSetupReviews } from "./RulesSetupReviews";
import { RulesSetupSettings } from "./RulesSetupSettings";

const file = ".cursor/commands/check.md";
const other = ".codex/prompts/check.md";
const scope: PullbackCard = { id: "r1", kind: "scope", files: [file], reason: "Kept for this tool. Share across tools?", diff: "-Before\n+After\n" };
const conflict: PullbackCard = { ...scope, kind: "conflict", reason: "The tool edited this file." };
const status: RulesSetupStatus = {
  enabled: true, adopted: true, can_adopt: true, can_restore: true, in_sync: false, proposals: [],
  review_tools: { [file]: { id: "cursor", name: "Cursor" }, [other]: { id: "codex", name: "Codex" } },
};
const receipt = { written: [], blocked: [], proposals: [], message: "" };
const plan: RulesAdoptionPlan = {
  token: "approved-token",
  files: [
    { path: ".cursor/AGENTS.md", added: 1, removed: 0, diff: "+Approved change" },
    { path: ".cursor/skills/shared", added: 0, removed: 0, diff: "Contains an unsupported link." },
  ],
  skills: [],
  tools: [{ name: "Cursor", rule_files: 1, skills: 2, other_files: 0 }],
  skipped_files: [".cursor/skills/shared"],
  skipped_tools: ["OpenCode has no tool folder."],
};
const mock = vi.mocked(invoke);
const mount = (node: React.ReactNode) => render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>,
);

beforeEach(async () => {
  mock.mockReset();
  await i18n.changeLanguage("en");
});

describe("setup review", () => {
  it.each([
    [scope, "Use in all tools", "all_tools"],
    [scope, "Keep in Cursor only", "tool_only"],
    [conflict, "Keep in Cursor only", "tool_only"],
    [conflict, "Use in all tools", "all_tools"],
    [conflict, "No", "dismiss"],
  ] as [PullbackCard, string, PullbackAction][])(
    "UX-SKILL-20: previews %s choice %s and writes after confirmation",
    async (card, label, action) => {
      const user = userEvent.setup();
      mock.mockImplementation(async (command) => command === "rules_setup_review_preview"
        ? { plan: "Plan returned by the daemon", diff: "-Preview before\n+Preview after\n" }
        : receipt);
      mount(<RulesSetupReviews status={{ ...status, proposals: [card] }} />);
      const review = within(screen.getByRole("article"));
      expect(review.getByText(card.reason)).toBeInTheDocument();
      expect(review.getByText("check.md")).toBeInTheDocument();
      expect(review.getByText("+After")).toHaveAttribute("data-change", "added");
      const buttons = review.getAllByRole("button").map((button) => button.textContent);
      if (card.kind === "scope") {
        expect(buttons).toEqual(["Use in all tools", "Keep in Cursor only"]);
        expect(review.getByText("This edit already applies in Cursor.")).toBeInTheDocument();
      } else {
        expect(buttons).toEqual(["Keep in Cursor only", "Use in all tools", "No"]);
        expect(review.getByText("No puts Cursor back to the earlier version")).toBeInTheDocument();
      }
      await user.click(review.getByRole("button", { name: label }));
      const dialog = within(await screen.findByRole("alertdialog"));
      const decision = { id: card.id, action };
      expect(invoke).toHaveBeenCalledWith("rules_setup_review_preview", { decision });
      expect(dialog.getByText("Plan returned by the daemon")).toBeInTheDocument();
      expect(dialog.getByText("+Preview after")).toBeInTheDocument();
      expect(invoke).not.toHaveBeenCalledWith("rules_setup_review", expect.anything());
      await user.click(dialog.getByRole("button", { name: action === "dismiss" ? "Restore" : label }));
      expect(mock.mock.calls.filter(([command]) => command === "rules_setup_review")).toEqual([["rules_setup_review", { decision }]]);
    },
  );

  it("UX-SKILL-20: uses the selected file in a competing edit and Escape cancels", async () => {
    mock.mockResolvedValue({ plan: "The selected source is used.", diff: "+Selected" });
    const user = userEvent.setup();
    mount(<RulesSetupReviews status={{ ...status, proposals: [{ ...conflict, files: [file, other] }] }} />);
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
    mount(<RulesSetupReviews status={{ ...status, proposals: [conflict] }} />);
    expect(screen.getByText("選「不要」，Cursor 改回原本的版本")).toBeInTheDocument();
  });

  it("UX-SKILL-20: a rejected preview leaves the review unapplied", async () => {
    const user = userEvent.setup();
    mock.mockRejectedValue(new Error("stale proposal"));
    mount(<RulesSetupReviews status={{ ...status, proposals: [conflict] }} />);
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
      if (command === "rules_setup_adopt_preview") return adoption;
      return receipt;
    });
  }

  it("UX-SKILL-22: shows tools and skips before sending the approved adoption token", async () => {
    answers({ ...status, adopted: false, can_restore: false });
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: "Bring in…" }));
    let dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("Cursor")).toBeInTheDocument();
    expect(dialog.getByText("1 rule file, 2 skills")).toBeInTheDocument();
    expect(dialog.getByText("Contains an unsupported link.")).toBeInTheDocument();
    expect(dialog.getByText("OpenCode has no tool folder.")).toBeInTheDocument();
    expect(dialog.getByText("Where you keep your shared setup stays untouched.")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_adopt", expect.anything());
    await user.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_adopt", expect.anything());
    await user.click(screen.getByRole("button", { name: "Bring in…" }));
    dialog = within(await screen.findByRole("alertdialog"));
    await user.click(dialog.getByRole("button", { name: "Bring in" }));
    expect(mock.mock.calls.filter(([command]) => command === "rules_setup_adopt")).toEqual([["rules_setup_adopt", { token: plan.token }]]);
  });

  it("UX-SKILL-22: a plan containing only skips offers Close", async () => {
    answers({ ...status, adopted: false, can_restore: false }, { ...plan, files: plan.files.slice(1) });
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: "Bring in…" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("There are no new files to bring in.")).toBeInTheDocument();
    await user.click(dialog.getByRole("button", { name: "Close" }));
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_adopt", expect.anything());
  });

  it("UX-SKILL-22: restore explains later edits and requires a second press", async () => {
    answers(status);
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    await user.click(await screen.findByRole("button", { name: "Restore…" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("Edits you made since then stay where they are for review.")).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_restore");
    await user.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_restore");
    await user.click(screen.getByRole("button", { name: "Restore…" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Restore" }));
    expect(mock.mock.calls.filter(([command]) => command === "rules_setup_restore")).toEqual([["rules_setup_restore"]]);
  });

  it("UX-SKILL-19: sync runs only after the person asks in Settings", async () => {
    answers(status);
    const user = userEvent.setup();
    mount(<RulesSetupSettings />);
    const sync = await screen.findByRole("button", { name: "Sync now" });
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_sync");
    await user.click(sync);
    expect(mock.mock.calls.filter(([command]) => command === "rules_setup_sync")).toEqual([["rules_setup_sync"]]);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Tools checked for changes."));
  });

  it("UX-SKILL-19: the off result hides Settings controls", async () => {
    answers({ ...status, enabled: false });
    mount(<RulesSetupSettings />);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("rules_setup_status"));
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(mock.mock.calls.map(([command]) => command)).toEqual(["rules_setup_status"]);
  });
});
