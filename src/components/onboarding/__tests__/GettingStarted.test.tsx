// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  clipboardWrite,
  getBackgroundAiEnabled,
  listAgents,
  listOnboardingMilestones,
  listPagesExplicitBrowse,
  listRegisteredSources,
  openToolWithPrompt,
  type AgentConnection,
  type MilestoneRecord,
  type Page,
  type RegisteredSource,
} from "../../../lib/tauri";
import type { WritePagesState } from "../../memory/askAi/WritePages";
import { GettingStarted } from "../GettingStarted";
import {
  ESTABLISHED_LIBRARY_PAGES,
  gettingStartedProgress,
  useGettingStartedState,
} from "../useGettingStartedState";

vi.mock("../../../lib/tauri", () => ({
  clipboardWrite: vi.fn(),
  getBackgroundAiEnabled: vi.fn(),
  listAgents: vi.fn(),
  listOnboardingMilestones: vi.fn(),
  listPagesExplicitBrowse: vi.fn(),
  listRegisteredSources: vi.fn(),
  openToolWithPrompt: vi.fn(),
  PROMPT_LINK_FAMILIES: ["claude-desktop", "claude-code", "codex", "cursor"],
}));

function page(overrides: Partial<Page> = {}): Page {
  return {
    id: "page-1",
    title: "Trip plan",
    summary: null,
    content: "",
    entity_id: null,
    domain: null,
    space: null,
    source_memory_ids: [],
    version: 1,
    status: "active",
    created_at: "2026-10-01T00:00:00Z",
    last_compiled: "2026-10-01T00:00:00Z",
    last_modified: "2026-10-01T00:00:00Z",
    creation_kind: "authored",
    ...overrides,
  } as Page;
}

function agent(name: string, overrides: Partial<AgentConnection> = {}): AgentConnection {
  return {
    id: `agent-${name}`,
    name,
    display_name: null,
    agent_type: "",
    description: null,
    enabled: true,
    trust_level: "full",
    last_seen_at: 1_790_000_000,
    memory_count: 0,
    created_at: 1_790_000_000,
    updated_at: 1_790_000_000,
    ...overrides,
  };
}

const folder = { id: "src-1", source_type: "directory", path: "/notes" } as RegisteredSource;
const written = page({ id: "page-w", title: "Bread proofing", creation_kind: "distilled", source_memory_ids: ["m1", "m2"] });
const recall = { id: "first-recall", first_triggered_at: 1, acknowledged_at: null, payload: null } as MilestoneRecord;

/** The library the card reads through the shared inventory: active pages and
 *  drafts come from separate lists, as the daemon serves them. */
function mockLibrary(pages: Page[]) {
  vi.mocked(listPagesExplicitBrowse).mockImplementation(async (status) =>
    pages.filter((p) => (status === "draft") === (p.status === "draft")));
}

function renderCard(
  pages: Page[] = [],
  handlers: Partial<Record<"onWriteNote" | "onConnectTool" | "onAddSources" | "onImportChats" | "onSeeExample" | "onTurnOnSteeping", () => void>> = {},
  writePages: WritePagesState | null = null,
) {
  const props = {
    onWriteNote: vi.fn(),
    onConnectTool: vi.fn(),
    onAddSources: vi.fn(),
    onImportChats: vi.fn(),
    onSeeExample: vi.fn(),
    ...handlers,
    writePages,
  };
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  mockLibrary(pages);
  render(
    <QueryClientProvider client={client}>
      <GettingStarted {...props} />
    </QueryClientProvider>,
  );
  return props;
}

const title = () => i18n.t("firstUse.checklist.title");

