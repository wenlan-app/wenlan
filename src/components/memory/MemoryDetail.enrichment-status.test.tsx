// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import MemoryDetail from "./MemoryDetail";
import * as tauri from "../../lib/tauri";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof tauri>();
  return Object.fromEntries(Object.entries(actual).map(([key, value]) => [
    key,
    typeof value === "function" ? vi.fn() : value,
  ]));
});

vi.mock("./context/KnowledgeContext", () => ({
  default: ({ onNavigateMemory, onNavigatePage, onNavigateEntity }: {
    onNavigateMemory?: (id: string) => void;
    onNavigatePage?: (id: string) => void;
    onNavigateEntity?: (id: string) => void;
  }) => <section aria-label="Knowledge context">
    <button onClick={() => onNavigateMemory?.("linked-memory")}>Linked memory</button>
    <button onClick={() => onNavigatePage?.("linked-page")}>Used in page</button>
    <button onClick={() => onNavigateEntity?.("linked-entity")}>Linked topic</button>
  </section>,
}));

vi.mock("./editor/MarkdownEditor", async () => {
  const React = await import("react");
  return {
    MarkdownEditor: (props: any) => {
      const editor = React.useRef<HTMLDivElement>(null);
      React.useEffect(() => { if (editor.current) editor.current.textContent = props.initialDocument; }, [props.sessionId, props.initialDocument]);
      return <div ref={editor} role="textbox" aria-label={props.ariaLabel} aria-multiline="true"
        aria-disabled={props.disabled} contentEditable={!props.disabled} suppressContentEditableWarning
        onInput={event => {
          const next = event.currentTarget.textContent ?? "";
          props.onDocumentChange(next);
        }}
        onBlur={event => props.onSave(event.currentTarget.textContent ?? "")}
        onKeyDown={event => { if (event.key === "Escape") props.onCancel(); }}>
        {props.initialDocument}
      </div>;
    },
  };
});

const memory: tauri.MemoryItem = {
  source_id: "mem-1",
  title: "Memory",
  content: "A memory",
  summary: null,
  memory_type: "fact",
  domain: null,
  source_agent: null,
  confidence: null,
  confirmed: false,
  pinned: false,
  supersedes: null,
  last_modified: 1,
  chunk_count: 1,
};

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

