// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page, SkillSuggestion, Space } from "../../../lib/tauri";

const api = vi.hoisted(() => ({
  enabled: vi.fn(),
  getSpace: vi.fn(),
  records: vi.fn(),
  suggestions: vi.fn(),
  refresh: vi.fn(),
  undoNight: vi.fn(),
  apply: vi.fn(),
  dismiss: vi.fn(),
  undoSuggestion: vi.fn(),
  updatePage: vi.fn(),
  delivery: vi.fn(),
  askSuggestions: vi.fn(),
  rules: vi.fn(),
  spaceFolders: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => ({
  skillsFromLibraryEnabled: api.enabled,
  getSpace: api.getSpace,
  listSkillBorrowRecords: api.records,
  listSkillSuggestions: api.suggestions,
  refreshSkills: api.refresh,
  undoSpaceRules: api.undoNight,
  applySkillSuggestion: api.apply,
  dismissSkillSuggestion: api.dismiss,
  undoSkillSuggestion: api.undoSuggestion,
  updatePage: api.updatePage,
  getRulesDelivery: api.delivery,
  listSpaceRulesSuggestions: api.askSuggestions,
  getSpaceRules: api.rules,
  getSpaceFolders: api.spaceFolders,
}));

import { HowIWorkPage } from "./HowIWorkPage";
import { RulesFolderPageHead } from "./RulesToolsHeader";

const now = Math.floor(Date.now() / 1000);
const content = `# How I work — Wenlan

These are this Space's approved working rules. Wenlan suggests the first version; only the person accepts it.

- [rule] Discuss the approach before writing code — Source: Working style (page-source-1) r2 at ${now - 86400} — Quote: "Discuss the approach before writing code" <!-- wenlan-rule:rule-1 -->
- [decision] Put completed work into the merge queue — Source: Release flow (page-source-2) r4 at ${now - 43200} — Quote: "Push completed work directly to main" <!-- wenlan-rule:decision-1 -->
`;
const page: Page = {
  id: "rules-page",
  title: "How I work — Wenlan",
  summary: null,
  content,
  entity_id: null,
  domain: "Wenlan",
  space: "Wenlan",
  source_memory_ids: [],
  version: 3,
  status: "active",
  creation_kind: "authored",
  created_at: new Date((now - 86400) * 1000).toISOString(),
  last_compiled: new Date((now - 3600) * 1000).toISOString(),
  last_modified: new Date((now - 3600) * 1000).toISOString(),
};
const space: Space = {
  id: "space-wenlan",
  name: "Wenlan",
  description: null,
  suggested: false,
  starred: false,
  sort_order: 0,
  memory_count: 0,
  entity_count: 0,
  created_at: 1,
  updated_at: 1,
};
const suggestion: SkillSuggestion = {
  id: "suggestion-1",
  skill: "release",
  old_line: "Push completed work directly to main",
  new_line: "Put completed work into the merge queue",
  reason: "A newer decision changed the release flow",
  source_rule_id: "decision-1",
  source_page: "Release flow",
};

/** What the daemon lists for this Space, as the app reads it. */
const daemonLine = (id: string, text: string, more: object = {}) => ({ id, kind: "rule", text, source_page_id: page.id, source_page_title: page.title, source_revision: 3, source_revision_at: now, source_quote: text, ...more });
const told = (lines: object[]) => ({ space_id: space.id, space_name: space.name, rules_page_id: page.id, lines });

function renderPage(shown: Page = page, onOpenPage = vi.fn()) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return {
    onOpenPage,
    ...render(
      <QueryClientProvider client={queryClient}>
        <HowIWorkPage page={shown} onOpenPage={onOpenPage} />
      </QueryClientProvider>,
    ),
  };
}

describe("How-I-work page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.enabled.mockResolvedValue(true);
    api.getSpace.mockResolvedValue(space);
    api.records.mockResolvedValue([
      { at: now - 1800, tool: "codex", space_id: space.id, page_title: "Release flow", rule_id: "decision-1", self_report: "changed_plan" },
      { at: now - 3600, tool: "some-tool", space_id: space.id, page_title: "Working style", rule_id: "rule-1" },
    ]);
    api.suggestions.mockResolvedValue([suggestion]);
    api.refresh.mockResolvedValue({ spaces_considered: 1, spaces_unchanged: 1, spaces_updated: 0, suggestions: 0, failures: 0, model_calls: 0 });
    api.undoNight.mockResolvedValue({ space_id: space.id, status: "undone", rules_page_id: page.id, undo_available: false });
    api.apply.mockResolvedValue({ suggestion, applied: true, undo_available: true });
    api.dismiss.mockResolvedValue({ suggestion, applied: false, undo_available: false });
    api.undoSuggestion.mockResolvedValue({ suggestion, applied: false, undo_available: false });
    api.updatePage.mockResolvedValue({ outcome: "saved" });
    api.delivery.mockResolvedValue([]);
    api.askSuggestions.mockResolvedValue([]);
    api.rules.mockResolvedValue(told([]));
    api.spaceFolders.mockReset().mockResolvedValue(false);
  });

  it("UX-SKILL-2 shows actions, real borrowing labels and person-edited lines without internal ids", async () => {
    const { onOpenPage } = renderPage();
    expect(await screen.findByRole("button", { name: "Update now" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Undo last night" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    const borrowed = await screen.findByRole("heading", { name: "AI tools borrowed this week" });
    const borrowedSection = borrowed.closest("section")!;
    expect(within(borrowedSection).getByText(/Reported by the AI tool/)).toBeInTheDocument();
    expect(within(borrowedSection).getByText(/Recorded by Wenlan/)).toBeInTheDocument();
    expect(screen.getByText("You changed this")).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("page-source-1");
    // Every line says where it came from; an edited one still names its page.
    fireEvent.click(screen.getByRole("button", { name: "from “Working style”" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-source-1");
    expect(screen.getByRole("button", { name: "from “Release flow”" })).toBeInTheDocument();
  });

  it("UX-SKILL-11, UX-SKILL-10: says written by you only for a bullet the daemon lists as hand-written from this page with the same words", async () => {
    api.rules.mockResolvedValue(told([
      daemonLine("own-1", "Ask before  changing the plan", { hand_written: true }),
      // The same words, hand-written on another page: not this bullet.
      daemonLine("own-2", "Keep commits small", { hand_written: true, source_page_id: "other-page" }),
      // From this page, but the daemon does not say the person wrote it.
      daemonLine("own-3", "Run the tests first"),
    ]));
    renderPage({ ...page, content: `${content}- Ask before changing the plan\n  - a detail, not a rule\n- [ ] a to-do, not a rule\n- Keep commits small\n- Run the tests first\n- Added by an AI tool\n` });
    expect(api.rules).toHaveBeenCalledWith("Wenlan");
    const own = (await screen.findByText("Ask before changing the plan")).closest("li")!;
    expect(await within(own).findByText("written by you")).toBeInTheDocument();
    expect(within(own).queryByRole("button")).not.toBeInTheDocument();
    // Every other bullet shows no author at all: not the person, not a page.
    for (const text of ["Keep commits small", "Run the tests first", "Added by an AI tool"]) {
      const row = screen.getByText(text).closest("li")!;
      expect(row).not.toHaveTextContent("written by you");
      expect(within(row).queryByRole("button")).not.toBeInTheDocument();
      expect(row.querySelector(".how-i-work-line-source")).toBeNull();
    }
    expect(screen.queryByText(/a detail, not a rule|a to-do, not a rule/)).not.toBeInTheDocument();
    expect(screen.getAllByText("written by you")).toHaveLength(1);
    expect(document.body).not.toHaveTextContent("own:");
  });

  it("UX-SKILL-11, UX-SKILL-10: names no author for any bullet while the daemon has not answered or cannot", async () => {
    api.rules.mockRejectedValue(new Error("older daemon"));
    renderPage({ ...page, content: `${content}- Ask before changing the plan\n` });
    expect(await screen.findByText("Ask before changing the plan")).toBeInTheDocument();
    await waitFor(() => expect(api.rules).toHaveBeenCalled());
    expect(screen.queryByText("written by you")).not.toBeInTheDocument();
  });

  const tool = (id: string, display_name: string, more: object) => ({ id, display_name, connected: true, method: "file", state: "delivered", rule_count: 2, ...more });
  const everyState = [
    tool("claude-code", "Claude Code", {}),
    tool("opencode", "OpenCode", { state: "covered_by_another_file" }),
    // A tool that asks Wenlan has no file: never shown as delivered.
    tool("cursor", "Cursor", { method: "mcp", state: "on_request" }),
    tool("odd", "Odd", { method: "mcp", state: "delivered" }),
    tool("windsurf", "Windsurf", { state: "over_limit" }),
    tool("gemini", "Gemini CLI", { state: "blocked", blocked_reason: "read_only" }),
    tool("codex", "Codex", { state: "out_of_date" }),
    tool("kiro", "Kiro", { state: "folder_missing" }),
    tool("zed", "Zed", { connected: false, state: "not_connected" }),
  ];
  const headerRows = async () => {
    const header = await screen.findByRole("region", { name: "AI tools these rules are used in" });
    return { header, rows: () => [...header.querySelectorAll(".rules-tools-row")].map((row) => row.textContent) };
  };

  it("UX-SKILL-11, UX-SKILL-8: heads a page that gives a rule used in every Space with the connected AI tools, saying only connected and delivered", async () => {
    api.delivery.mockResolvedValue(everyState);
    // One of this page's rules is used in every Space: those are the rules
    // Wenlan writes into the tools' files.
    api.rules.mockResolvedValue(told([daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1", general: true })]));
    renderPage();
    const { header, rows } = await headerRows();
    await waitFor(() => expect(rows()).toEqual([
      "Used inClaude CodeOpenCodeconnected, rules delivered",
      "Used inCursorOddconnected",
      "Used inCodexconnected, not delivered yet",
      "Used inWindsurfconnected, too long to deliver",
      "Used inGemini CLIconnected, file can’t be changed",
      "Used inKiroconnected, not delivered",
    ]));
    // A tool that is not connected is not listed, and nothing claims the AI
    // follows or obeys the rules; no path or machine word shows.
    expect(header).not.toHaveTextContent("Zed");
    expect(header.textContent).not.toMatch(/follow|obey|comply|read_only|\.md|\//i);
  });

  it("UX-SKILL-11, UX-SKILL-8: says only connected on a page whose rules are for its own Space, stopped, or not listed by the daemon", async () => {
    api.delivery.mockResolvedValue(everyState);
    const onlyConnected = ["Used inClaude CodeOpenCodeCursorOddWindsurfGemini CLICodexKiroconnected"];
    // Rules for this Space only: none of them is in any tool's file.
    api.rules.mockResolvedValue(told([
      daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1" }),
      // A rule used in every Space that another Space's page gives.
      daemonLine("far-1", "Write in plain words", { source_page_id: "far-page", general: true }),
    ]));
    const first = renderPage();
    expect((await headerRows()).rows()).toEqual(onlyConnected);
    await waitFor(() => expect(api.rules).toHaveBeenCalled());
    expect((await headerRows()).rows()).toEqual(onlyConnected);
    first.unmount();
    // A stopped page gives nothing, whatever the daemon lists.
    api.rules.mockResolvedValue(told([daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1", general: true })]));
    renderPage({ ...page, version: 4, content: content.replace("\n\n", "\n\n<!-- wenlan:not-for-ai-tools -->\n\n") });
    await waitFor(() => expect(api.rules).toHaveBeenCalledTimes(2));
    expect((await headerRows()).rows()).toEqual(onlyConnected);
    expect(document.body).not.toHaveTextContent("delivered");
  });

  it("UX-SKILL-11: says so plainly when no AI tool is connected", async () => {
    api.delivery.mockResolvedValue([{ id: "zed", display_name: "Zed", connected: false, method: "file", state: "not_connected", rule_count: 0 }]);
    renderPage();
    expect(await screen.findByText("No AI tool connected")).toBeInTheDocument();
  });

  it("UX-SKILL-2 shows no borrowing section when the record is empty and Stop only adds the opt-out marker", async () => {
    api.records.mockResolvedValue([]);
    renderPage();
    await screen.findByRole("button", { name: "Stop" });
    expect(screen.queryByRole("heading", { name: "AI tools borrowed this week" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(api.updatePage).toHaveBeenCalled());
    const request = api.updatePage.mock.calls[0][0];
    expect(request.content).toContain("<!-- wenlan:not-for-ai-tools -->");
    expect(request.content).toContain("<!-- wenlan-rule:rule-1 -->");
  });

  it("UX-SKILL-3 keeps an own-skill conflict suggestion-only, then offers Undo after Apply", async () => {
    renderPage();
    const title = await screen.findByText("Your skill “release” has a line that differs from a newer decision");
    const card = title.closest("article")!;
    expect(within(card).getByText(suggestion.old_line)).toHaveClass("how-i-work-suggestion-old");
    expect(within(card).getByText(suggestion.new_line)).toBeInTheDocument();
    expect(api.apply).not.toHaveBeenCalled();
    fireEvent.click(within(card).getByRole("button", { name: "Change it" }));
    await waitFor(() => expect(api.apply).toHaveBeenCalledExactlyOnceWith(suggestion.id));
    const undo = within(card).getByRole("button", { name: "Undo" });
    fireEvent.click(undo);
    await waitFor(() => expect(api.undoSuggestion).toHaveBeenCalledExactlyOnceWith(suggestion.id));
    await waitFor(() => expect(screen.queryByText(title.textContent ?? "")).not.toBeInTheDocument());
  });
});

describe("a page kept in a Rules folder", () => {
  const folderPage: Page = { ...page, id: "git-rules", title: "Git", space: null, domain: null, storage_path: "Rules/git.md", content: "# Git\n\n- Use the merge queue\n- Squash before merging\n" };
  const line = (id: string, text: string, more: object = {}) => ({ id, kind: "rule", text, source_page_id: folderPage.id, source_page_title: "Git", source_revision: 1, source_revision_at: now, source_quote: text, ...more });
  const tools = [{ id: "claude-code", display_name: "Claude Code", connected: true, method: "file", state: "delivered", rule_count: 1 }];
  function renderHead(shown: Page) {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    const onOpenPage = vi.fn();
    render(<QueryClientProvider client={queryClient}><RulesFolderPageHead page={shown} onOpenPage={onOpenPage} /></QueryClientProvider>);
  }
  beforeEach(() => {
    vi.clearAllMocks();
    api.enabled.mockResolvedValue(false);
    api.askSuggestions.mockResolvedValue([]);
    api.delivery.mockResolvedValue(tools);
    api.spaceFolders.mockReset().mockResolvedValue(false);
  });

  it("UX-SKILL-11: lists each rule the daemon reads from the page with who wrote it, under the same header as a How-I-work page", async () => {
    api.rules.mockResolvedValue({ space_id: "", space_name: "", rules_page_id: null, lines: [
      line("a", "Use the merge queue", { hand_written: true, general: true }),
      line("b", "From another page", { hand_written: true, source_page_id: "other" }),
    ] });
    renderHead(folderPage);
    // In no Space: asked with no Space, where every rule is used in every Space.
    await waitFor(() => expect(api.rules).toHaveBeenCalledWith(null));
    const list = await screen.findByRole("region", { name: "Rules on this page" });
    const rows = [...list.querySelectorAll("li")].map((row) => row.textContent);
    expect(rows).toEqual(["RuleUse the merge queuewritten by you·Used in every Space"]);
    // "Squash before merging" is on the page, but the daemon does not list it
    // as a rule the person wrote: it is not shown as one.
    expect(list).not.toHaveTextContent("Squash before merging");
    const header = screen.getByRole("region", { name: "AI tools these rules are used in" });
    await waitFor(() => expect(header).toHaveTextContent("Used inClaude Codeconnected, rules delivered"));
    expect(header.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("UX-SKILL-11: a Rules-folder page in a Space says only connected, and a page whose bullets the daemon does not list shows no rule list", async () => {
    api.rules.mockResolvedValue({ space_id: "s", space_name: "Wenlan", rules_page_id: null, lines: [line("a", "Use the merge queue", { hand_written: true })] });
    renderHead({ ...folderPage, space: "Wenlan", storage_path: "Wenlan/Rules/git.md" });
    await waitFor(() => expect(api.rules).toHaveBeenCalledWith("Wenlan"));
    expect(await screen.findByText("written by you")).toBeInTheDocument();
    const header = screen.getByRole("region", { name: "AI tools these rules are used in" });
    expect(header).toHaveTextContent("Used inClaude Codeconnected");
    expect(header).not.toHaveTextContent("delivered");
  });

  it("UX-FIND-4: shows nothing on a Docs page, and nothing in a Space's own folder that is only called Rules", async () => {
    renderHead({ ...folderPage, storage_path: "Work/git.md" });
    expect(api.spaceFolders).not.toHaveBeenCalled();
    api.spaceFolders.mockResolvedValue(true);
    renderHead({ ...folderPage, space: "Rules", storage_path: "Rules/git.md" });
    await waitFor(() => expect(api.spaceFolders).toHaveBeenCalled());
    await Promise.resolve();
    expect(screen.queryByRole("region", { name: "AI tools these rules are used in" })).not.toBeInTheDocument();
    expect(api.rules).not.toHaveBeenCalled();
  });
});