describe("gettingStartedProgress", () => {
  it("reads every step from the library as it stands", () => {
    expect(gettingStartedProgress({ pages: [], agents: [], milestones: [] }))
      .toEqual({ note: false, tool: false, pages: false, recall: false });
    expect(gettingStartedProgress({
      pages: [page(), written],
      agents: [agent("claude-code")],
     
      milestones: [recall],
    })).toEqual({ note: true, tool: true, pages: true, recall: true });
  });

  it("does not count Wenlan's own identities", () => {
    const progress = gettingStartedProgress({
      pages: [],
      agents: [
        agent("wenlan-app"),
        agent("wenlan-setup"),
        agent("wenlan-cli"),
        agent("manual"),
      ],
     
      milestones: [],
    });
    expect(progress.tool).toBe(false);
  });

  it("counts a tool the app registered before it ever called", () => {
    // Connecting from the app registers the tool with the daemon; the user
    // did the step, so it is done even though the tool has not called yet.
    const progress = gettingStartedProgress({
      pages: [],
      agents: [agent("codex", { last_seen_at: null, memory_count: 0 })],
     
      milestones: [],
    });
    expect(progress.tool).toBe(true);
  });

  it("does not count a draft as a saved note, because recall cannot see it yet", () => {
    expect(gettingStartedProgress({
      pages: [page({ status: "draft" })], agents: [], milestones: [],
    }).note).toBe(false);
  });

  it("does not count copies of brought-in files as a page the user wrote or one written from material", () => {
    for (const kind of ["imported", "source"]) {
      const progress = gettingStartedProgress({
        pages: [page({ creation_kind: kind })], agents: [], milestones: [],
      });
      expect(progress).toMatchObject({ note: false, pages: false });
    }
  });

  it("counts a page written from material as that step, not as writing or asking", () => {
    expect(gettingStartedProgress({
      pages: [written], agents: [], milestones: [],
    })).toMatchObject({ pages: true, note: false, recall: false });
  });
});

