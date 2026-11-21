// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { invoke } from "@tauri-apps/api/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { toast } from "sonner";
import { getSpaceFolders, knowledgeFolderCreate, knowledgeFoldersList, listSpaceRulesSuggestions, skillsFromLibraryEnabled, type Page, type RulesSetupStatus } from "../../../lib/tauri";
import { readPinnedPages } from "../navigation/pinnedPages";
import { SectionListPage, type SectionListPageProps } from "./SectionListPage";
import { TeamAccessProvider, teamAccessFor } from "../../../web/teamAccess";

const { active, drafts } = vi.hoisted(() => ({ active: vi.fn(), drafts: vi.fn() }));
vi.mock("./listAllPages", () => ({ EXPLICIT_BROWSE_QUERY_POLICY: {}, listAllActivePages: active, listAllDraftPages: drafts, listAllActivePagesExplicitBrowse: active, listAllDraftPagesExplicitBrowse: drafts }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
vi.mock("../../../lib/tauri", async (original) => ({
  ...await original<typeof import("../../../lib/tauri")>(),
  knowledgeFoldersList: vi.fn(),
  knowledgeFolderCreate: vi.fn(),
  getSpaceFolders: vi.fn().mockResolvedValue(false),
  skillsFromLibraryEnabled: vi.fn(),
  listSpaceRulesSuggestions: vi.fn(),
  listShares: vi.fn().mockResolvedValue([]),
}));

const DAY = 86_400_000;
// One time for every page, so pages that change "at the same moment" sort by
// title and never by which millisecond the fixture happened to be built in.
const AT = new Date(Date.now() - DAY - 60_000).toISOString();
const page = (id: string, overrides: Partial<Page> = {}): Page => {
  const at = AT;
  return { id, title: id, summary: null, content: "", entity_id: null, domain: null, source_memory_ids: [], version: 1, status: "active", created_at: at, last_modified: at, last_compiled: at, ...overrides };
};

function setup(props: Partial<SectionListPageProps> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const handlers = { onBrowseFolder: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn(), onOpenPageId: vi.fn(), onFindSkills: vi.fn() };
  render(<QueryClientProvider client={client}><SectionListPage section="docs" {...handlers} {...props} /></QueryClientProvider>);
  return handlers;
}

beforeEach(async () => {
  vi.mocked(invoke).mockReset();
  localStorage.clear();
  await i18n.changeLanguage("en");
  active.mockReset().mockResolvedValue([
    page("Pricing", { storage_path: "pricing.md" }),
    page("Plan", { storage_path: "Work/plan.md" }),
    page("Deep", { storage_path: "Work/Research/deep.md" }),
    page("Git rules", { storage_path: "Rules/git.md" }),
    page("How I work — Wenlan", { space: "Wenlan" }),
    page("Release notes", { storage_path: "Skills/release-notes.md" }),
  ]);
  drafts.mockReset().mockResolvedValue([]);
  vi.mocked(knowledgeFoldersList).mockReset().mockResolvedValue({ folders: [
    { path: "Work", name: "Work", parent_path: "" }, { path: "Work/Research", name: "Research", parent_path: "Work" },
    { path: "Empty", name: "Empty", parent_path: "" }, { path: "Rules", name: "Rules", parent_path: "" }, { path: "Skills", name: "Skills", parent_path: "" },
  ], truncated: false });
  vi.mocked(knowledgeFolderCreate).mockReset();
  vi.mocked(getSpaceFolders).mockReset().mockResolvedValue(false);
  vi.mocked(toast).mockReset();
  vi.mocked(skillsFromLibraryEnabled).mockReset().mockResolvedValue(false);
  vi.mocked(listSpaceRulesSuggestions).mockReset().mockResolvedValue([]);
});

describe("personal rules setup in the section lists", () => {
  const synced: RulesSetupStatus = {
    enabled: true, adopted: true, can_adopt: true, can_restore: true, in_sync: true, proposals: [], review_tools: {},
  };

  it("UX-SKILL-19: keeps a quiet synced line and checks status without running sync", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "rules_setup_status" ? synced : command === "rules_setup_review_list" ? { items: [], review_tools: {} } : undefined);
    setup({ section: "rules" });
    expect(await screen.findByText("Rules setup is in sync.")).toHaveAttribute("role", "status");
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_sync");
  });

  it("UX-SKILL-19: puts pending cards only on Rules and marks their tool chip", async () => {
    const file = ".cursor/AGENTS.md";
    const pending: RulesSetupStatus = {
      ...synced, in_sync: false,
      proposals: [{ id: "1", kind: "scope", files: [file], reason: "Kept for this tool. Share across tools?", diff: "-Old\n+New" }],
      review_tools: { [file]: { id: "cursor", name: "Cursor" } },
    };
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "rules_setup_status") return pending;
      if (command === "rules_setup_review_list") return {
        items: [{ id: "1", kind: "tool_edit", card: pending.proposals[0], answers: ["tool_only", "all_tools", "dismiss"] }],
        review_tools: pending.review_tools,
      };
      if (command === "rules_setup_skill_updates") return [];
      if (command === "get_rules_delivery") return { tools: [
        { id: "cursor", display_name: "Cursor", connected: true, method: "file", state: "delivered", rule_count: 1 },
        { id: "codex", display_name: "Codex", connected: true, method: "file", state: "delivered", rule_count: 1 },
      ] };
      return undefined;
    });
    setup({ section: "rules" });
    const user = userEvent.setup();
    const chip = await screen.findByRole("button", { name: "Cursor needs review" });
    expect(chip.querySelector('[data-dot="problem"]')).toBeVisible();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    await user.click(chip);
    expect(await screen.findByRole("article", { name: "Cursor changed a rule" })).toBeInTheDocument();
    expect(screen.getByText("Cursor changed one thing. Your call.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Codex" }).querySelector('[data-dot="delivered"]')).toBeVisible();
    cleanup();
    setup({ section: "skills" });
    expect(await screen.findByText("Cursor changed one thing. Your call.")).toBeInTheDocument();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
  });

  it("UX-SKILL-19: hides setup when the daemon reports it off", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => command === "rules_setup_status" ? { ...synced, enabled: false } : undefined);
    setup({ section: "skills" });
    expect(await screen.findByRole("button", { name: "Open Release notes" })).toBeInTheDocument();
    expect(screen.queryByTestId("rules-tools")).not.toBeInTheDocument();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith("rules_setup_skill_updates");
  });

  it("UX-SKILL-21: shows a badge on the skill row and a quiet count with no update action", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "rules_setup_status") return synced;
      if (command === "rules_setup_skill_updates") return [
        { name: "Release notes", status: { state: "available", revision: "2" } },
        { name: "still-current", status: { state: "current" } },
      ];
      return undefined;
    });
    setup({ section: "skills" });
    const row = await screen.findByRole("button", { name: "Open Release notes. New version available." });
    expect(within(row).getByText("New version")).toBeInTheDocument();
    expect(screen.getByText("1 skill has a new version")).toHaveAttribute("role", "status");
    expect(screen.queryByText("still-current")).not.toBeInTheDocument();
    expect(screen.queryByRole("article")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Use new version" })).not.toBeInTheDocument();
  });

  it("UX-SKILL-21: keeps the update visible for a local skill with no library page", async () => {
    active.mockResolvedValue([]);
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "rules_setup_status") return synced;
      if (command === "rules_setup_skill_updates") return [{ name: "local-skill", status: { state: "available", revision: "2" } }];
      return undefined;
    });
    setup({ section: "skills" });
    const row = (await screen.findByText("local-skill")).closest("li")!;
    expect(within(row).getByText("New version")).toBeInTheDocument();
    expect(within(row).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByText("No skills yet")).not.toBeInTheDocument();
  });
});

