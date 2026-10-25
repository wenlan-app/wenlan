// SPDX-License-Identifier: AGPL-3.0-only
import { StrictMode } from "react";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import PageDetail from "./PageDetail";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  pressKey,
  replaceDocument,
  selectRange,
} from "./editor/editorTestUtils";

const tauriMocks = vi.hoisted(() => ({
  getPage: vi.fn(),
  getKnowledgeGraph: vi.fn(),
  getPageSources: vi.fn(),
  listRegisteredSources: vi.fn(),
  getPageLinks: vi.fn(),
  getPageRevisions: vi.fn(),
  getEntityDetail: vi.fn(),
  redistillPage: vi.fn(),
  updatePage: vi.fn(),
  getDaemonVersion: vi.fn(),
  getSystemInfo: vi.fn(),
  recordPageEditorDiagnostic: vi.fn(),
  deletePage: vi.fn(),
  clipboardWrite: vi.fn(),
  exportPageToObsidian: vi.fn(),
}));

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  ...tauriMocks,
}));

const PAGE = {
  id: "page-codemirror-integration",
  title: "Real CodeMirror page",
  summary: null,
  content: "Original exact source.\n",
  entity_id: null,
  domain: "testing",
  source_memory_ids: [],
  version: 7,
  status: "active",
  created_at: "2026-07-20T00:00:00+00:00",
  last_compiled: "2026-07-20T00:00:00+00:00",
  last_modified: "2026-07-20T00:00:00+00:00",
};

beforeAll(() => {
  installCodeMirrorDomPolyfills();
});