describe("GettingStarted", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
    vi.mocked(listAgents).mockResolvedValue([]);
    vi.mocked(listRegisteredSources).mockResolvedValue([]);
    vi.mocked(listOnboardingMilestones).mockResolvedValue([]);
  });

  it("offers each unfinished step with an action that leads to it", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    const props = renderCard();

    expect(await screen.findByRole("heading", { name: title() })).toBeInTheDocument();
    expect(screen.getByText(i18n.t("firstUse.checklist.progress", { done: 1, total: 4 }))).toBeInTheDocument();
    expect(screen.getByTestId("getting-started-tool")).toHaveAttribute("data-done", "true");
    expect(screen.queryByRole("button", { name: i18n.t("firstUse.checklist.steps.tool.action") })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.checklist.steps.note.action") }));
    fireEvent.click(screen.getByTestId("getting-started-pages-folder"));
    fireEvent.click(screen.getByTestId("getting-started-pages-chats"));
    fireEvent.click(screen.getByTestId("getting-started-pages-sample"));
    expect(props.onWriteNote).toHaveBeenCalledOnce();
    expect(props.onAddSources).toHaveBeenCalledOnce();
    expect(props.onImportChats).toHaveBeenCalledOnce();
    expect(props.onSeeExample).toHaveBeenCalledOnce();
    // The example is offered once, in the pages step; asking waits for a page.
    expect(screen.getByTestId("getting-started-recall")).toHaveTextContent(i18n.t("firstUse.checklist.steps.recall.bodyConnected"));
    expect(screen.queryByRole("button", { name: i18n.t("firstUse.checklist.steps.recall.action") })).toBeNull();
    expect(props.onConnectTool).not.toHaveBeenCalled();
  });

  it("stays away when a read fails rather than claim a step is undone", async () => {
    vi.mocked(listAgents).mockRejectedValue(new Error("daemon down"));
    renderCard();
    await vi.waitFor(() => expect(listAgents).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByTestId("getting-started")).toBeNull();
  });

  it("does not appear for an established library or once every step is done", async () => {
    const many = Array.from({ length: ESTABLISHED_LIBRARY_PAGES }, (_, i) => page({ id: `p${i}` }));
    renderCard(many);
    await vi.waitFor(() => expect(listOnboardingMilestones).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByTestId("getting-started")).toBeNull();
  });

  it("stays for a new library whose pages are mostly copies of brought-in files", async () => {
    const copies = Array.from({ length: ESTABLISHED_LIBRARY_PAGES * 2 }, (_, i) =>
      page({ id: `c${i}`, creation_kind: i % 2 ? "source" : "imported" }),
    );
    renderCard([...copies, written]);
    expect(await screen.findByTestId("getting-started")).toBeTruthy();
  });

  it("hides when every step is done", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(listRegisteredSources).mockResolvedValue([folder]);
    vi.mocked(listOnboardingMilestones).mockResolvedValue([recall]);
    renderCard([page(), written]);
    await vi.waitFor(() => expect(listOnboardingMilestones).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.queryByTestId("getting-started")).toBeNull();
  });

  it("steps back to one line when writing a page by hand is all that is left", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(listOnboardingMilestones).mockResolvedValue([recall]);
    const props = renderCard([written]);
    const card = await screen.findByTestId("getting-started");
    expect(card).toHaveTextContent(i18n.t("firstUse.checklist.progress", { done: 3, total: 4 }));
    expect(card).toHaveTextContent(i18n.t("firstUse.checklist.steps.note.title"));
    expect(screen.queryByRole("heading", { name: title() })).toBeNull();
    expect(screen.queryByTestId("getting-started-pages")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.checklist.steps.note.action") }));
    expect(props.onWriteNote).toHaveBeenCalledOnce();
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.checklist.hide") }));
    expect(screen.getByRole("button", { name: i18n.t("firstUse.checklist.reopen") })).toBeInTheDocument();
  });

  it("tells a user whose only note is a draft to leave the editor so AI can find it", async () => {
    renderCard([page({ status: "draft" })]);
    await screen.findByRole("heading", { name: title() });
    expect(screen.getByTestId("getting-started-note")).toHaveAttribute("data-done", "false");
    expect(screen.getByText(i18n.t("firstUse.checklist.steps.note.draftBody"))).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: i18n.t("firstUse.checklist.steps.recall.copy") })).toBeNull();
  });

  it("does not mistake copies of brought-in files for a draft or a page to ask about", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    renderCard([page({ id: "src", title: "ISP Outage Runbook (section_h1)", creation_kind: "source" })]);
    await screen.findByRole("heading", { name: title() });
    expect(screen.getByTestId("getting-started-note")).toHaveAttribute("data-done", "false");
    expect(screen.queryByText(i18n.t("firstUse.checklist.steps.note.draftBody"))).toBeNull();
    expect(screen.queryByText(/ISP Outage Runbook/)).toBeNull();
  });

  it("asks about a page written from material before anything else", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    renderCard([page({ id: "src", title: "ISP Outage Runbook (section_h1)", creation_kind: "source" }), written]);
    await screen.findByRole("heading", { name: title() });
    expect(screen.getByTestId("getting-started-pages")).toHaveAttribute("data-done", "true");
    expect(screen.getByText(i18n.t("firstUse.checklist.steps.recall.question", { title: "Bread proofing" }))).toBeInTheDocument();
  });

  it("keeps the pages step open, and the question back, while the tool is still writing", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    const working = {
      view: { kind: "working", written: [written], writing: true, stuck: false },
      stepView: { kind: "working", written: [written], writing: true, stuck: false },
      onDemand: true,
      loaded: true,
      run: null,
      tool: { family: "claude-code", name: "Claude Code" },
      toolName: "Claude Code",
      opensTool: true,
      command: "/wenlan steep",
      copied: false,
      waiting: null,
      start: vi.fn(),
      copyCommand: vi.fn(),
      close: vi.fn(),
    } as unknown as WritePagesState;
    renderCard([written], {}, working);
    await screen.findByRole("heading", { name: title() });
    expect(screen.getByTestId("getting-started-pages")).toHaveAttribute("data-done", "false");
    expect(screen.getByText(i18n.t("firstUse.checklist.progress", { done: 1, total: 4 }))).toBeInTheDocument();
    expect(screen.queryByText(i18n.t("firstUse.checklist.steps.recall.question", { title: "Bread proofing" }))).toBeNull();
  });

  it("can skip the pages step when there is nothing to bring in", async () => {
    renderCard();
    await screen.findByRole("heading", { name: title() });
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.checklist.steps.pages.skip") }));
    expect(screen.getByTestId("getting-started-pages")).toHaveAttribute("data-done", "true");
    expect(localStorage.getItem("onboarding:pagesStepSkipped")).toBe("1");
  });

  it("says what the pages step will do before there is anything to hand over", async () => {
    renderCard();
    await screen.findByRole("heading", { name: title() });
    expect(screen.getByTestId("getting-started-pages")).toHaveTextContent(i18n.t("firstUse.checklist.steps.pages.body"));
  });

  type AskKey = "prompt" | "question" | "askLine" | "askIn" | "waiting" | "action" | "copy" | "copied";
  const ask = (key: AskKey, options: { tool?: string; title?: string } = {}) =>
    i18n.t(`firstUse.checklist.steps.recall.${key}` as const, options);

  function renderAskStep() {
    renderCard([
      page({ id: "newest", title: "Kyoto itinerary", last_modified: "2026-10-02T00:00:00Z" }),
      page({ id: "older", title: "Older note" }),
    ]);
  }

  it("opens the connected tool with the question about the newest note typed in", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    vi.mocked(openToolWithPrompt).mockResolvedValue(undefined);
    renderAskStep();
    await screen.findByRole("heading", { name: title() });

    const prompt = ask("prompt", { title: "Kyoto itinerary" });
    expect(screen.getByText(ask("question", { title: "Kyoto itinerary" }))).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: ask("askIn", { tool: "Claude Code" }) }));
    await vi.waitFor(() => expect(openToolWithPrompt).toHaveBeenCalledWith("claude-code", prompt));
    expect(clipboardWrite).toHaveBeenCalledWith(prompt);
    expect(await screen.findByText(ask("waiting", { tool: "Claude Code" }))).toBeInTheDocument();
    // The example stays one click away in the header for whoever wants to see it first.
    expect(screen.getAllByRole("button", { name: ask("action") })).toHaveLength(1);
  });

  it("falls back to the copied question when the tool cannot be opened", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    vi.mocked(openToolWithPrompt).mockRejectedValue(new Error("not installed"));
    renderAskStep();
    await screen.findByRole("heading", { name: title() });

    fireEvent.click(screen.getByRole("button", { name: ask("askIn", { tool: "Codex" }) }));
    expect(await screen.findByRole("button", { name: ask("copied") })).toBeInTheDocument();
    expect(clipboardWrite).toHaveBeenCalledWith(ask("prompt", { title: "Kyoto itinerary" }));
    expect(screen.getByText(ask("askLine", { tool: "Codex", title: "Kyoto itinerary" }))).toBeInTheDocument();
  });

  it("only copies the question for a tool no link can open", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("gemini-cli")]);
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    renderAskStep();
    await screen.findByRole("heading", { name: title() });

    fireEvent.click(screen.getByRole("button", { name: ask("copy") }));
    expect(await screen.findByRole("button", { name: ask("copied") })).toBeInTheDocument();
    expect(openToolWithPrompt).not.toHaveBeenCalled();
  });

  it("offers background steeping under the steps while it is off", async () => {
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(false);
    const onTurnOnSteeping = vi.fn();
    renderCard([], { onTurnOnSteeping });
    const line = await screen.findByTestId("getting-started-steep");
    expect(line).toHaveTextContent(i18n.t("firstUse.checklist.steep.line"));
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.checklist.steep.action") }));
    expect(onTurnOnSteeping).toHaveBeenCalledTimes(1);
  });

  it("does not offer steeping once it is on, or when its state is unknown", async () => {
    vi.mocked(getBackgroundAiEnabled).mockResolvedValue(true);
    renderCard([], { onTurnOnSteeping: vi.fn() });
    await screen.findByRole("heading", { name: title() });
    await waitFor(() => expect(getBackgroundAiEnabled).toHaveBeenCalled());
    expect(screen.queryByTestId("getting-started-steep")).toBeNull();
  });

  it("keeps the prompt away until a tool has actually connected", async () => {
    renderCard([page()]);
    await screen.findByRole("heading", { name: title() });
    expect(screen.queryByRole("button", { name: i18n.t("firstUse.checklist.steps.recall.copy") })).toBeNull();
  });

  it("can be hidden, remembers that, and can be shown again", async () => {
    renderCard();
    fireEvent.click(await screen.findByRole("button", { name: i18n.t("firstUse.checklist.hide") }));
    expect(screen.queryByTestId("getting-started")).toBeNull();
    expect(localStorage.getItem("onboarding:gettingStartedDismissed")).toBe("1");

    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.checklist.reopen") }));
    expect(screen.getByRole("heading", { name: title() })).toBeInTheDocument();
    expect(localStorage.getItem("onboarding:gettingStartedDismissed")).toBeNull();
  });
});