describe("section list page", () => {
  it("UX-FIND-4: Docs shows folders as cards with their page counts, then the pages in no folder with when each last changed", async () => {
    const handlers = setup();
    const user = userEvent.setup();
    expect(await screen.findByRole("heading", { level: 1, name: "Docs" })).toBeInTheDocument();
    const folders = await screen.findByRole("list", { name: "Folders" });
    // Rules and Skills are sections of their own, never folders under Docs.
    expect(within(folders).getAllByRole("button").map((card) => card.textContent)).toEqual(["Work2", "Empty0"]);
    const pages = screen.getByRole("list", { name: "Pages" });
    expect(within(pages).getAllByRole("button").map((row) => row.getAttribute("aria-label"))).toEqual(["Open Pricing"]);
    expect(within(pages).getByText("yesterday")).toBeInTheDocument();
    await user.click(within(folders).getByRole("button", { name: "Work" }));
    expect(handlers.onBrowseFolder).toHaveBeenCalledWith("Work");
    await user.click(screen.getByRole("button", { name: "Open Pricing" }));
    expect(handlers.onOpenPage).toHaveBeenCalledWith(expect.objectContaining({ id: "Pricing" }));
  });

  it("UX-FIND-4: a folder lists its own folders and pages and leads back to Docs", async () => {
    const handlers = setup({ folderPath: "Work" });
    const user = userEvent.setup();
    expect(await screen.findByRole("heading", { level: 1, name: "Work" })).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Research" })).toHaveTextContent("Research1");
    expect(screen.getByRole("button", { name: "Open Plan" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open Deep" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Docs" }));
    expect(handlers.onBrowseFolder).toHaveBeenCalledWith(null);
  });

  it("UX-FIND-4: Skills and Rules list their pages wherever the file is, with no folder cards", async () => {
    setup({ section: "rules" });
    expect(await screen.findByRole("heading", { level: 1, name: "Rules" })).toBeInTheDocument();
    expect((await screen.findAllByRole("button", { name: /^Open / })).map((row) => row.getAttribute("aria-label"))).toEqual(["Open Git rules", "Open How I work · Wenlan"]);
    expect(screen.queryByRole("list", { name: "Folders" })).not.toBeInTheDocument();
  });

  it("UX-FIND-4: a section with nothing in it keeps its page and says so briefly", async () => {
    active.mockResolvedValue([page("Pricing", { storage_path: "pricing.md" })]);
    setup({ section: "skills" });
    expect(await screen.findByText("No skills yet")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "Skills" })).toBeInTheDocument();
  });

  it("New offers a page or a folder under Docs, and the folder is made where the list is", async () => {
    vi.mocked(knowledgeFolderCreate).mockResolvedValue({ path: "Work/Notes" });
    const handlers = setup({ folderPath: "Work" });
    const user = userEvent.setup();
    await screen.findByRole("button", { name: "Open Plan" });
    await user.click(screen.getByRole("button", { name: "New" }));
    await user.click(screen.getByRole("menuitem", { name: "New page" }));
    expect(handlers.onCreatePage).toHaveBeenCalledWith("Work");
    await user.click(screen.getByRole("button", { name: "New" }));
    await user.click(screen.getByRole("menuitem", { name: "New folder" }));
    await user.type(screen.getByRole("textbox", { name: "Folder name" }), "Notes");
    await user.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(knowledgeFolderCreate).toHaveBeenCalledWith("Work", "Notes"));
    await waitFor(() => expect(screen.queryByRole("textbox", { name: "Folder name" })).not.toBeInTheDocument());
  });

  it("a new rules page lands in the Rules folder, which is made when the library has none", async () => {
    const handlers = setup({ section: "rules" });
    const user = userEvent.setup();
    await screen.findByRole("button", { name: "Open Git rules" });
    await user.click(screen.getByRole("button", { name: "New" }));
    expect(handlers.onCreatePage).toHaveBeenCalledWith("Rules");
    expect(knowledgeFolderCreate).not.toHaveBeenCalled();
  });

  it("a new skills page makes the Skills folder first when there is none", async () => {
    vi.mocked(knowledgeFoldersList).mockResolvedValue({ folders: [], truncated: false });
    vi.mocked(knowledgeFolderCreate).mockResolvedValue({ path: "Skills" });
    const handlers = setup({ section: "skills", onFindSkills: undefined });
    const user = userEvent.setup();
    await screen.findByRole("button", { name: "Open Release notes" });
    await waitFor(() => expect(knowledgeFoldersList).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "New" }));
    await waitFor(() => expect(knowledgeFolderCreate).toHaveBeenCalledWith("", "Skills"));
    await waitFor(() => expect(handlers.onCreatePage).toHaveBeenCalledWith("Skills"));
  });

  it("UX-SKILL-12: the Skills list offers Find skills to the person, and not to a team member who is given the same way in", async () => {
    const user = userEvent.setup();
    const handlers = setup({ section: "skills" });
    await screen.findByRole("button", { name: "Open Release notes" });
    await user.click(screen.getByRole("button", { name: "New" }));
    await user.click(within(screen.getByRole("menu", { name: "New" })).getByRole("menuitem", { name: "Find skills" }));
    expect(handlers.onFindSkills).toHaveBeenCalledTimes(1);
    cleanup();

    // A team member who may write: New makes a page, and no menu offers skills.
    const member = teamAccessFor({ member_id: "m", name: "Amy", spaces: [{ space: "Field Notes", access: "write" }] });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const asMember = { onBrowseFolder: vi.fn(), onCreatePage: vi.fn(), onOpenDraft: vi.fn(), onOpenPage: vi.fn(), onOpenPageId: vi.fn(), onFindSkills: vi.fn() };
    render(<QueryClientProvider client={client}><TeamAccessProvider value={member}><SectionListPage section="skills" {...asMember} /></TeamAccessProvider></QueryClientProvider>);
    await screen.findByRole("button", { name: "Open Release notes" });
    const create = screen.getByRole("button", { name: "New" });
    expect(create).not.toHaveAttribute("aria-haspopup");
    await user.click(create);
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Find skills" })).not.toBeInTheDocument();
    expect(asMember.onFindSkills).not.toHaveBeenCalled();
  });

  it("a new rules page never lands in a Space's own Rules folder: the top one is made instead", async () => {
    vi.mocked(getSpaceFolders).mockResolvedValue(true);
    active.mockResolvedValue([page("Team git", { space: "Work", storage_path: "Work/Rules/git.md" })]);
    vi.mocked(knowledgeFoldersList).mockResolvedValue({ folders: [{ path: "Work", name: "Work", parent_path: "" }, { path: "Work/Rules", name: "Rules", parent_path: "Work" }], truncated: false });
    vi.mocked(knowledgeFolderCreate).mockResolvedValue({ path: "Rules" });
    const handlers = setup({ section: "rules" });
    const user = userEvent.setup();
    await screen.findByRole("button", { name: "Open Team git" });
    await waitFor(() => expect(getSpaceFolders).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "New" }));
    await waitFor(() => expect(knowledgeFolderCreate).toHaveBeenCalledWith("", "Rules"));
    await waitFor(() => expect(handlers.onCreatePage).toHaveBeenCalledWith("Rules"));
    expect(handlers.onCreatePage).toHaveBeenCalledTimes(1);
  });

  it("a new rules page is not made anywhere else when its folder cannot be: it says so", async () => {
    vi.mocked(knowledgeFoldersList).mockResolvedValue({ folders: [], truncated: false });
    vi.mocked(knowledgeFolderCreate).mockRejectedValue(new Error("read-only"));
    const handlers = setup({ section: "rules" });
    const user = userEvent.setup();
    await screen.findByRole("button", { name: "Open Git rules" });
    await waitFor(() => expect(knowledgeFoldersList).toHaveBeenCalled());
    await user.click(await screen.findByRole("button", { name: "New" }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith("Couldn’t make the Rules folder."));
    expect(handlers.onCreatePage).not.toHaveBeenCalled();
  });

  it("a new rules page is not made when a Space already owns the top folder called Rules", async () => {
    vi.mocked(getSpaceFolders).mockResolvedValue(true);
    active.mockResolvedValue([page("Plan", { space: "Rules", storage_path: "Rules/plan.md" }), page("How I work — Wenlan", { space: "Wenlan" })]);
    vi.mocked(knowledgeFoldersList).mockResolvedValue({ folders: [{ path: "Rules", name: "Rules", parent_path: "" }], truncated: false });
    const handlers = setup({ section: "rules" });
    const user = userEvent.setup();
    await screen.findByRole("button", { name: "Open How I work · Wenlan" });
    // The Space's page is a Docs page, so it is not listed under Rules.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Open Plan" })).not.toBeInTheDocument());
    await user.click(screen.getByRole("button", { name: "New" }));
    await waitFor(() => expect(toast).toHaveBeenCalledWith("Couldn’t make the Rules folder."));
    expect(knowledgeFolderCreate).not.toHaveBeenCalled();
    expect(handlers.onCreatePage).not.toHaveBeenCalled();
  });

  it("UX-FIND-4: a Space called Rules or Skills keeps its pages under Docs, in its own folder card, never counted and unlisted", async () => {
    vi.mocked(getSpaceFolders).mockResolvedValue(true);
    active.mockResolvedValue([
      page("Plan", { space: "Rules", storage_path: "Rules/plan.md" }),
      page("Deep", { space: "Rules", storage_path: "Rules/Skills/deep.md" }),
      page("Notes", { space: "Skills", storage_path: "Skills/notes.md" }),
      page("Git rules", { space: "Rules", storage_path: "Rules/Rules/git.md" }),
    ]);
    vi.mocked(knowledgeFoldersList).mockResolvedValue({ folders: [
      { path: "Rules", name: "Rules", parent_path: "" }, { path: "Rules/Rules", name: "Rules", parent_path: "Rules" },
      { path: "Rules/Skills", name: "Skills", parent_path: "Rules" }, { path: "Skills", name: "Skills", parent_path: "" },
    ], truncated: false });
    setup();
    const folders = await screen.findByRole("list", { name: "Folders" });
    await waitFor(() => expect(within(folders).getAllByRole("button").map((card) => card.textContent)).toEqual(["Rules1", "Skills1"]));
  });

  it("UX-FIND-4: inside a Space called Rules, its Docs pages are listed and its own Rules folder is not a card", async () => {
    vi.mocked(getSpaceFolders).mockResolvedValue(true);
    active.mockResolvedValue([page("Plan", { space: "Rules", storage_path: "Rules/plan.md" }), page("Git rules", { space: "Rules", storage_path: "Rules/Rules/git.md" })]);
    vi.mocked(knowledgeFoldersList).mockResolvedValue({ folders: [{ path: "Rules", name: "Rules", parent_path: "" }, { path: "Rules/Rules", name: "Rules", parent_path: "Rules" }], truncated: false });
    setup({ folderPath: "Rules" });
    expect(await screen.findByRole("button", { name: "Open Plan" })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Folders" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open Git rules" })).not.toBeInTheDocument();
  });

  it("UX-FIND-6: right-clicking a page in the list pins it, and again unpins it", async () => {
    setup();
    const user = userEvent.setup();
    const row = await screen.findByRole("button", { name: "Open Pricing" });
    await user.pointer({ keys: "[MouseRight]", target: row });
    await user.click(screen.getByRole("menuitem", { name: "Pin" }));
    expect(readPinnedPages()).toEqual([{ id: "Pricing", title: "Pricing" }]);
    await user.pointer({ keys: "[MouseRight]", target: row });
    await user.click(screen.getByRole("menuitem", { name: "Unpin" }));
    expect(readPinnedPages()).toEqual([]);
  });

  it("UX-SKILL-11, UX-SKILL-1: rules the AI drew from pages wait in their own box on the Rules list, with Accept and No", async () => {
    vi.mocked(skillsFromLibraryEnabled).mockResolvedValue(true);
    vi.mocked(listSpaceRulesSuggestions).mockResolvedValue([{ space_id: "s1", space_name: "Wenlan", lines: [{ id: "l1", kind: "rule", text: "Thank the customer first", source_page_id: "src", source_page_title: "Replies", source_revision: 1, source_revision_at: 1, source_quote: "Thank the customer first" }] }]);
    const handlers = setup({ section: "rules" });
    const user = userEvent.setup();
    const box = await screen.findByTestId("space-rules-ask");
    expect(box).toHaveTextContent("WenlanAI drew 1 rule from your pages. Not counted yet.");
    expect(within(box).getByRole("button", { name: "Accept" })).toBeInTheDocument();
    expect(within(box).getByRole("button", { name: "No" })).toBeInTheDocument();
    expect(box.textContent).not.toMatch(/follow|obey/i);
    await user.click(within(box).getByRole("button", { name: "from “Replies”" }));
    expect(handlers.onOpenPageId).toHaveBeenCalledWith("src");
  });

  it("UX-SKILL-11, UX-SKILL-1: every Space with rules waiting shows its own box on the Rules list, not only the first", async () => {
    vi.mocked(skillsFromLibraryEnabled).mockResolvedValue(true);
    const line = (id: string, text: string) => ({ id, kind: "rule" as const, text, source_page_id: "src", source_page_title: "Replies", source_revision: 1, source_revision_at: 1, source_quote: text });
    vi.mocked(listSpaceRulesSuggestions).mockResolvedValue([
      { space_id: "s1", space_name: "Wenlan", lines: [line("l1", "Thank the customer first")] },
      { space_id: "s2", space_name: "Support", lines: [line("l2", "Answer within a day"), line("l3", "Never promise a date")] },
    ]);
    setup({ section: "rules" });
    await waitFor(() => expect(screen.getAllByTestId("space-rules-ask")).toHaveLength(2));
    const [first, second] = screen.getAllByTestId("space-rules-ask");
    expect(first).toHaveTextContent("WenlanAI drew 1 rule from your pages. Not counted yet.");
    expect(second).toHaveTextContent("SupportAI drew 2 rules from your pages. Not counted yet.");
    expect(within(second).getByText("Never promise a date")).toBeInTheDocument();
  });
});