async function openMemoryInfo(user = userEvent.setup()) {
  await user.click(await screen.findByRole("button", { name: "Info" }));
  return screen.getByRole("dialog", { name: "Info" });
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

describe("MemoryDetail Info and history", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.clearAllMocks();
    window.localStorage.clear();
    vi.stubGlobal("matchMedia", vi.fn(() => ({
      matches: false,
      media: "(min-width: 1100px)",
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    } as unknown as MediaQueryList)));
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue(memory);
    vi.mocked(tauri.listSpaces).mockResolvedValue([]);
    vi.mocked(tauri.listEntities).mockResolvedValue([]);
    vi.mocked(tauri.listAllTags).mockResolvedValue({
      tags: [],
      document_tags: {},
      categories: [],
      document_categories: {},
    });
    vi.mocked(tauri.setDocumentTags).mockResolvedValue([]);
    vi.mocked(tauri.suggestTags).mockResolvedValue([]);
    vi.mocked(tauri.search).mockResolvedValue([]);
    vi.mocked(tauri.getEnrichmentStatus).mockRejectedValue(new Error("old daemon"));
    vi.mocked(tauri.getVersionChain).mockResolvedValue([]);
    vi.mocked(tauri.getPendingRevision).mockResolvedValue(null);
    vi.mocked(tauri.acceptPendingRevision).mockResolvedValue({
      target_source_id: "mem-1",
      revision_source_id: "rev-1",
      wrote: true,
    });
    vi.mocked(tauri.getMemoryRevisions).mockResolvedValue({
      current_source_id: "mem-1",
      chain_depth: 1,
      entries: [],
    });
  });

  it("keeps internal enrichment diagnostics out of the memory Info reading surface", async () => {
    vi.mocked(tauri.getEnrichmentStatus).mockResolvedValue({
      source_id: "mem-1",
      summary: "complete",
      steps: [{ step: "classify", status: "done", error: null, attempts: 1 }],
    });

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeInTheDocument();
    expect(screen.queryByText(/complete/i)).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByRole("dialog", { name: "Info" })).toBeInTheDocument();
    expect(tauri.getEnrichmentStatus).not.toHaveBeenCalled();
    expect(screen.queryByText(/enrichment/i)).toBeNull();
  });

  it("UX-ADD-4 shows an imported chat turn as a chat transcript over its stored type, with no type to pick and nothing to confirm", async () => {
    // The flag keeps the row's stored type; the detail must not show it.
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
      ...memory,
      source_id: "import_claude_c1",
      content: "User: hi\nAssistant: hello",
      memory_type: "preference",
      conversation_text: true,
    });
    render(
      <MemoryDetail
        sourceId="import_claude_c1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );
    const info = await openMemoryInfo();
    expect(within(info).getByTestId("memory-detail-conversation-text")).toHaveTextContent("Conversation");
    expect(within(info).queryByText("Preference")).toBeNull();
    expect(within(info).queryByRole("button", { name: /confirm content/i })).toBeNull();
  });

  it("keeps a navigable shell while memory detail is loading", () => {
    vi.mocked(tauri.getMemoryDetail).mockReturnValue(
      new Promise<tauri.MemoryItem | null>(() => {}),
    );

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    const dossier = screen.getByRole("main", { name: /memory dossier/i });
    expect(within(dossier).getByRole("button", { name: /back to memories/i })).toBeInTheDocument();
  });

  it("renders the quiet dossier structure without losing core controls", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
      ...memory,
      title: "Durable design note",
      content: "Primary reading content",
      memory_type: "preference",
      domain: "work",
      source_agent: "codex",
      confirmed: true,
      pinned: true,
      structured_fields: JSON.stringify({ source: "design review" }),
    });
    vi.mocked(tauri.listAllTags).mockResolvedValue({
      tags: ["design", "workflow"],
      document_tags: { "memory::mem-1": ["design"] },
      categories: [],
      document_categories: {},
    });
    vi.mocked(tauri.getEnrichmentStatus).mockResolvedValue({
      source_id: "mem-1",
      summary: "complete",
      steps: [{ step: "classify", status: "done", error: null, attempts: 1 }],
    });
    vi.mocked(tauri.getMemoryRevisions).mockResolvedValue({
      current_source_id: "mem-1",
      chain_depth: 2,
      entries: [
        {
          source_id: "mem-1",
          depth: 0,
          title: "Durable design note",
          content_preview: "Current version",
          last_modified: 10,
          delta_summary: "Current dossier copy",
        },
      ],
    });
    vi.mocked(tauri.listEntities).mockResolvedValue([
      {
        id: "entity-1",
        name: "Wenlan",
        entity_type: "project",
        domain: "work",
        source_agent: null,
        confidence: null,
        confirmed: true,
        created_at: 1,
        updated_at: 1,
        memory_count: 1,
        status: "established",
        established_by: "manual",
      },
    ]);
    vi.mocked(tauri.search).mockResolvedValue([
      {
        id: "related-1",
        source_id: "mem-2",
        source: "memory",
        title: "Related memory",
        url: null,
        content: "Related context appears here",
        chunk_index: 0,
        last_modified: 2,
        score: 0.9,
        memory_type: "fact",
      },
    ]);

    const onNavigateEntity = vi.fn();
    const onNavigateMemory = vi.fn();

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={onNavigateEntity}
        onNavigateMemory={onNavigateMemory}
      />,
      { wrapper },
    );

    expect(await screen.findByText("Primary reading content")).toBeInTheDocument();
    const dossier = screen.getByRole("main", { name: /memory dossier/i });
    const backButton = within(dossier).getByRole("button", { name: /back to memories/i });
    const reading = within(dossier).getByRole("region", { name: /memory reading/i });
    expect(backButton.textContent).toBe("");
    expect(backButton.querySelector("svg")).not.toBeNull();
    expect(within(reading).getByText("Primary reading content")).toBeVisible();
    expect(within(reading).getByRole("textbox", { name: "Memory reading" })).toBeInTheDocument();
    expect(within(reading).queryByText(/complete/i)).toBeNull();
    expect(within(reading).queryByText(/current dossier copy/i)).toBeNull();
    expect(within(reading).queryByText("Memory information")).toBeNull();
    expect(within(reading).queryByText("Similar memories")).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(within(dossier).queryByRole("menuitemcheckbox")).toBeNull();
    expect(dossier.querySelector(".memory-detail-eyebrow-row")).toBeNull();
    expect(dossier.querySelector(".memory-detail-rail")).toBeNull();
    expect(tauri.listEntities).not.toHaveBeenCalled();

    await userEvent.click(within(dossier).getByRole("button", { name: /memory actions/i }));
    expect(within(dossier).getByRole("menuitemcheckbox", { name: /pinned yes/i })).toBeInTheDocument();
    expect(within(dossier).queryByRole("menuitemcheckbox", { name: /confirmed/i })).toBeNull();
    await userEvent.click(within(dossier).getByRole("button", { name: "Info" }));
    const panel = screen.getByRole("dialog", { name: "Info" });
    expect(within(panel).getByRole("button", { name: /edit tags/i })).toBeInTheDocument();
    expect(within(panel).queryByText(/complete/i)).toBeNull();
    expect(within(panel).getByText(/current dossier copy/i)).toBeVisible();
    await userEvent.click(within(panel).getByRole("button", { name: "Linked topic" }));
    expect(onNavigateEntity).toHaveBeenCalledWith("linked-entity");
    expect(screen.getByRole("dialog", { name: "Info" })).toBeVisible();
    await userEvent.click(screen.getByText("Similar memories", { selector: "summary" }));
    await userEvent.click(screen.getByRole("button", { name: /related context appears here/i }));
    expect(onNavigateMemory).toHaveBeenCalledWith("mem-2");
    expect(screen.getByRole("dialog", { name: "Info" })).toBeVisible();
  });

  it("orders Info content consistently and removes exact duplicates of the current memory", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
      ...memory,
      content: "A memory",
      structured_fields: JSON.stringify({
        claim: " A\n   memory ",
        project_focus: "Design review",
      }),
      source_text: "Original source excerpt",
    });
    vi.mocked(tauri.search).mockResolvedValue([{
      id: "related-1",
      source_id: "mem-2",
      source: "memory",
      title: "Related memory",
      url: null,
      content: "A separate related memory",
      chunk_index: 0,
      last_modified: 2,
      score: 0.9,
      memory_type: "fact",
    }]);
    vi.mocked(tauri.getMemoryRevisions).mockResolvedValue({
      current_source_id: "mem-1",
      chain_depth: 2,
      entries: [
        {
          source_id: "mem-1",
          depth: 0,
          title: "Current memory",
          content_preview: " A   memory ",
          last_modified: 10,
          delta_summary: "Refined from the source",
        },
        {
          source_id: "mem-0",
          depth: 1,
          title: "Earlier memory",
          content_preview: "Earlier distinct content",
          last_modified: 5,
        },
      ],
    });

    render(
      <MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeVisible();
    const panel = await openMemoryInfo();
    const confirmAction = within(panel).getByRole("button", { name: /confirm content/i });
    const details = panel.querySelector(".memory-detail-information");
    const additional = panel.querySelector(".memory-detail-structured");
    const source = panel.querySelector(".memory-detail-source");
    const connections = panel.querySelector(".memory-detail-connections");
    const similar = within(panel).getByText("Similar memories", { selector: "summary" });
    const history = within(panel).getByText(/revision history/i);
    expect(details).not.toBeNull();
    expect(additional).not.toBeNull();
    expect(source).not.toBeNull();
    expect(connections).not.toBeNull();
    if (!details || !additional || !source || !connections) throw new Error("Expected ordered Info sections");
    expect(confirmAction.compareDocumentPosition(details) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(details.compareDocumentPosition(additional) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(additional.compareDocumentPosition(source) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(source.compareDocumentPosition(connections) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(connections.compareDocumentPosition(similar) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(similar.compareDocumentPosition(history) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(panel).queryByText("A memory", { selector: ".memory-detail-field-value" })).toBeNull();
    expect(within(panel).getByText("Design review")).toBeVisible();
    expect(within(panel).getByText("Project focus")).toBeVisible();
    expect(within(panel).queryByText(/depth 0/i)).toBeNull();
    expect(within(panel).queryByText(" A   memory ")).toBeNull();
    await userEvent.click(within(panel).getByRole("button", { name: /show all 2/i }));
    expect(within(panel).getByText("Earlier distinct content")).toBeVisible();
    expect(tauri.getEnrichmentStatus).not.toHaveBeenCalled();
  });

  it("opens the action menu at the matching keyboard edge and returns focus on Escape", async () => {
    const user = userEvent.setup();
    render(
      <MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />,
      { wrapper },
    );
    expect(await screen.findByText("A memory")).toBeVisible();
    const trigger = screen.getByRole("button", { name: /memory actions/i });
    trigger.focus();
    await user.keyboard("{ArrowUp}");
    const menu = screen.getByRole("menu", { name: /memory actions/i });
    expect(menu).toHaveAttribute("data-keyboard-navigation", "true");
    expect(screen.getByRole("menuitem", { name: /delete memory/i })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).toBeNull();
    expect(trigger).toHaveFocus();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("saves edited memory content from the redesigned reading column", async () => {
    const user = userEvent.setup();
    vi.mocked(tauri.updateMemory).mockImplementation(async (_id, content) => {
      vi.mocked(tauri.getMemoryDetail).mockResolvedValue({ ...memory, content: content ?? memory.content });
    });

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeInTheDocument();

    const editor = screen.getByRole("textbox", { name: "Memory reading" });
    await user.clear(editor);
    await user.type(editor, "Updated reading content");
    fireEvent.blur(editor);

    await waitFor(() => {
      expect(tauri.updateMemory).toHaveBeenCalledWith("mem-1", "Updated reading content", undefined, undefined, undefined, "A memory");
    });
    expect(screen.getByRole("textbox", { name: "Memory reading" })).toHaveTextContent("Updated reading content");
  });

  it.each([
    ["Linked memory", "linked-memory", "onNavigateMemory"],
    ["Used in page", "linked-page", "onNavigatePage"],
    ["Linked topic", "linked-entity", "onNavigateEntity"],
  ] as const)("keeps the context panel open through the %s navigation callback", async (label, id, callback) => {
    const callbacks = { onNavigateMemory: vi.fn(), onNavigatePage: vi.fn(), onNavigateEntity: vi.fn() };
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} {...callbacks} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    await userEvent.click(screen.getByRole("button", { name: label }));
    expect(callbacks[callback]).toHaveBeenCalledWith(id);
    expect(screen.getByRole("dialog", { name: "Info" })).toBeVisible();
  });

  it("summarizes long related context until the user expands it", async () => {
    const user = userEvent.setup();
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
      ...memory,
      domain: "work",
    });
    vi.mocked(tauri.listEntities).mockResolvedValue(
      Array.from({ length: 5 }, (_, index) => ({
        id: `entity-${index + 1}`,
        name: `Entity ${index + 1}`,
        entity_type: "project",
        domain: "work",
        source_agent: null,
        confidence: null,
        confirmed: true,
        created_at: 1,
        updated_at: 1,
        memory_count: 1,
        status: "established",
        established_by: "manual",
      })),
    );
    vi.mocked(tauri.search).mockResolvedValue(
      Array.from({ length: 5 }, (_, index) => ({
        id: `related-${index + 1}`,
        source_id: `mem-${index + 2}`,
        source: "memory",
        title: `Related memory ${index + 1}`,
        url: null,
        content: `Related memory ${index + 1}`,
        chunk_index: 0,
        last_modified: index + 2,
        score: 0.9,
        memory_type: "fact",
      })),
    );

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Info" }));
    await user.click(await screen.findByText("Similar memories", { selector: "summary" }));
    expect(screen.queryByText("Entity 4")).toBeNull();
    expect(tauri.listEntities).not.toHaveBeenCalled();

    const relatedMemoriesSummary = screen.getByText("Similar memories", { selector: "summary" });
    const relatedMemoriesPanel = relatedMemoriesSummary.closest("details");
    expect(relatedMemoriesPanel).not.toBeNull();
    if (!relatedMemoriesPanel) throw new Error("Missing related memories panel");
    expect(within(relatedMemoriesPanel).getByText("Related memory 3")).toBeInTheDocument();
    expect(within(relatedMemoriesPanel).queryByText("Related memory 4")).toBeNull();
    const relatedMemoriesShowAll = within(relatedMemoriesPanel).getByRole("button", { name: /show all 5/i });
    expect(relatedMemoriesShowAll).toHaveClass("memory-detail-disclosure-button");
    expect(relatedMemoriesShowAll.querySelector(".memory-detail-disclosure-label")).not.toBeNull();
    expect(relatedMemoriesShowAll.querySelector(".memory-detail-disclosure-count")).not.toBeNull();
    await user.click(relatedMemoriesShowAll);
    expect(within(relatedMemoriesPanel).getByText("Related memory 5")).toBeInTheDocument();
  });

  it("keeps rendering the memory when enrichment status route is unavailable", async () => {
    vi.mocked(tauri.getEnrichmentStatus).mockRejectedValue(new Error("404"));

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeInTheDocument();
    expect(screen.queryByText(/enrichment/i)).toBeNull();
  });

  it("renders daemon memory revision history", async () => {
    vi.mocked(tauri.getEnrichmentStatus).mockRejectedValue(new Error("old daemon"));
    vi.mocked(tauri.getMemoryRevisions).mockResolvedValue({
      current_source_id: "mem-1",
      chain_depth: 2,
      entries: [
        {
          source_id: "mem-1",
          depth: 0,
          title: "Current",
          content_preview: "Current version",
          last_modified: 10,
          source_agent: "claude-code",
          supersede_mode: "protected_revision",
          delta_summary: "Clarified wording",
        },
      ],
    });

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(await screen.findByText(/revision history/i)).toBeVisible();
    expect(screen.getByText(/clarified wording/i)).toBeInTheDocument();
  });

  it("keeps rendering the memory when memory revisions route is unavailable", async () => {
    vi.mocked(tauri.getEnrichmentStatus).mockRejectedValue(new Error("old daemon"));
    vi.mocked(tauri.getMemoryRevisions).mockRejectedValue(new Error("404"));

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeInTheDocument();
    expect(screen.queryByText(/revision history/i)).toBeNull();
  });

  it("refreshes daemon memory revision history after accepting a pending revision", async () => {
    const user = userEvent.setup();
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
      ...memory,
      memory_type: "identity",
      confirmed: true,
    });
    vi.mocked(tauri.getPendingRevision).mockResolvedValue({
      source_id: "rev-1",
      content: "Proposed protected update",
      source_agent: "claude-code",
    });

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText(/proposed protected update/i)).toBeInTheDocument();
    expect(tauri.getMemoryRevisions).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /accept/i }));

    await waitFor(() => {
      expect(tauri.acceptPendingRevision).toHaveBeenCalledWith("mem-1");
      expect(tauri.getMemoryRevisions).toHaveBeenCalledTimes(2);
    });
  });

  it("invalidates tag inventory after editing tags", async () => {
    const user = userEvent.setup();
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");
    vi.mocked(tauri.setDocumentTags).mockResolvedValue(["reviewed"]);

    render(
      <QueryClientProvider client={queryClient}>
        <MemoryDetail
          sourceId="mem-1"
          onBack={vi.fn()}
          onNavigateEntity={vi.fn()}
          onNavigateMemory={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByText("A memory")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Info" }));
    await user.click(screen.getByTitle("Edit tags"));
    await user.type(screen.getByPlaceholderText("Add a tag..."), "reviewed{enter}");

    await waitFor(() => {
      expect(tauri.setDocumentTags).toHaveBeenCalledWith("memory", "mem-1", ["reviewed"]);
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["tags"] });
    });
  });

  it("uses daemon memory revision history instead of the legacy version panel", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({
      ...memory,
      supersedes: "mem-old",
    });
    vi.mocked(tauri.getVersionChain).mockResolvedValue([
      {
        source_id: "mem-old",
        title: "Legacy version",
        content: "Old content",
        memory_type: "fact",
        confirmed: true,
        supersedes: null,
        last_modified: 5,
      },
    ]);
    vi.mocked(tauri.getMemoryRevisions).mockResolvedValue({
      current_source_id: "mem-1",
      chain_depth: 2,
      entries: [
        {
          source_id: "mem-1",
          depth: 0,
          title: "Current",
          content_preview: "Current version",
          last_modified: 10,
          delta_summary: "Clarified wording",
        },
      ],
    });

    render(
      <MemoryDetail
        sourceId="mem-1"
        onBack={vi.fn()}
        onNavigateEntity={vi.fn()}
        onNavigateMemory={vi.fn()}
      />,
      { wrapper },
    );

    expect(await screen.findByText("A memory")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(await screen.findByText(/revision history/i)).toBeVisible();
    expect(screen.getByText(/clarified wording/i)).toBeInTheDocument();
    await waitFor(() => {
      expect(tauri.getVersionChain).toHaveBeenCalledWith("mem-1");
    });
    expect(screen.queryByText(/version history/i)).toBeNull();
    expect(screen.queryByText(/legacy version/i)).toBeNull();
  });

  it("keeps source text distinct and expandable inside the context panel", async () => {
    const user = userEvent.setup();
    const sourceText = "Original captured source. ".repeat(24);
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({ ...memory, source_text: sourceText });
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });

    const content = await screen.findByText("A memory");
    expect(screen.queryByText("Source", { selector: "summary" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Info" }));
    const panel = screen.getByRole("dialog", { name: "Info" });
    const sourceSummary = within(panel).getByText("Source", { selector: "summary" });
    const sourceDisclosure = sourceSummary.closest("details");
    expect(sourceDisclosure).toHaveAttribute("open");
    expect(content).toBeVisible();
    expect(screen.getByText(/Original captured source/)).toBeVisible();
    const sourceQuote = within(panel).getByText(/Original captured source/);
    expect(sourceQuote.textContent).toHaveLength(121);
    expect(sourceQuote.textContent).toMatch(/…$/);
    expect(screen.getByRole("region", { name: "Memory reading" }).contains(sourceSummary)).toBe(false);
    expect(within(panel).queryByText("A piece of knowledge saved from a conversation, a source, or a capture.")).toBeNull();
    const primaryContext = within(panel).getByRole("region", { name: "Knowledge context" });
    const confirmAction = within(panel).getByRole("button", { name: "Confirm content" });
    if (!sourceDisclosure) throw new Error("Expected source excerpt disclosure");
    expect(confirmAction.compareDocumentPosition(sourceDisclosure) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(sourceDisclosure.compareDocumentPosition(primaryContext) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await user.click(screen.getByRole("button", { name: /show full source/i }));
    expect(screen.getByText(sourceText.trim())).toBeVisible();
    expect(sourceQuote.textContent).toBe(sourceText.trim());
    await user.click(screen.getByRole("button", { name: /show less/i }));
    expect(screen.queryByText(sourceText.trim())).toBeNull();
    expect(sourceQuote.textContent).toHaveLength(121);
  });

  it("omits enrichment when the daemon summary is empty", async () => {
    vi.mocked(tauri.getEnrichmentStatus).mockResolvedValue({ source_id: "mem-1", summary: "  ", steps: [] });
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.queryByText("Enrichment")).toBeNull();
  });

  it("preserves actual Markdown headings without turning prose into a hero", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({ ...memory, content: "# Actual heading\n\nNormal reading paragraph.\n\n## A subsection\n\nMore context." });
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    const editor = await screen.findByRole("textbox", { name: "Memory reading" });
    expect(editor.textContent).toBe("# Actual heading\n\nNormal reading paragraph.\n\n## A subsection\n\nMore context.");
    expect(screen.getByRole("main").querySelector(".memory-detail-hero-text")).toBeNull();
  });

  it("keeps low quality and pending revisions visible without opening information", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({ ...memory, memory_type: "identity", confirmed: true, quality: "low" });
    vi.mocked(tauri.getPendingRevision).mockResolvedValue({ source_id: "rev-1", content: "Protected proposal", source_agent: "codex" });
    vi.mocked(tauri.dismissPendingRevision).mockResolvedValue({ target_source_id: "mem-1", wrote: true });
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("Protected proposal")).toBeVisible();
    expect(screen.getByText("Low quality")).toBeVisible();
    expect(screen.queryByRole("dialog")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(tauri.dismissPendingRevision).toHaveBeenCalledWith("mem-1"));
  });

  it("confirms content from Info and preserves pin and guarded delete in actions", async () => {
    const user = userEvent.setup();
    const onBack = vi.fn();
    vi.mocked(tauri.updateMemory).mockResolvedValue();
    vi.mocked(tauri.pinMemory).mockResolvedValue();
    vi.mocked(tauri.deleteFileChunks).mockResolvedValue();
    const confirmation = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(<MemoryDetail sourceId="mem-1" onBack={onBack} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    const trigger = screen.getByRole("button", { name: "Memory actions" });
    const info = await openMemoryInfo(user);
    await user.click(within(info).getByRole("button", { name: "Confirm content" }));
    await waitFor(() => expect(tauri.updateMemory).toHaveBeenCalledWith("mem-1", undefined, undefined, true));
    await user.click(within(info).getByRole("button", { name: "Close" }));
    await user.click(trigger);
    await user.click(screen.getByRole("menuitemcheckbox", { name: "Pinned No" }));
    await waitFor(() => expect(tauri.pinMemory).toHaveBeenCalledWith("mem-1"));
    await user.click(trigger);
    await user.click(screen.getByRole("menuitem", { name: "Delete memory" }));
    expect(confirmation).toHaveBeenCalledWith("Delete this memory?");
    expect(tauri.deleteFileChunks).not.toHaveBeenCalled();
    confirmation.mockReturnValue(true);
    await user.click(trigger);
    await user.click(screen.getByRole("menuitem", { name: "Delete memory" }));
    await waitFor(() => expect(tauri.deleteFileChunks).toHaveBeenCalledWith("memory", "mem-1"));
    await waitFor(() => expect(onBack).toHaveBeenCalled());
  });

  it("supports keyboard navigation, Escape focus return and outside dismissal for actions", async () => {
    const user = userEvent.setup();
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    const trigger = screen.getByRole("button", { name: "Memory actions" });
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitemcheckbox", { name: "Pinned No" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Delete memory" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(screen.getByRole("menuitemcheckbox", { name: "Pinned No" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(trigger).toHaveFocus();
    expect(screen.queryByRole("menu")).toBeNull();
    await user.click(trigger);
    await user.click(screen.getByText("A memory"));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("preserves reclassification and space assignment inside information", async () => {
    const user = userEvent.setup();
    vi.mocked(tauri.listSpaces).mockResolvedValue([{ id: "work", name: "work", description: null, created_at: 1, updated_at: 1, memory_count: 0, entity_count: 0, suggested: false, starred: false, sort_order: 0 }]);
    vi.mocked(tauri.reclassifyMemory).mockResolvedValue("preference");
    vi.mocked(tauri.updateMemory).mockResolvedValue();
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Info" }));
    await user.click(screen.getByRole("button", { name: /fact/i }));
    await user.click(screen.getByRole("button", { name: "Preference" }));
    await waitFor(() => expect(tauri.reclassifyMemory).toHaveBeenCalledWith("mem-1", "preference"));
    await user.click(screen.getByRole("button", { name: "Assign space" }));
    await user.click(screen.getByRole("button", { name: "work" }));
    await waitFor(() => expect(tauri.updateMemory).toHaveBeenCalledWith("mem-1", undefined, "work", undefined, undefined));
  });


  it("keeps an already-confirmed memory disabled and preserves unpin", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({ ...memory, confirmed: true, pinned: true });
    vi.mocked(tauri.updateMemory).mockResolvedValue();
    vi.mocked(tauri.unpinMemory).mockResolvedValue();
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    const info = await openMemoryInfo();
    const confirmed = within(info).getByRole("button", { name: "Content confirmed" });
    expect(confirmed).toBeDisabled();
    expect(confirmed).toHaveAttribute("data-active", "true");
    expect(within(info).queryByText("You have confirmed this memory.")).toBeNull();
    expect(tauri.updateMemory).not.toHaveBeenCalled();
    await userEvent.click(within(info).getByRole("button", { name: "Close" }));
    await userEvent.click(screen.getByRole("button", { name: "Memory actions" }));
    await userEvent.click(screen.getByRole("menuitemcheckbox", { name: "Pinned Yes" }));
    await waitFor(() => expect(tauri.unpinMemory).toHaveBeenCalledWith("mem-1"));
  });

  it("keeps confirmation pending once, leaves it unconfirmed on error, and allows retry", async () => {
    const pending = deferred<void>();
    vi.mocked(tauri.updateMemory)
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValue();
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();

    const info = await openMemoryInfo();
    const confirm = within(info).getByRole("button", { name: "Confirm content" });
    await userEvent.click(confirm);
    expect(confirm).toBeDisabled();
    expect(confirm).toHaveAttribute("aria-busy", "true");
    await userEvent.click(confirm);
    expect(tauri.updateMemory).toHaveBeenCalledTimes(1);
    expect(tauri.updateMemory).toHaveBeenCalledWith("mem-1", undefined, undefined, true);

    await act(async () => {
      pending.reject(new Error("disk unavailable"));
      await pending.promise.catch(() => undefined);
    });
    expect(await screen.findByText("Couldn’t save this change. Try again.")).toBeVisible();
    expect(confirm).toHaveAttribute("data-active", "false");
    expect(tauri.updateMemory).not.toHaveBeenCalledWith("mem-1", undefined, undefined, false);

    await userEvent.click(confirm);
    await waitFor(() => expect(tauri.updateMemory).toHaveBeenCalledTimes(2));
    expect(tauri.updateMemory).toHaveBeenLastCalledWith("mem-1", undefined, undefined, true);
  });

  it("preserves an unsaved draft and blocks confirmation until it is saved", async () => {
    const user = userEvent.setup();
    vi.mocked(tauri.updateMemory).mockRejectedValueOnce(new Error("offline"));
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();

    const editor = screen.getByRole("textbox", { name: "Memory reading" });
    await user.clear(editor);
    await user.type(editor, "Unsaved draft stays here");
    fireEvent.blur(editor);
    expect(await screen.findByRole("button", { name: "Retry saving" })).toBeVisible();

    const info = await openMemoryInfo(user);
    const confirm = within(info).getByRole("button", { name: "Confirm content" });
    expect(confirm).toBeDisabled();
    expect(confirm).toHaveAttribute("title", "Save your changes before confirming this memory.");
    expect(editor).toHaveTextContent("Unsaved draft stays here");
    expect(tauri.updateMemory).not.toHaveBeenCalledWith("mem-1", undefined, undefined, true);
  });

  it("keeps recaps readable without the retired copy-context action", async () => {
    vi.mocked(tauri.getMemoryDetail).mockResolvedValue({ ...memory, memory_type: "recap", is_recap: true, domain: "work", source_agent: "codex" });
    vi.mocked(tauri.clipboardWrite).mockResolvedValue();
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={vi.fn()} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Edit memory" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Copy as context" })).toBeNull();
    expect(tauri.clipboardWrite).not.toHaveBeenCalled();
  });

  it("keeps recap source memories separate from semantic similarity", async () => {
    const onNavigateMemory = vi.fn();
    vi.mocked(tauri.getMemoryDetail).mockImplementation(async (id) => id === "mem-1" ? {
      ...memory, memory_type: "recap", is_recap: true,
      structured_fields: JSON.stringify({ source_ids: ["recap-source"] }),
    } : { ...memory, source_id: id, content: "Exact recap source" });
    render(<MemoryDetail sourceId="mem-1" onBack={vi.fn()} onNavigateEntity={vi.fn()} onNavigateMemory={onNavigateMemory} />, { wrapper });
    expect(await screen.findByText("A memory")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    await userEvent.click(await screen.findByText("Source memories", { selector: "summary" }));
    expect(screen.queryByText("Similar memories")).toBeNull();
    expect(tauri.search).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: /exact recap source/i }));
    expect(onNavigateMemory).toHaveBeenCalledWith("recap-source");
    expect(screen.getByRole("dialog", { name: "Info" })).toBeVisible();
  });

  it("closes disclosures and source expansion when navigating to another memory", async () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const sourceText = "Original source. ".repeat(30);
    vi.mocked(tauri.getMemoryDetail).mockImplementation(async (id) => ({ ...memory, source_id: id, content: id === "mem-1" ? "First memory" : "Second memory", source_text: sourceText }));
    const props = { onBack: vi.fn(), onNavigateEntity: vi.fn(), onNavigateMemory: vi.fn() };
    const { rerender } = render(<QueryClientProvider client={queryClient}><MemoryDetail sourceId="mem-1" {...props} /></QueryClientProvider>);
    expect(await screen.findByText("First memory")).toBeVisible();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    await userEvent.click(screen.getByText("Source", { selector: "summary" }));
    await userEvent.click(screen.getByRole("button", { name: "Show full source" }));
    rerender(<QueryClientProvider client={queryClient}><MemoryDetail sourceId="mem-2" {...props} /></QueryClientProvider>);
    expect(await screen.findByText("Second memory")).toBeVisible();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByText("Source", { selector: "summary" })).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Info" }));
    expect(screen.getByText("Source", { selector: "summary" }).closest("details")).toHaveAttribute("open");
    expect(screen.getByRole("button", { name: "Show full source" })).toBeVisible();
  });

});