describe("useGettingStartedState", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue([]);
    vi.mocked(listAgents).mockResolvedValue([]);
    vi.mocked(listRegisteredSources).mockResolvedValue([]);
    vi.mocked(listOnboardingMilestones).mockResolvedValue([]);
  });

  function renderState() {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    return renderHook(() => useGettingStartedState(), { wrapper });
  }

  it("is not visible until every read has loaded", async () => {
    const { result } = renderState();
    expect(result.current.visible).toBe(false);
    await waitFor(() => expect(result.current.visible).toBe(true));
    expect(result.current.doneCount).toBe(0);
    expect(result.current.dismissed).toBe(false);
  });

  it("counts finished steps and stays visible while collapsed", async () => {
    mockLibrary([page()]);
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    const { result } = renderState();
    await waitFor(() => expect(result.current.visible).toBe(true));
    expect(result.current.doneCount).toBe(2);
    expect(result.current.progress).toMatchObject({ note: true, tool: true, pages: false, recall: false });

    act(() => result.current.setDismissed(true));
    expect(result.current.visible).toBe(true);
    expect(result.current.dismissed).toBe(true);
    expect(localStorage.getItem("onboarding:gettingStartedDismissed")).toBe("1");

    act(() => result.current.setDismissed(false));
    expect(result.current.dismissed).toBe(false);
  });

  it("shares the dismissed choice between two consumers", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const { result } = renderHook(() => ({ a: useGettingStartedState(), b: useGettingStartedState() }), { wrapper });
    await waitFor(() => expect(result.current.a.visible).toBe(true));
    act(() => result.current.a.setDismissed(true));
    expect(result.current.b.dismissed).toBe(true);
    act(() => result.current.b.setDismissed(false));
  });

  it("is not visible when a read fails, the library is established, or all steps are done", async () => {
    vi.mocked(listAgents).mockRejectedValue(new Error("daemon down"));
    const failed = renderState();
    await waitFor(() => expect(listAgents).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(failed.result.current.visible).toBe(false);
    failed.unmount();

    vi.mocked(listAgents).mockResolvedValue([]);
    mockLibrary(Array.from({ length: ESTABLISHED_LIBRARY_PAGES }, (_, i) => page({ id: `p${i}` })));
    const established = renderState();
    await waitFor(() => expect(listPagesExplicitBrowse).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(established.result.current.visible).toBe(false);
    established.unmount();

    mockLibrary([page(), written]);
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(listRegisteredSources).mockResolvedValue([folder]);
    vi.mocked(listOnboardingMilestones).mockResolvedValue([recall]);
    const done = renderState();
    await waitFor(() => expect(listOnboardingMilestones).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(done.result.current.visible).toBe(false);
  });
});