beforeEach(() => {
  vi.clearAllMocks();
  tauriMocks.getPage.mockResolvedValue(PAGE);
  tauriMocks.getKnowledgeGraph.mockResolvedValue({entities:[],relations:[],memories:[],memory_links:[],pages:[],page_links:[]});
  tauriMocks.getPageSources.mockResolvedValue([]);
  tauriMocks.listRegisteredSources.mockResolvedValue([]);
  tauriMocks.getPageLinks.mockResolvedValue({ outbound: [], inbound: [] });
  tauriMocks.getPageRevisions.mockResolvedValue({
    page_id: PAGE.id,
    current_version: PAGE.version,
    user_edited: false,
    stale_reason: null,
    entries: [],
  });
  tauriMocks.redistillPage.mockResolvedValue({ status: "ok", updated: true });
  tauriMocks.updatePage.mockImplementation(async (input) => {
    tauriMocks.getPage.mockResolvedValue({ ...PAGE, content: input.content, version: input.expectedVersion + 1 });
    return { outcome: "saved" };
  });
  tauriMocks.getDaemonVersion.mockResolvedValue("0.14.1");
  tauriMocks.getSystemInfo.mockResolvedValue({ os: "macos" });
  tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined);
  tauriMocks.deletePage.mockResolvedValue(undefined);
  tauriMocks.clipboardWrite.mockResolvedValue(undefined);
  tauriMocks.exportPageToObsidian.mockResolvedValue({ path: "/tmp/page.md" });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("PageDetail with the real MarkdownEditor and CodeMirror", () => {
  it("opens an existing page in reading-first mode without focusing or writing", async () => {
    const source = "# Heading\n\n> quoted block\n\n[link](https://example.com)\n";
    tauriMocks.getPage.mockResolvedValue({ ...PAGE, content: source });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          initialMode="edit"
          initialSelection={{ anchor: 0, head: source.length }}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const view = editorViewFromTextbox(textbox);

    expect(view.contentDOM).not.toHaveFocus();
    expect(view.contentDOM.textContent).not.toContain("# Heading");
    expect(view.contentDOM.textContent).not.toContain("> quoted block");
    expect(view.contentDOM.textContent).not.toContain("[link](");
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it.each([
    ["macos", "Cmd", "Ctrl"],
    ["windows", "Ctrl", "Cmd"],
  ])(
    "renders the %s shortcut modifier in the editor description",
    async (os, modifier, otherModifier) => {
      tauriMocks.getSystemInfo.mockResolvedValue({ os });
      const client = new QueryClient({
        defaultOptions: { queries: { retry: false } },
      });
      const user = userEvent.setup();
      render(
        <QueryClientProvider client={client}>
          <PageDetail
            pageId={PAGE.id}
            onBack={vi.fn()}
            onMemoryClick={vi.fn()}
            onPageClick={vi.fn()}
          />
        </QueryClientProvider>,
      );

      expect(await screen.findByText(PAGE.title)).toBeInTheDocument();
      await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));

      const description = document.getElementById(
        "page-markdown-editor-description",
      );
      expect(description).toHaveTextContent(
        `Changes save automatically. Use ${modifier}+S to save now.`,
      );
      expect(description).toHaveClass("sr-only");
      expect(description).not.toHaveTextContent(otherModifier);
      expect(tauriMocks.getSystemInfo).toHaveBeenCalledOnce();
    },
  );

  it("keeps the local macOS Cmd hint when system info is unavailable", async () => {
    vi.spyOn(window.navigator, "platform", "get").mockReturnValue("MacIntel");
    tauriMocks.getSystemInfo.mockRejectedValue(new Error("system info offline"));
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByText(PAGE.title)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));

    expect(
      document.getElementById("page-markdown-editor-description"),
    ).toHaveTextContent("Changes save automatically. Use Cmd+S to save now.");
    expect(tauriMocks.getSystemInfo).toHaveBeenCalledOnce();
  });

  it("uses one editing view and keeps Markdown syntax contextual", async () => {
    const editablePage = {
      ...PAGE,
      content: "# Heading\n\nalpha\n",
    };
    tauriMocks.getPage.mockResolvedValue(editablePage);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByText(PAGE.title)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const view = editorViewFromTextbox(textbox);
    expect(
      screen.queryByRole("radiogroup", { name: "Editing mode" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("radio", { name: "Live Preview" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("radio", { name: "Source mode" }),
    ).not.toBeInTheDocument();
    expect(view.dom).toHaveAttribute("data-editor-presentation-mode", "writing");
    expect(
      document.getElementById("page-markdown-editor-description"),
    ).toHaveTextContent(
      "Changes save automatically. Use Cmd+S to save now.",
    );

    act(() => {
      replaceDocument(view, "# Local heading\n\nalpha\n");
      selectRange(view, view.state.doc.length - 2);
    });
    expect(view.state.doc.toString()).toBe("# Local heading\n\nalpha\n");
    expect(view.dom).toHaveAttribute("data-editor-presentation-mode", "writing");
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("formats through the keyboard and flush sends the exact live CodeMirror snapshot", async () => {
    const editablePage = { ...PAGE, content: "alpha" };
    tauriMocks.getPage.mockResolvedValue(editablePage);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByText(PAGE.title)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const view = editorViewFromTextbox(textbox);
    act(() => selectRange(view, 0, view.state.doc.length));

    act(() => pressKey(textbox, "b", { ctrlKey: true }));
    expect(view.state.doc.toString()).toBe("**alpha**");
    expect(textbox).toHaveFocus();

    act(() => pressKey(textbox, "s", { ctrlKey: true }));

    await waitFor(() => {
      expect(tauriMocks.updatePage).toHaveBeenCalledWith({
        id: PAGE.id,
        content: "**alpha**",
        expectedVersion: PAGE.version,
        callerId: "wenlan-app",
        operationId: expect.any(String),
      });
    });
  });

  it("saves the exact current CodeMirror document through the PageDetail CAS seam", async () => {
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    render(
      <StrictMode>
        <QueryClientProvider client={client}>
          <PageDetail
            pageId={PAGE.id}
            onBack={vi.fn()}
            onMemoryClick={vi.fn()}
            onPageClick={vi.fn()}
          />
        </QueryClientProvider>
      </StrictMode>,
    );

    expect(await screen.findByText(PAGE.title)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const textbox = await screen.findByRole("textbox", {
      name: "Page editor",
    });
    await waitFor(() => {
      expect(
        screen.getByRole("textbox", { name: "Page editor" }),
      ).toHaveFocus();
    });
    expect(
      document.getElementById("page-markdown-editor-description"),
    ).toHaveClass("sr-only");
    const exactSource = "  # Exact CodeMirror source  \n\nFinal line\t \n";
    act(() => {
      replaceDocument(editorViewFromTextbox(textbox), exactSource);
      pressKey(textbox, "s", { ctrlKey: true });
    });

    await waitFor(() => {
      expect(tauriMocks.updatePage).toHaveBeenCalledWith({
        id: PAGE.id,
        content: exactSource,
        expectedVersion: PAGE.version,
        callerId: "wenlan-app",
        operationId: expect.any(String),
      });
    });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Saved"));
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(exactSource);
    act(() => pressKey(textbox, "z", { ctrlKey: true }));
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(PAGE.content);
    act(() => pressKey(textbox, "y", { ctrlKey: true }));
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(exactSource);
    expect(screen.queryByRole("button", { name: /^Save$/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("preserves an upgrade-blocked draft, offers copy, and keeps retry recoverable", async () => {
    tauriMocks.updatePage.mockResolvedValue({
      outcome: "upgrade_required",
      reportedVersion: "0.14.0",
      requiredFloor: "0.14.1",
    });
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByText(PAGE.title)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const textbox = await screen.findByRole("textbox", {
      name: "Page editor",
    });
    const blockedDraft = "# Draft blocked by the old daemon\n";
    act(() => replaceDocument(editorViewFromTextbox(textbox), blockedDraft));
    act(() => pressKey(textbox, "s", { ctrlKey: true }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Page editing requires stable Wenlan daemon 0.14.1 or later. Running version: 0.14.0.",
    );
    expect(screen.getByRole("button", { name: "Copy my draft" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Retry$/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Save$/ })).toBeNull();
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(blockedDraft);

    const editedDraft = `${blockedDraft}\nStill editing locally.\n`;
    act(() => replaceDocument(editorViewFromTextbox(textbox), editedDraft));
    expect(screen.getByRole("alert")).toHaveTextContent("0.14.1");
    await user.click(screen.getByRole("button", { name: "Copy my draft" }));
    expect(tauriMocks.clipboardWrite).toHaveBeenCalledWith(editedDraft);

    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(tauriMocks.updatePage).toHaveBeenCalledTimes(2));
  });

  it("uses the editable H1 as the single title without a persistent formatting toolbar", async () => {
    const editablePage = {
      ...PAGE,
      content: `# ${PAGE.title}\n\nBody copy.\n`,
    };
    tauriMocks.getPage.mockResolvedValue(editablePage);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    const { container } = render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    expect(await screen.findByText(PAGE.title)).toHaveClass("page-detail-title");
    expect(screen.queryByRole("button", { name: "Copy as context" })).toBeNull();
    expect(
      screen.getByRole("button", { name: "Page actions" }),
    ).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const textbox = await screen.findByRole("textbox", {
      name: "Page editor",
    });

    expect(container.querySelector(".page-detail-title")).toBeNull();
    expect(
      screen.getByRole("heading", { level: 1, name: PAGE.title }),
    ).toHaveClass("sr-only");
    expect(
      screen.queryByRole("toolbar", { name: "Formatting" }),
    ).toBeNull();
    expect(
      screen.queryByRole("button", { name: "Copy as context" }),
    ).toBeNull();
    expect(screen.getByRole("button", { name: "Page actions" })).toBeVisible();
    expect(
      document.getElementById("page-markdown-editor-description"),
    ).toHaveClass("sr-only");

    act(() => {
      replaceDocument(
        editorViewFromTextbox(textbox),
        "Body without a matching title.\n",
      );
    });
    expect(container.querySelector(".page-detail-title")).toBeVisible();

    act(() => {
      replaceDocument(editorViewFromTextbox(textbox), editablePage.content);
    });
    expect(container.querySelector(".page-detail-title")).toBeNull();
    expect(
      screen.getByRole("heading", { level: 1, name: PAGE.title }),
    ).toHaveClass("sr-only");

    act(() => pressKey(textbox, "Escape"));
    expect(await screen.findByText(PAGE.title)).toHaveClass("page-detail-title");
    expect(screen.queryByRole("button", { name: "Copy as context" })).toBeNull();
  });

  it("keeps title suppression tied to the active editor baseline during a remote conflict", async () => {
    const editablePage = {
      ...PAGE,
      content: `# ${PAGE.title}\n\nLocal baseline.\n`,
    };
    tauriMocks.getPage.mockResolvedValue(editablePage);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    const { container } = render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    await screen.findByRole("heading", { level: 1, name: PAGE.title });
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const localDraft = `# ${PAGE.title}\n\nUnsaved local draft.\n`;
    act(() => replaceDocument(editorViewFromTextbox(textbox), localDraft));

    act(() => {
      client.setQueryData(["page", PAGE.id], {
        ...editablePage,
        content: "Remote source without a matching heading.\n",
        version: PAGE.version + 1,
      });
    });

    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent(
        "This page changed elsewhere.",
      ),
    );
    expect(container.querySelector(".page-detail-title")).toBeNull();
    expect(
      screen.getByRole("heading", { level: 1, name: PAGE.title }),
    ).toHaveClass("sr-only");
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(localDraft);
  });

  it("keeps the Page title visible while matching-H1 source awaits line-ending normalization", async () => {
    const mixedLineEndingPage = {
      ...PAGE,
      content: `# ${PAGE.title}\r\n\r\nMixed source.\n`,
    };
    tauriMocks.getPage.mockResolvedValue(mixedLineEndingPage);
    const client = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    const user = userEvent.setup();
    const { container } = render(
      <QueryClientProvider client={client}>
        <PageDetail
          pageId={PAGE.id}
          onBack={vi.fn()}
          onMemoryClick={vi.fn()}
          onPageClick={vi.fn()}
        />
      </QueryClientProvider>,
    );

    await screen.findByRole("button", { name: "Page actions" });
    expect(container.querySelector(".page-detail-title")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Line-ending normalization required",
    );
    expect(container.querySelector(".page-detail-title")).toBeVisible();
    expect(
      screen.getByRole("heading", { level: 1, name: PAGE.title }),
    ).not.toHaveClass("sr-only");
    expect(
      screen.queryByRole("textbox", { name: "Page editor" }),
    ).toBeNull();
  });

  it("autosaves without persistence buttons and keeps undo history across confirmed versions", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const registerFlush = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onRegisterFlush={registerFlush} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
    expect(screen.queryByRole("toolbar", { name: "Formatting" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
    const view = editorViewFromTextbox(textbox);
    // Control the 650 ms debounce so CI wall-clock load cannot decide its boundary.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      act(() => replaceDocument(view, "A new autosaved body.\n"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(649);
      });
      expect(tauriMocks.updatePage).not.toHaveBeenCalled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(tauriMocks.updatePage).toHaveBeenNthCalledWith(
        1,
        expect.objectContaining({
          id: PAGE.id,
          content: "A new autosaved body.\n",
          expectedVersion: PAGE.version,
        }),
      );
      expect(screen.getByRole("status")).toHaveTextContent("Saved");
      expect(client.getQueryData<typeof PAGE>(["page", PAGE.id])).toEqual(
        expect.objectContaining({
          content: "A new autosaved body.\n",
          version: PAGE.version + 1,
        }),
      );
      expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
      const flush = registerFlush.mock.lastCall?.[0] as () => Promise<boolean>;
      await act(async () => {
        expect(await flush()).toBe(true);
      });
      expect(tauriMocks.updatePage).toHaveBeenCalledTimes(1);

      view.focus();
      act(() => pressKey(textbox, "z", { ctrlKey: true }));
      expect(view.state.doc.toString()).toBe(PAGE.content);
      expect(screen.getByRole("status")).toHaveTextContent("Unsaved changes");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(649);
      });
      expect(tauriMocks.updatePage).toHaveBeenCalledTimes(1);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(tauriMocks.updatePage).toHaveBeenNthCalledWith(
        2,
        expect.objectContaining({
          id: PAGE.id,
          content: PAGE.content,
          expectedVersion: PAGE.version + 1,
        }),
      );
      expect(screen.getByRole("status")).toHaveTextContent("Saved");
      expect(client.getQueryData<typeof PAGE>(["page", PAGE.id])).toEqual(
        expect.objectContaining({
          content: PAGE.content,
          version: PAGE.version + 2,
        }),
      );
      expect(tauriMocks.updatePage).toHaveBeenCalledTimes(2);
      expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
    } finally {
      vi.useRealTimers();
    }
  });

  it("registered flush drains newer input through the confirmed canonical version", async () => {
    let finishWrite!: (value: { outcome: "saved" }) => void;
    const writing = new Promise<{ outcome: "saved" }>((resolve) => { finishWrite = resolve; });
    tauriMocks.updatePage.mockReturnValueOnce(writing);
    const register = vi.fn();
    const dirty = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onRegisterFlush={register} onEditDirtyChange={dirty} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const view = editorViewFromTextbox(textbox);
    act(() => replaceDocument(view, "First snapshot."));
    tauriMocks.getPage.mockResolvedValueOnce({ ...PAGE, content: "First snapshot.", version: PAGE.version + 1 });
    const flush = register.mock.lastCall?.[0] as () => Promise<boolean>;
    let flushed!: Promise<boolean>;
    act(() => { flushed = flush(); });
    expect(tauriMocks.updatePage).toHaveBeenCalledTimes(1);
    expect(textbox).toHaveAttribute("contenteditable", "true");
    act(() => replaceDocument(view, "Second snapshot while saving."));
    expect(dirty).toHaveBeenLastCalledWith(true);
    const pendingUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(pendingUnload);
    expect(pendingUnload.defaultPrevented).toBe(true);

    await act(async () => {
      finishWrite({ outcome: "saved" });
      expect(await flushed).toBe(true);
    });
    expect(tauriMocks.updatePage).toHaveBeenCalledTimes(2);
    expect(tauriMocks.updatePage.mock.calls[1][0]).toEqual(expect.objectContaining({
      content: "Second snapshot while saving.", expectedVersion: PAGE.version + 1,
    }));
    expect(dirty).toHaveBeenLastCalledWith(false);
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
    expect(view.state.doc.toString()).toBe("Second snapshot while saving.");
    const confirmedUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(confirmedUnload);
    expect(confirmedUnload.defaultPrevented).toBe(false);
  });

  it("Back waits for canonical confirmation and leaves the editor intact until then", async () => {
    let finishRead!: (page: typeof PAGE) => void;
    const canonical = new Promise<typeof PAGE>((resolve) => { finishRead = resolve; });
    const onBack = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={onBack} onMemoryClick={vi.fn()} onPageClick={vi.fn()} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const draft = "Leave only after canonical confirmation.";
    act(() => replaceDocument(editorViewFromTextbox(textbox), draft));
    tauriMocks.getPage.mockReturnValueOnce(canonical);
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(tauriMocks.updatePage).toHaveBeenCalledTimes(1);
    expect(onBack).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
    await act(async () => finishRead({ ...PAGE, content: draft, version: PAGE.version + 1 }));
    await waitFor(() => expect(onBack).toHaveBeenCalledOnce());
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
  });


  it.each([false, true])("preserves the live editor when the canonical page is deleted (local edit: %s)", async (changed) => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    const navigate = vi.fn();
    const dirty = vi.fn();
    const register = vi.fn();
    const onBack = vi.fn(async () => {
      if (await register.mock.lastCall?.[0]()) navigate();
    });
    const confirmDiscard = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={onBack} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onEditDirtyChange={dirty} onRegisterFlush={register} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const draft = changed ? "Keep this locally after deletion." : PAGE.content;
    if (changed) act(() => replaceDocument(editorViewFromTextbox(textbox), draft));
    tauriMocks.getPage.mockResolvedValue(null);
    await act(async () => { await client.invalidateQueries({ queryKey: ["page", PAGE.id] }); });

    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(draft);
    expect(await screen.findByRole("alert")).toHaveTextContent("This page no longer exists. Copy your draft before closing.");
    expect(screen.getByRole("status")).toHaveTextContent("Unsaved changes");
    expect(dirty).toHaveBeenLastCalledWith(true);
    await user.click(screen.getByRole("button", { name: "Copy my draft" }));
    expect(tauriMocks.clipboardWrite).toHaveBeenCalledWith(draft);
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(navigate).not.toHaveBeenCalled();
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
    const closing = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(closing);
    expect(closing.defaultPrevented).toBe(true);
    const flush = register.mock.lastCall?.[0] as () => Promise<boolean>;
    expect(await flush()).toBe(false);
    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(confirmDiscard).toHaveBeenCalledWith("Discard your unsaved draft?");
    expect(screen.getByRole("textbox", { name: "Page editor" })).toBe(textbox);
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(draft);
    expect(await flush()).toBe(false);
    const cancelledUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(cancelledUnload);
    expect(cancelledUnload.defaultPrevented).toBe(true);

    confirmDiscard.mockReturnValue(true);
    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(screen.queryByRole("textbox", { name: "Page editor" })).toBeNull();
    expect(dirty).toHaveBeenLastCalledWith(false);
    expect(await flush()).toBe(true);
    const discardedUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(discardedUnload);
    expect(discardedUnload.defaultPrevented).toBe(false);
    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(navigate).toHaveBeenCalledOnce();
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("confirms deletion after a typed not-found save without a query refresh and guards discard", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    const dirty = vi.fn();
    const register = vi.fn();
    const confirmDiscard = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onEditDirtyChange={dirty} onRegisterFlush={register} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    tauriMocks.updatePage.mockResolvedValueOnce({ outcome: "failure", kind: "not_found", status: 404, message: "Missing" });
    tauriMocks.getPage.mockResolvedValue(null);
    const draft = "Draft survives a remote deletion discovered during save.";
    act(() => replaceDocument(editorViewFromTextbox(textbox), draft));
    const flush = register.mock.lastCall?.[0] as () => Promise<boolean>;
    await act(async () => { expect(await flush()).toBe(false); });
    expect(client.getQueryData(["page", PAGE.id])).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent("This page no longer exists. Copy your draft before closing.");
    await user.click(screen.getByRole("button", { name: "Copy my draft" }));
    expect(tauriMocks.clipboardWrite).toHaveBeenCalledWith(draft);
    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(confirmDiscard).toHaveBeenCalledWith("Discard your unsaved draft?");
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(draft);
    expect(dirty).toHaveBeenLastCalledWith(true);
    expect(await flush()).toBe(false);
    const cancelledUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(cancelledUnload);
    expect(cancelledUnload.defaultPrevented).toBe(true);
    confirmDiscard.mockReturnValue(true);
    await user.click(screen.getByRole("button", { name: "Discard draft" }));
    expect(screen.queryByRole("textbox", { name: "Page editor" })).toBeNull();
    expect(dirty).toHaveBeenLastCalledWith(false);
    expect(await flush()).toBe(true);
    const discardedUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(discardedUnload);
    expect(discardedUnload.defaultPrevented).toBe(false);
    expect(tauriMocks.updatePage).toHaveBeenCalledOnce();
  });

  it("keeps a not-found draft protected without offering discard when confirmation cannot load", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const register = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onRegisterFlush={register} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    tauriMocks.updatePage.mockResolvedValueOnce({ outcome: "failure", kind: "not_found", status: 404, message: "Missing" });
    tauriMocks.getPage.mockRejectedValueOnce(new Error("offline"));
    const draft = "Unconfirmed deletion still protects my draft.";
    act(() => replaceDocument(editorViewFromTextbox(textbox), draft));
    await act(async () => { expect(await register.mock.lastCall?.[0]()).toBe(false); });
    expect(client.getQueryData(["page", PAGE.id])).toEqual(PAGE);
    expect(screen.queryByRole("button", { name: "Discard draft" })).toBeNull();
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(draft);
    const unload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unload);
    expect(unload.defaultPrevented).toBe(true);
    expect(tauriMocks.updatePage).toHaveBeenCalledOnce();
  });

  it("keeps the newest observed conflict preview and reloads it only after confirmation", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    const confirmReload = vi.spyOn(window, "confirm").mockReturnValue(false);
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    const draft = "Keep my local document until confirmation.";
    act(() => replaceDocument(editorViewFromTextbox(textbox), draft));
    const remote4 = { ...PAGE, content: "Remote first conflict.", version: PAGE.version + 1 };
    const remote5 = { ...PAGE, content: "Remote newest conflict.", version: PAGE.version + 2 };
    await act(async () => { client.setQueryData(["page", PAGE.id], remote4); });
    await screen.findByText(`Latest source (version ${remote4.version})`);
    await act(async () => { client.setQueryData(["page", PAGE.id], remote5); });
    await screen.findByText(`Latest source (version ${remote5.version})`);
    expect(screen.queryByRole("button", { name: "Discard draft" })).toBeNull();
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(draft);
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe(draft);
    expect(client.getQueryData(["page", PAGE.id])).toEqual(remote5);
    confirmReload.mockReturnValue(true);
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    await waitFor(() => expect(editorViewFromTextbox(screen.getByRole("textbox", { name: "Page editor" })).state.doc.toString()).toBe(remote5.content));
    expect(client.getQueryData(["page", PAGE.id])).toEqual(remote5);
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("shows an edit made in Obsidian when nothing was typed here, with no conflict", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} />
      </QueryClientProvider>,
    );
    await screen.findByRole("textbox", { name: "Page editor" });
    const edited = { ...PAGE, content: "Edited in Obsidian.\n", version: PAGE.version + 1 };
    await act(async () => { client.setQueryData(["page", PAGE.id], edited); });
    await waitFor(() => expect(editorViewFromTextbox(screen.getByRole("textbox", { name: "Page editor" })).state.doc.toString()).toBe(edited.content));
    expect(screen.queryByText("This page changed elsewhere.")).toBeNull();
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("Reload latest asks again and loads the newest page, not the first one it saw", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    act(() => replaceDocument(editorViewFromTextbox(textbox), "My local draft."));
    const remote = { ...PAGE, content: "Edit three.", version: PAGE.version + 1 };
    const newest = { ...PAGE, content: "Edit five.", version: PAGE.version + 3 };
    await act(async () => { client.setQueryData(["page", PAGE.id], remote); });
    await screen.findByText(`Latest source (version ${remote.version})`);
    tauriMocks.getPage.mockResolvedValue(newest);
    await user.click(screen.getByRole("button", { name: "Reload latest" }));
    await waitFor(() => expect(editorViewFromTextbox(screen.getByRole("textbox", { name: "Page editor" })).state.doc.toString()).toBe(newest.content));
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("retains a successful Retry latest preview after additional typing", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    const register = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onRegisterFlush={register} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    tauriMocks.updatePage.mockResolvedValueOnce({ outcome: "conflict", message: "Remote edit" });
    tauriMocks.getPage.mockRejectedValueOnce(new Error("Latest temporarily unavailable"));
    act(() => replaceDocument(editorViewFromTextbox(textbox), "Local document."));
    await act(async () => { expect(await register.mock.lastCall?.[0]()).toBe(false); });
    await screen.findByRole("button", { name: "Retry loading latest" });
    expect(screen.queryByRole("button", { name: "Discard draft" })).toBeNull();
    const latest = { ...PAGE, content: "Recovered remote source.", version: PAGE.version + 1 };
    tauriMocks.getPage.mockResolvedValue(latest);
    await user.click(screen.getByRole("button", { name: "Retry loading latest" }));
    await screen.findByText(`Latest source (version ${latest.version})`);
    act(() => replaceDocument(editorViewFromTextbox(textbox), "Local document with more typing."));
    expect(screen.getByText(`Latest source (version ${latest.version})`)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry loading latest" })).toBeNull();
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe("Local document with more typing.");
    expect(tauriMocks.updatePage).toHaveBeenCalledTimes(1);
  });

  it("offers no discard escape during a pending or ambiguous write", async () => {
    let rejectWrite!: (error: Error) => void;
    tauriMocks.updatePage.mockReturnValueOnce(new Promise((_resolve, reject) => { rejectWrite = reject; }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const register = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onRegisterFlush={register} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    act(() => replaceDocument(editorViewFromTextbox(textbox), "Possibly committed local draft."));
    let flushing!: Promise<boolean>;
    act(() => { flushing = register.mock.lastCall?.[0](); });
    expect(screen.queryByRole("button", { name: "Discard draft" })).toBeNull();
    await act(async () => {
      rejectWrite(new Error("Response lost"));
      expect(await flushing).toBe(false);
    });
    expect(screen.queryByRole("button", { name: "Discard draft" })).toBeNull();
    expect(await register.mock.lastCall?.[0]()).toBe(false);
    const unresolvedUnload = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(unresolvedUnload);
    expect(unresolvedUnload.defaultPrevented).toBe(true);
    expect(editorViewFromTextbox(textbox).state.doc.toString()).toBe("Possibly committed local draft.");
  });

  it("queues Back immediately with the navigation owner when a flush handle is registered", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onBack = vi.fn();
    const register = vi.fn();
    render(
      <QueryClientProvider client={client}>
        <PageDetail pageId={PAGE.id} initialMode="edit" onBack={onBack} onMemoryClick={vi.fn()} onPageClick={vi.fn()} onRegisterFlush={register} />
      </QueryClientProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Page editor" });
    // Hold the 650 ms autosave debounce so a slow CI click cannot let it write first.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      act(() => replaceDocument(editorViewFromTextbox(textbox), "Main owns this flush."));
      act(() => { fireEvent.click(screen.getByRole("button", { name: "Back" })); });
      expect(onBack).toHaveBeenCalledOnce();
      expect(tauriMocks.updatePage).not.toHaveBeenCalled();
      const flush = register.mock.lastCall?.[0] as () => Promise<boolean>;
      await act(async () => expect(await flush()).toBe(true));
      expect(tauriMocks.updatePage).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

});


it("refreshes an open context graph after a page autosave persists new links", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} /></QueryClientProvider>);
  const textbox = await screen.findByRole("textbox", { name: "Page editor" });
  await userEvent.click(screen.getByRole("button", { name: "Open page sidebar" }));
  await userEvent.click(screen.getByRole("tab", { name: "Info" }));
  await screen.findByText("No direct connections in the currently visible knowledge.");
  const priorSave = tauriMocks.updatePage.getMockImplementation()!;
  tauriMocks.updatePage.mockImplementationOnce(async (input) => {
    const result = await priorSave(input);
    tauriMocks.getKnowledgeGraph.mockResolvedValue({entities:[],relations:[],memories:[],memory_links:[],pages:[{id:"linked-note", title:"New linked page", space:null, creation_kind:"source", entity_id:null,last_modified:""}],page_links:[{from:{kind:"page",id:PAGE.id},to:{kind:"page",id:"linked-note"},link_type:"wikilink"}]});
    return result;
  });
  act(() => replaceDocument(editorViewFromTextbox(textbox), "Body with [[New linked page]].\n"));
  expect(await screen.findByRole("button", { name: "Open page: New linked page" })).toBeVisible();
  expect(screen.queryByText("No direct connections in the currently visible knowledge.")).toBeNull();
});
