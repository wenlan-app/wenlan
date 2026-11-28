// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page, SkillSuggestion, Space } from "../../../lib/tauri";
import { i18n } from "../../../i18n";

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
  removeSaved: vi.fn(),
  revisions: vi.fn(),
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
  removeSavedRule: api.removeSaved,
  getPageRevisions: api.revisions,
  SAVED_FROM_TERMINAL: "terminal",
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
    api.revisions.mockReset().mockResolvedValue({ page_id: page.id, current_version: 3, user_edited: false, entries: [] });
  });

  /** Opens the "…" menu and returns its items by name. */
  const openMenu = async () => {
    fireEvent.click(await screen.findByRole("button", { name: "How I work actions" }));
    return screen.getByRole("menu", { name: "How I work actions" });
  };

  it("UX-SKILL-2 shows actions behind the … menu, real borrowing labels and person-edited lines without internal ids", async () => {
    const { onOpenPage } = renderPage();
    const menu = await openMenu();
    expect(within(menu).getByRole("menuitem", { name: "Update now" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Undo last night" })).toBeInTheDocument();
    expect(within(menu).getByRole("menuitem", { name: "Stop" })).toBeInTheDocument();
    // No row of buttons is left in the page body.
    expect(screen.queryByRole("button", { name: "Update now" })).not.toBeInTheDocument();
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
    tool("antigravity", "Antigravity", { state: "over_limit" }),
    tool("gemini-cli", "Gemini CLI", { state: "blocked", blocked_reason: "read_only" }),
    tool("codex", "Codex", { state: "out_of_date" }),
    tool("kiro", "Kiro", { state: "folder_missing" }),
    tool("zed", "Zed", { connected: false, state: "not_connected" }),
  ];
  /** Each chip in the tools row: its name, its dot, and the state its card says. */
  const headerChips = async () => {
    const header = await screen.findByRole("region", { name: "AI tools these rules are used in" });
    const chips = () => [...header.querySelectorAll<HTMLButtonElement>(".rules-tools-chip")].map((chip) => {
      const card = document.getElementById(chip.getAttribute("aria-describedby") ?? "")!;
      return [chip.querySelector(".rules-tools-name")!.textContent, chip.querySelector(".rules-tools-dot")?.getAttribute("data-dot") ?? null, card.querySelector(".rules-tools-card-state")!.textContent];
    });
    return { header, chips };
  };

  it("UX-SKILL-11, UX-SKILL-8: heads a page that gives a rule used in every Space with the connected AI tools, saying only connected and delivered", async () => {
    api.delivery.mockResolvedValue(everyState);
    // One of this page's rules is used in every Space: those are the rules
    // Wenlan writes into the tools' files.
    api.rules.mockResolvedValue(told([daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1", general: true })]));
    renderPage();
    const { header, chips } = await headerChips();
    // One row: delivered tools first, then connected only, then the ones
    // whose rules were not delivered, with an amber dot.
    await waitFor(() => expect(chips()).toEqual([
      ["Claude Code", null, "connected, rules delivered"],
      ["OpenCode", null, "connected, rules delivered"],
      ["Cursor", null, "connected"],
      ["ChatGPT · Codex", "problem", "connected, not delivered yet"],
      ["Antigravity", "problem", "connected, too long to deliver"],
      ["Gemini CLI", "problem", "connected, file can’t be changed"],
    ]));
    expect(header.querySelectorAll(".rules-tools-row")).toHaveLength(1);
    expect(header.querySelector(".rules-tools-row")).not.toHaveTextContent("Used in");
    fireEvent.click(within(header).getByRole("button", { name: "+2 other connections" }));
    expect(header).toHaveTextContent("Odd");
    // A tool that is not connected is not listed, and nothing claims the AI
    // follows or obeys the rules; no path or machine word shows.
    expect(header).not.toHaveTextContent("Zed");
    expect(header.textContent).not.toMatch(/follow|obey|comply|read_only|\.md|\//i);
  });

  it("UX-SKILL-11, UX-SKILL-8: says only connected on a page whose rules are for its own Space, stopped, or not listed by the daemon", async () => {
    api.delivery.mockResolvedValue(everyState);
    const onlyConnected = [
      ["Claude Code", null, "connected"], ["Cursor", null, "connected"], ["OpenCode", null, "connected"],
      ["ChatGPT · Codex", "problem", "connected, not delivered yet"], ["Antigravity", "problem", "connected, too long to deliver"],
      ["Gemini CLI", "problem", "connected, file can’t be changed"],
    ];
    // Rules for this Space only: none of them is in any tool's file.
    api.rules.mockResolvedValue(told([
      daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1" }),
      // A rule used in every Space that another Space's page gives.
      daemonLine("far-1", "Write in plain words", { source_page_id: "far-page", general: true }),
    ]));
    const first = renderPage();
    expect((await headerChips()).chips()).toEqual(onlyConnected);
    await waitFor(() => expect(api.rules).toHaveBeenCalled());
    expect((await headerChips()).chips()).toEqual(onlyConnected);
    first.unmount();
    // A stopped page gives nothing, whatever the daemon lists.
    api.rules.mockResolvedValue(told([daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1", general: true })]));
    renderPage({ ...page, version: 4, content: content.replace("\n\n", "\n\n<!-- wenlan:not-for-ai-tools -->\n\n") });
    await waitFor(() => expect(api.rules).toHaveBeenCalledTimes(2));
    expect((await headerChips()).chips()).toEqual(onlyConnected);
    expect(document.body).not.toHaveTextContent("rules delivered");
  });

  it("UX-SKILL-11: says so plainly when no AI tool is connected", async () => {
    api.delivery.mockResolvedValue([{ id: "zed", display_name: "Zed", connected: false, method: "file", state: "not_connected", rule_count: 0 }]);
    renderPage();
    expect(await screen.findByText("No AI tool connected")).toBeInTheDocument();
  });

  it("UX-SKILL-2 shows no borrowing section when the record is empty and Stop only adds the opt-out marker", async () => {
    api.records.mockResolvedValue([]);
    renderPage();
    const menu = await openMenu();
    expect(screen.queryByRole("heading", { name: "AI tools borrowed this week" })).not.toBeInTheDocument();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Stop" }));
    await waitFor(() => expect(api.updatePage).toHaveBeenCalled());
    const request = api.updatePage.mock.calls[0][0];
    expect(request.content).toContain("<!-- wenlan:not-for-ai-tools -->");
    expect(request.content).toContain("<!-- wenlan-rule:rule-1 -->");
  });

  it("UX-SKILL-11: each tool chip shows its logo or a short badge, and keyboard focus opens a card with its state, its rules across all Spaces and delivered time", async () => {
    api.delivery.mockResolvedValue([
      tool("claude-code", "Claude Code", { delivered_at: now - 7200, rule_count: 3 }),
      tool("antigravity", "Antigravity", { state: "over_limit" }),
      // Two tools Wenlan has no logo for, whose first letters would read the same.
      tool("vscode", "Mira", { method: "mcp", state: "on_request" }),
      tool("continue", "Muse", { method: "mcp", state: "on_request" }),
    ]);
    api.rules.mockResolvedValue(told([daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1", general: true })]));
    renderPage();
    const claude = await screen.findByRole("button", { name: "Claude Code" });
    expect(claude.querySelector("svg[data-logo='claude-code']")).not.toBeNull();
    expect(screen.getByRole("button", { name: "+2 other connections" })).toBeInTheDocument();
    // The card is closed until the chip is hovered or focused, and is the chip's description.
    const card = document.getElementById(claude.getAttribute("aria-describedby")!)!;
    expect(card).toHaveAttribute("role", "tooltip");
    expect(card).not.toBeVisible();
    fireEvent.focus(claude);
    expect(card).toBeVisible();
    expect(card).toHaveTextContent("Claude Codeconnected, rules delivered3 rules delivered, across all Spaces · 2h ago");
    fireEvent.keyDown(claude, { key: "Escape" });
    expect(card).not.toBeVisible();
    fireEvent.blur(claude);
    const windsurf = screen.getByRole("button", { name: "Antigravity" });
    fireEvent.mouseEnter(windsurf.parentElement!);
    const windsurfCard = screen.getByRole("tooltip");
    expect(windsurfCard).toHaveTextContent(/^Antigravityconnected, too long to deliver$/);
    expect(windsurfCard.querySelector(".rules-tools-dot")).toHaveAttribute("data-dot", "problem");
    // Escape closes a card the pointer opened, wherever the focus is.
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    fireEvent.mouseLeave(windsurf.parentElement!);
    fireEvent.mouseEnter(windsurf.parentElement!);
    // The pointer moving into the card stays inside the chip's item.
    expect(windsurf.parentElement!.contains(screen.getByRole("tooltip"))).toBe(true);
    fireEvent.mouseLeave(windsurf.parentElement!);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    expect(document.querySelector(".rules-tools")!.textContent).not.toMatch(/follow|obey/i);
  });

  it("UX-SKILL-11: a click or tap pins a chip's card: the click after a hover keeps it open, the next click or a press elsewhere closes it", async () => {
    api.delivery.mockResolvedValue([tool("claude-code", "Claude Code", {}), tool("antigravity", "Antigravity", { state: "over_limit" })]);
    renderPage();
    const claude = await screen.findByRole("button", { name: "Claude Code" });
    const item = claude.parentElement!;
    // A mouse: enter, then click. The card the hover opened stays.
    fireEvent.mouseEnter(item);
    fireEvent.click(claude);
    expect(screen.getByRole("tooltip")).toBeVisible();
    fireEvent.mouseLeave(item);
    expect(screen.getByRole("tooltip")).toBeVisible();
    expect(claude).toHaveAttribute("data-pinned", "true");
    fireEvent.click(claude);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    // A tap with no hover pins it open; a press elsewhere closes it.
    fireEvent.click(claude);
    expect(screen.getByRole("tooltip")).toBeVisible();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("UX-SKILL-11: the dots differ in shape as well as colour: a check when delivered, a ring when connected, an exclamation mark when not delivered", async () => {
    api.delivery.mockResolvedValue([
      tool("claude-code", "Claude Code", {}),
      tool("cursor", "Cursor", { method: "mcp", state: "on_request" }),
      tool("antigravity", "Antigravity", { state: "over_limit" }),
    ]);
    api.rules.mockResolvedValue(told([daemonLine("rule-1", "Discuss the approach before writing code", { source_page_id: "page-source-1", general: true })]));
    renderPage();
    await screen.findByRole("button", { name: "Antigravity" });
    const marks = Object.fromEntries([...document.querySelectorAll(".rules-tools-chip .rules-tools-dot")].map((dot) => [dot.getAttribute("data-dot"), dot.innerHTML]));
    expect(Object.keys(marks)).toEqual(["problem"]);
    expect(marks.delivered).toBeUndefined();
    expect(marks.problem).toContain("rules-tools-dot-glyph");
    expect(marks.connected).toBeUndefined();
  });

  const entry = (version: number, at: number, edited_by: string, text: string) => ({ version, at, edited_by, text });

  it("UX-SKILL-2: under the title, says You edited, how many source pages the AI updated it from, and when, from the page's history", async () => {
    api.revisions.mockResolvedValue({ page_id: page.id, current_version: 3, user_edited: true, entries: [
      entry(1, now - 86400 * 3, "agent_create", "v1"),
      entry(2, now - 7200, "agent_refresh", "v2"),
      entry(3, now - 60, "manual_edit", "v3"),
    ] });
    renderPage({ ...page, user_edited: true });
    const status = await screen.findByTestId("how-i-work-status");
    // Two managed lines, from two different pages; the time is the AI's
    // last rules write, not the person's later edit.
    await waitFor(() => expect(status).toHaveTextContent("You edited·AI updated it from 2 sources·2h ago"));
    expect(api.revisions).toHaveBeenCalledWith(page.id, true);
  });

  it("UX-SKILL-2: a teammate's edit, Stop and Undo last night do not read You edited, and an undone night names the write that still stands", async () => {
    api.revisions.mockResolvedValue({ page_id: page.id, current_version: 5, user_edited: true, entries: [
      entry(1, now - 86400 * 2, "agent_refresh", "v1"),
      entry(2, now - 86400, "shared_person:member-2", "v2"),
      entry(3, now - 7200, "agent_refresh", "v3"),
      // Undo last night: the text from before that night's write.
      entry(4, now - 3600, "manual_edit", "v2"),
      entry(5, now - 60, "manual_edit", "v2\n<!-- wenlan:not-for-ai-tools -->"),
    ] });
    renderPage({ ...page, user_edited: true, human_edited_at: now - 60 });
    const status = await screen.findByTestId("how-i-work-status");
    await waitFor(() => expect(status).toHaveTextContent(/^AI updated it from 2 sources·2d ago$/));
    expect(status).not.toHaveTextContent("You edited");
  });

  it("UX-SKILL-2: without the page's history the line names no time and no edit, never the page's modified time, and is gone when nothing is true", async () => {
    let answer: (value: unknown) => void = () => undefined;
    api.revisions.mockReturnValue(new Promise((resolve) => { answer = resolve; }));
    const oneSource = content.replace("(page-source-2)", "(page-source-1)");
    const first = renderPage({ ...page, content: oneSource, user_edited: true, human_edited_at: now - 5, last_modified: new Date().toISOString() });
    const status = await screen.findByTestId("how-i-work-status");
    // While the history loads: the AI part alone.
    expect(status).toHaveTextContent(/^AI updated it from 1 source$/);
    answer({ page_id: page.id, current_version: 3, user_edited: true, entries: [entry(1, now - 7200, "agent_refresh", "v1")] });
    await waitFor(() => expect(status).toHaveTextContent(/^AI updated it from 1 source·2h ago$/));
    first.unmount();
    // A daemon that cannot answer: still no time.
    api.revisions.mockRejectedValue(new Error("older daemon"));
    const second = renderPage({ ...page, version: 8, content: oneSource, last_modified: new Date().toISOString() });
    await waitFor(() => expect(api.revisions).toHaveBeenCalledTimes(2));
    expect(await screen.findByTestId("how-i-work-status")).toHaveTextContent(/^AI updated it from 1 source$/);
    second.unmount();
    // Only the person's own bullets, never edited by them: no line at all.
    renderPage({ ...page, version: 9, content: "# How I work — Wenlan\n\nThese are this Space's approved working rules.\n\n- Keep commits small\n" });
    await screen.findByText("Keep commits small");
    expect(screen.queryByTestId("how-i-work-status")).not.toBeInTheDocument();
  });

  it("UX-SKILL-2: the status line in Traditional and Simplified Chinese", async () => {
    api.revisions.mockResolvedValue({ page_id: page.id, current_version: 2, user_edited: true, entries: [entry(1, now - 7200, "agent_refresh", "v1"), entry(2, now - 30, "manual_edit", "v2")] });
    try {
      await i18n.changeLanguage("zh-Hant");
      const first = renderPage();
      await waitFor(async () => expect(await screen.findByTestId("how-i-work-status")).toHaveTextContent(/^你改過·AI 依 2 個來源更新·2 ?小時前$/));
      first.unmount();
      await i18n.changeLanguage("zh-Hans");
      renderPage({ ...page, version: 5 });
      await waitFor(async () => expect(await screen.findByTestId("how-i-work-status")).toHaveTextContent(/^你改过·AI 根据 2 个来源更新·2 ?小时前$/));
    } finally {
      await i18n.changeLanguage("en");
    }
  });

  it("UX-SKILL-2: Update now reads the page's history again", async () => {
    renderPage();
    await waitFor(() => expect(api.revisions).toHaveBeenCalledTimes(1));
    const menu = await openMenu();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Update now" }));
    await waitFor(() => expect(api.revisions).toHaveBeenCalledTimes(2));
  });

  it("UX-SKILL-2: the … menu greys out Undo last night while no Space can be undone, shows the Stop sentence under Stop, and works from the keyboard", async () => {
    api.getSpace.mockResolvedValue(null);
    renderPage();
    const trigger = await screen.findByRole("button", { name: "How I work actions" });
    expect(trigger).toHaveAttribute("aria-haspopup", "menu");
    fireEvent.click(trigger);
    const menu = screen.getByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Undo last night" })).toBeDisabled();
    const stop = within(menu).getByRole("menuitem", { name: "Stop" });
    expect(stop).toHaveAccessibleDescription("Stop means AI tools can’t read this page right away. The page stays here, and you can start it again later.");
    // The sentence is in the menu only, not in the page body.
    expect(document.querySelectorAll(".how-i-work-stop-note")).toHaveLength(0);
    expect(within(menu).getByRole("menuitem", { name: "Update now" })).toHaveFocus();
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(stop).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitem", { name: "Update now" }));
    await waitFor(() => expect(api.refresh).toHaveBeenCalledTimes(1));
  });

  it("UX-SKILL-2: Undo last night in the menu undoes the Space's night, and a failed action still says so", async () => {
    api.undoNight.mockRejectedValueOnce(new Error("nothing to undo"));
    renderPage();
    const menu = await openMenu();
    await waitFor(() => expect(within(menu).getByRole("menuitem", { name: "Undo last night" })).toBeEnabled());
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Undo last night" }));
    await waitFor(() => expect(api.undoNight).toHaveBeenCalledWith(space.id));
    expect(await screen.findByRole("alert")).toHaveTextContent("That change didn’t finish. Nothing else was changed.");
  });

  it("UX-SKILL-2: a stopped page says so on the page and offers Start in the menu, with the stopped sentence under it", async () => {
    renderPage({ ...page, content: content.replace("\n\n", "\n\n<!-- wenlan:not-for-ai-tools -->\n\n") });
    expect(await screen.findByTestId("how-i-work-stopped")).toHaveTextContent("AI tools can’t read this page. The page is still here; press Start when you want to use it again.");
    const menu = await openMenu();
    const start = within(menu).getByRole("menuitem", { name: "Start" });
    expect(start).toHaveAccessibleDescription("AI tools can’t read this page. The page is still here; press Start when you want to use it again.");
    fireEvent.click(start);
    await waitFor(() => expect(api.updatePage).toHaveBeenCalled());
    expect(api.updatePage.mock.calls[0][0].content).not.toContain("<!-- wenlan:not-for-ai-tools -->");
  });

  it("UX-SKILL-11: a rule carries no tag; only a decision or a pitfall says what it is", async () => {
    renderPage({ ...page, content: `${content}- [pitfall] Staging hangs when the VPN is off\n` });
    const rule = (await screen.findByText("Discuss the approach before writing code")).closest("li")!;
    expect(rule.querySelector(".how-i-work-chip")).toBeNull();
    expect(rule).not.toHaveTextContent(/^Rule/);
    const decision = screen.getByText("Put completed work into the merge queue").closest("li")!;
    expect(decision.querySelector(".how-i-work-chip")).toHaveTextContent("Decision");
    const pitfall = screen.getByText("Staging hangs when the VPN is off").closest("li")!;
    expect(pitfall.querySelector(".how-i-work-chip")).toHaveTextContent("Pitfall");
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
    expect(rows).toEqual(["Use the merge queuewritten by you·Used in every Space"]);
    // "Squash before merging" is on the page, but the daemon does not list it
    // as a rule the person wrote: it is not shown as one.
    expect(list).not.toHaveTextContent("Squash before merging");
    const header = screen.getByRole("region", { name: "AI tools these rules are used in" });
    await waitFor(() => expect(header.querySelector(".rules-tools-row")).toHaveTextContent(/^Claude Code/));
    expect(header.querySelector(".rules-tools-chip .rules-tools-dot")).toBeNull();
    expect(screen.getByRole("tooltip", { hidden: true })).toHaveTextContent("connected, rules delivered");
    expect(header.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("UX-SKILL-11: a Rules-folder page in a Space says only connected, and a page whose bullets the daemon does not list shows no rule list", async () => {
    api.rules.mockResolvedValue({ space_id: "s", space_name: "Wenlan", rules_page_id: null, lines: [line("a", "Use the merge queue", { hand_written: true })] });
    renderHead({ ...folderPage, space: "Wenlan", storage_path: "Wenlan/Rules/git.md" });
    await waitFor(() => expect(api.rules).toHaveBeenCalledWith("Wenlan"));
    expect(await screen.findByText("written by you")).toBeInTheDocument();
    const header = screen.getByRole("region", { name: "AI tools these rules are used in" });
    expect(header.querySelector(".rules-tools-chip .rules-tools-dot")).toBeNull();
    expect(screen.getByRole("tooltip", { hidden: true })).toHaveTextContent(/^Claude Codeconnected$/);
    expect(header).not.toHaveTextContent("delivered");
  });

  describe("the page that keeps the rules the person asked to be saved", () => {
    const savedPage: Page = { ...folderPage, id: "saved-rules", title: "Rules I saved", storage_path: "Rules I saved.md", content: "# Rules I saved\n\n- Release on Tuesdays\n- Answer in plain words\n- Keep pull requests small\n" };
    const saved = (id: string, text: string, more: object = {}) => ({ ...line(id, text, more), source_page_id: savedPage.id, source_page_title: "Rules I saved" });
    const lines = [
      saved("a", "Release on Tuesdays", { saved_from: "Codex", general: true }),
      // Even a line the daemon also marks hand-written is never the person's.
      saved("b", "Answer in plain words", { saved_from: "terminal", general: true, hand_written: true }),
      saved("c", "Keep pull requests small", { hand_written: true, general: true }),
    ];
    beforeEach(() => {
      // The daemon says which pages keep saved rules, by id.
      api.rules.mockResolvedValue({ space_id: "", space_name: "", rules_page_id: null, lines, saved_rules_page_ids: [savedPage.id] });
      api.removeSaved.mockReset();
    });

    it("UX-SKILL-14: says where each rule was saved from, never written by you, and offers Remove only on a saved rule", async () => {
      renderHead(savedPage);
      const list = await screen.findByRole("region", { name: "Rules on this page" });
      const rows = [...list.querySelectorAll("li")].map((row) => row.textContent);
      expect(rows).toEqual([
        "Release on Tuesdayssaved from Codex·Used in every SpaceRemove",
        "Answer in plain wordssaved from the terminal·Used in every SpaceRemove",
        "Keep pull requests smallwritten by you·Used in every Space",
      ]);
      expect(within(list).getAllByRole("button", { name: /^Remove “/ })).toHaveLength(2);
      // The page is in no Rules folder: the daemon's list alone puts it under Rules.
      expect(savedPage.storage_path).not.toContain("Rules/");
    });

    it("UX-SKILL-14: a renamed saved-rules page keeps its rules and their Remove, and a page that only has the title is an ordinary page", async () => {
      const renamed: Page = { ...savedPage, title: "Work conventions", storage_path: "Work conventions.md" };
      renderHead(renamed);
      const list = await screen.findByRole("region", { name: "Rules on this page" });
      expect(within(list).getAllByRole("button", { name: /^Remove “/ })).toHaveLength(2);
      expect(screen.getByRole("region", { name: "AI tools these rules are used in" })).toBeInTheDocument();
    });

    it("UX-SKILL-14: a page somebody titled like the saved-rules page gets no rules header and no Remove", async () => {
      const lookalike: Page = { ...savedPage, id: "made-by-a-tool", title: "Rules I saved — anything", storage_path: "Rules I saved — anything.md" };
      renderHead(lookalike);
      await waitFor(() => expect(api.rules).toHaveBeenCalledWith(null));
      await Promise.resolve();
      expect(screen.queryByRole("region", { name: "AI tools these rules are used in" })).not.toBeInTheDocument();
      expect(screen.queryByRole("region", { name: "Rules on this page" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: /^Remove/ })).not.toBeInTheDocument();
      expect(api.spaceFolders).not.toHaveBeenCalled();
    });

    it("UX-SKILL-14: says where a rule was saved from in the app's language, with a self-named tool's name in quotes", async () => {
      api.rules.mockResolvedValue({
        space_id: "", space_name: "", rules_page_id: null, saved_rules_page_ids: [savedPage.id],
        lines: [
          saved("a", "Release on Tuesdays", { saved_from: "ChatGPT / Codex", saved_by: { kind: "tool", name: "ChatGPT / Codex" } }),
          saved("b", "Answer in plain words", { saved_from: "terminal", saved_by: { kind: "terminal" } }),
          saved("d", "Tag every release", { saved_from: 'an AI tool that calls itself "terminal"', saved_by: { kind: "self_named", name: "terminal" } }),
          saved("e", "Write the changelog", { saved_from: "an AI tool", saved_by: { kind: "unnamed" } }),
        ],
      });
      await i18n.changeLanguage("zh-Hant");
      try {
        renderHead(savedPage);
        const list = await screen.findByRole("region", { name: "這頁的規則" });
        const sources = [...list.querySelectorAll(".how-i-work-line-source > span:first-child")].map((item) => item.textContent);
        expect(sources).toEqual(["從 ChatGPT / Codex 存的", "從終端機存的", "從自稱「terminal」的 AI 工具存的", "從某個 AI 工具存的"]);
        // No English sentence from the daemon is left inside the line.
        expect(list).not.toHaveTextContent(/an AI tool|calls itself|saved from/u);
      } finally {
        await i18n.changeLanguage("en");
      }
      cleanup();
      renderHead(savedPage);
      const list = await screen.findByRole("region", { name: "Rules on this page" });
      expect(within(list).getByText("saved from an AI tool that calls itself “terminal”")).toBeInTheDocument();
      expect(within(list).getByText("saved from an AI tool")).toBeInTheDocument();
    });

    it("UX-SKILL-14: a rule already removed somewhere else leaves the list at the press, and the page says it was already removed", async () => {
      renderHead(savedPage);
      const press = await screen.findByRole("button", { name: "Remove “Release on Tuesdays”" });
      // The daemon no longer has the rule: its answer to the press is a 404,
      // and its fresh list no longer carries the row.
      api.removeSaved.mockRejectedValueOnce(new Error('HTTP POST /api/skills/rules/a/remove returned 404 Not Found: {"error":"rule_save_refused: no saved rule has that id; nothing was removed. `wenlan rules` and read_space_rules list each saved rule with its id"}'));
      api.rules.mockResolvedValue({ space_id: "", space_name: "", rules_page_id: null, lines: lines.slice(1), saved_rules_page_ids: [savedPage.id] });
      const asked = api.rules.mock.calls.length;
      fireEvent.click(press);
      const note = await screen.findByText("That rule was already removed.");
      expect(note).toHaveAttribute("role", "status");
      expect(api.rules.mock.calls.length).toBeGreaterThan(asked);
      await waitFor(() => expect(screen.queryByRole("button", { name: "Remove “Release on Tuesdays”" })).not.toBeInTheDocument());
      expect(screen.getByRole("button", { name: "Remove “Answer in plain words”" })).toBeInTheDocument();
      expect(document.body).not.toHaveTextContent(/rule_save_refused|\/api\/|Couldn’t remove it/u);
    });

    it("UX-SKILL-14: one press on Remove asks the daemon as the person, with no question first", async () => {
      api.removeSaved.mockResolvedValue({ status: "removed", id: "a", page_id: savedPage.id, page_title: "Rules I saved", message: "" });
      renderHead(savedPage);
      fireEvent.click(await screen.findByRole("button", { name: "Remove “Release on Tuesdays”" }));
      await waitFor(() => expect(api.removeSaved).toHaveBeenCalledWith("a"));
      expect(api.removeSaved).toHaveBeenCalledTimes(1);
      // The daemon is asked again for the rules that still count.
      await waitFor(() => expect(api.rules.mock.calls.length).toBeGreaterThan(1));
      expect(screen.queryByRole("status")).not.toBeInTheDocument();
    });

    it("UX-SKILL-14: says so in plain words when the page is locked or the daemon refuses, with no route or id", async () => {
      renderHead(savedPage);
      const press = async () => fireEvent.click(await screen.findByRole("button", { name: "Remove “Release on Tuesdays”" }));
      api.removeSaved.mockResolvedValueOnce({ status: "waiting", id: "a", page_id: savedPage.id, page_title: "Rules I saved", revision_card_id: "card-1", message: "" });
      await press();
      expect(await screen.findByText("This page is locked. Accept its card to remove the rule.")).toBeInTheDocument();
      api.removeSaved.mockRejectedValueOnce(new Error('HTTP POST /api/skills/rules/a/remove returned 403 Forbidden: {"error":"rule_save_refused: a rule saved this way goes to the AI tools of the person at this computer, so a team member cannot save or remove one"}'));
      await press();
      expect(await screen.findByText("Team members can’t remove a saved rule.")).toBeInTheDocument();
      api.removeSaved.mockRejectedValueOnce(new Error("rule_save_refused: a rule can be saved or removed only while Wenlan listens on this computer alone"));
      await press();
      expect(await screen.findByText("Can’t remove while Wenlan can be reached from other computers.")).toBeInTheDocument();
      api.removeSaved.mockRejectedValueOnce(new Error("HTTP POST /api/skills/rules/a/remove returned 500"));
      // Any failure reads the rules again, so a stale row cannot stay.
      const asked = api.rules.mock.calls.length;
      await press();
      const note = await screen.findByText("Couldn’t remove it. Nothing changed.");
      expect(api.rules.mock.calls.length).toBeGreaterThan(asked);
      expect(note.closest("li")).not.toHaveTextContent("/api/");
      expect(screen.getByRole("region", { name: "Rules on this page" })).not.toHaveTextContent("rule_save_refused");
    });
  });

  it("UX-FIND-4: shows nothing on a Docs page, and nothing in a Space's own folder that is only called Rules", async () => {
    renderHead({ ...folderPage, storage_path: "Work/git.md" });
    expect(api.spaceFolders).not.toHaveBeenCalled();
    api.spaceFolders.mockResolvedValue(true);
    renderHead({ ...folderPage, space: "Rules", storage_path: "Rules/git.md" });
    await waitFor(() => expect(api.spaceFolders).toHaveBeenCalled());
    await Promise.resolve();
    expect(screen.queryByRole("region", { name: "AI tools these rules are used in" })).not.toBeInTheDocument();
    // Each page asked the daemon once which pages keep saved rules (with no
    // Space); neither page's own rules were asked for.
    expect(api.rules.mock.calls).toEqual([[null], [null]]);
  });
});
