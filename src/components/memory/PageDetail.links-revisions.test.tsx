// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  replaceDocument,
  pressKey,
} from "./editor/editorTestUtils";

const tauriMocks = vi.hoisted(() => ({
  getPage: vi.fn(),
  getPageSources: vi.fn(),
  listRegisteredSources: vi.fn(),
  getPageLinks: vi.fn(),
  listOrphanLinks: vi.fn(),
  getPageRevisions: vi.fn(),
  listPages: vi.fn(),
  redistillPage: vi.fn(),
  updatePage: vi.fn(),
  getDaemonVersion: vi.fn(),
  getSystemInfo: vi.fn(),
  daemonMeetsFloor: vi.fn(),
  recordPageEditorDiagnostic: vi.fn(),
  deletePage: vi.fn(),
  clipboardWrite: vi.fn(),
  exportPageToObsidian: vi.fn(),
  getTruthStatus: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({entities:[],relations:[],memories:[],memory_links:[],pages:[],page_links:[]}),
  // Fails closed, which is what an older or unreachable daemon looks like:
  // the review action stays disabled unless a test opts in.
  pageReviewSupported: vi.fn().mockResolvedValue("daemon_unsupported"),
  reviewPage: vi.fn(),
  ...tauriMocks,
  FACET_COLORS: {},
  STABILITY_TIERS: {},
}));

const LINKED_PAGE = {
  id: "page-1",
  title: "Link Test Page",
  summary: null,
  content:
    "Intro sentence.\n\nThis page references [[Resolved Link]] and [[Missing Link]].\n\nIt also cites [memory](#memory:mem-1).",
  entity_id: null,
  domain: "testing",
  source_memory_ids: [],
  version: 1,
  status: "active",
  created_at: "2026-06-26T00:00:00+00:00",
  last_compiled: "2026-06-26T00:00:00+00:00",
  last_modified: "2026-06-26T00:00:00+00:00",
};

function renderWithQuery(ui: React.ReactElement, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  return {
    client,
    user: userEvent.setup(),
    ...render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>),
  };
}

beforeAll(() => {
  installCodeMirrorDomPolyfills();
});

async function openPageInfo(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: i18n.t("pageInspector.open") }));
  return screen.getByRole("dialog", { name: i18n.t("pageInspector.label") });
}

describe("PageDetail page links", () => {
  const defaultProps = {
    pageId: "page-1",
    onBack: vi.fn(),
    onMemoryClick: vi.fn(),
    onPageClick: vi.fn(),
  };

  beforeEach(() => {
    vi.clearAllMocks();
    tauriMocks.getPage.mockResolvedValue(LINKED_PAGE);
    tauriMocks.getPageSources.mockResolvedValue([]);
    tauriMocks.listRegisteredSources.mockResolvedValue([]);
    tauriMocks.getPageLinks.mockResolvedValue({ outbound: [], inbound: [] });
    tauriMocks.listOrphanLinks.mockResolvedValue({ min_count: 2, orphan_labels: [] });
    tauriMocks.getPageRevisions.mockResolvedValue({
      page_id: "page-1",
      current_version: 1,
      user_edited: false,
      stale_reason: null,
      entries: [],
    });
    tauriMocks.listPages.mockResolvedValue([]);
    tauriMocks.redistillPage.mockResolvedValue({ status: "ok", updated: true });
    tauriMocks.updatePage.mockImplementation(async (input) => {
      tauriMocks.getPage.mockResolvedValue({ ...LINKED_PAGE, content: input.content, version: input.expectedVersion + 1 });
      return { outcome: "saved" };
    });
    tauriMocks.getDaemonVersion.mockResolvedValue("0.14.1");
    tauriMocks.getSystemInfo.mockResolvedValue({ os: "macos" });
    tauriMocks.daemonMeetsFloor.mockReturnValue(true);
    tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined);
    tauriMocks.deletePage.mockResolvedValue(undefined);
    tauriMocks.clipboardWrite.mockResolvedValue(undefined);
    tauriMocks.exportPageToObsidian.mockResolvedValue({ path: "/tmp/page.md" });
    tauriMocks.getTruthStatus.mockResolvedValue(null);
  });

  it("uses daemon page links for related pages and wikilink navigation", async () => {
    tauriMocks.getPageLinks.mockResolvedValue({
      outbound: [
        { label: "Resolved Link", target_page_id: "page-2" },
        { label: "Missing Link", target_page_id: null },
      ],
      inbound: [{ source_page_id: "page-3", label: "Inbound Mention" }],
    });

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    await waitFor(() => {
      expect(tauriMocks.getPageLinks).toHaveBeenCalledWith("page-1");
    });
    expect(tauriMocks.listPages).not.toHaveBeenCalled();

    const contentLink = await screen.findByRole("link", { name: "Resolved Link" });
    await user.click(contentLink);
    expect(defaultProps.onPageClick).toHaveBeenCalledWith("page-2");

    expect(screen.queryByLabelText("Linked pages")).toBeNull();
    await openPageInfo(user);
    const related = await screen.findByLabelText("Linked pages");
    expect(within(related).getByText("Missing Link")).toBeInTheDocument();
    expect(within(related).queryByRole("button", { name: /Missing Link/ })).toBeNull();
    await user.click(within(related).getByRole("button", { name: /Resolved Link/ }));
    expect(defaultProps.onPageClick).toHaveBeenCalledWith("page-2");
    expect(screen.getByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Inbound Mention" })).toBeInTheDocument();
  });

  it("invalidates page links after saving edited content", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidateSpy = vi.spyOn(client, "invalidateQueries");
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />, client);

    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit page" }));
    const editor = await screen.findByRole("textbox", { name: "Page editor" });
    await waitFor(() => expect(editor).toHaveFocus());
    act(() =>
      replaceDocument(
        editorViewFromTextbox(editor),
        "Intro sentence.\n\nThis page now links [[New Link]].",
      ),
    );
    act(() => pressKey(editor, "s", { ctrlKey: true }));

    await waitFor(() => {
      expect(tauriMocks.updatePage).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "page-1",
          content: "Intro sentence.\n\nThis page now links [[New Link]].",
          expectedVersion: 1,
          callerId: "wenlan-app",
          operationId: expect.any(String),
        }),
      );
    });
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["page-links", "page-1"] });
    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: ["page-revisions", "page-1"] });
  });

  it("renders duplicate inbound labels without raw page ids", async () => {
    tauriMocks.getPageLinks.mockResolvedValue({
      outbound: [],
      inbound: [
        { source_page_id: "source-page-a", label: "Shared Mention" },
        { source_page_id: "source-page-b", label: "Shared Mention" },
      ],
    });

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    await openPageInfo(user);
    expect(screen.getAllByRole("button", { name: "Shared Mention" })).toHaveLength(2);
    expect(screen.queryByText(/source-page-a/)).toBeNull();
  });

  it("shows current titles for ordinary links while preserving explicit aliases", async () => {
    tauriMocks.getPage.mockResolvedValueOnce({
      ...LINKED_PAGE,
      content:
        "Intro sentence.\n\nAlias [[Resolved Link|Alias Text]], heading [[Resolved Link#Section]], unresolved [[Missing Link|Missing Alias]].",
    });
    tauriMocks.getPageLinks.mockResolvedValue({
      outbound: [{ label: "Resolved Link", target_page_id: "resolved-page", target_title: "Renamed [linked] *page*" }],
      inbound: [],
    });

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    const aliasLink = await screen.findByRole("link", { name: "Alias Text" });
    const headingLink = await screen.findByRole("link", { name: "Renamed [linked] *page*" });
    expect(screen.queryByRole("link", { name: "Resolved Link" })).toBeNull();
    expect(screen.getByText(/Missing Alias/)).toBeInTheDocument();
    expect(screen.queryByText("Missing Link|Missing Alias")).toBeNull();

    await user.click(aliasLink);
    await user.click(headingLink);
    expect(defaultProps.onPageClick).toHaveBeenCalledWith("resolved-page");
    expect(defaultProps.onPageClick).toHaveBeenCalledTimes(2);
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
    await openPageInfo(user);
    expect(
      within(screen.getByLabelText(i18n.t("knowledgeContext.linkedPages"))).getByRole("button", {
        name: "Renamed [linked] *page*",
      }),
    ).toBeInTheDocument();
  });

  it("reads a link to a note in another Space as written, naming that Space", async () => {
    tauriMocks.getPage.mockResolvedValueOnce({
      ...LINKED_PAGE,
      content: "Intro sentence.\n\nSee [[Home/Travel plan]] and [[Old name]].",
    });
    tauriMocks.getPageLinks.mockResolvedValue({
      outbound: [
        { label: "Home/Travel plan", target_page_id: "travel-page", target_title: "Travel Plan" },
        { label: "Old name", target_page_id: "renamed-page", target_title: "New name" },
      ],
      inbound: [],
    });

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    await user.click(await screen.findByRole("link", { name: "Home/Travel plan" }));
    expect(defaultProps.onPageClick).toHaveBeenCalledWith("travel-page");
    // A link in the same Space still follows its note's current title.
    expect(screen.getByRole("link", { name: "New name" })).toBeInTheDocument();
  });

  it("keeps the page visible and hides links when the daemon route fails", async () => {
    tauriMocks.getPageLinks.mockRejectedValue(new Error("HTTP GET /api/pages/page-1/links returned 404"));

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    expect(await screen.findByText(/This page references/)).toBeInTheDocument();
    await waitFor(() => {
      expect(tauriMocks.getPageLinks).toHaveBeenCalledWith("page-1");
    });
    expect(tauriMocks.listPages).not.toHaveBeenCalled();
    await openPageInfo(user);
    expect(screen.queryByLabelText("Linked pages")).toBeNull();
    expect(screen.getByRole("dialog", { name: i18n.t("pageInspector.label") })).toBeInTheDocument();
  });

  it("does not query orphan links and renders no Unlinked Mentions section", async () => {
    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    await openPageInfo(user);
    expect(tauriMocks.listOrphanLinks).not.toHaveBeenCalled();
    expect(screen.queryByText("Unlinked Mentions")).toBeNull();
  });

  it("renders daemon page revision history", async () => {
    tauriMocks.getPageRevisions.mockResolvedValue({
      page_id: "page-1",
      current_version: 2,
      user_edited: false,
      stale_reason: null,
      entries: [
        {
          version: 2,
          at: Math.floor(Date.now() / 1000),
          edited_by: "distill",
          delta_summary: "Added backlinks",
          incoming_source_ids: ["mem-1"],
        },
      ],
    });

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);
    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    await openPageInfo(user);
    expect(screen.getByText(/added backlinks/i)).toBeInTheDocument();
    expect(screen.getByText("just now")).toBeInTheDocument();
  });

  it("keeps rendering the page when page revisions route is unavailable", async () => {
    tauriMocks.getPageRevisions.mockRejectedValue(new Error("404"));

    const { user } = renderWithQuery(<PageDetail {...defaultProps} />);

    expect(await screen.findByText("Link Test Page")).toBeInTheDocument();
    await openPageInfo(user);
    const info = screen.getByRole("dialog", { name: i18n.t("pageInspector.label") });
    expect(info).toBeInTheDocument();
    expect(within(info).getByRole("heading", { name: /revision history/i })).toBeInTheDocument();
    expect(within(info).getByRole("alert")).toHaveTextContent(i18n.t("pageInfo.revisionsError"));
    tauriMocks.getPageRevisions.mockResolvedValueOnce({ page_id: "page-1", current_version: 2, user_edited: false, entries: [
      { version: 2, at: Math.floor(Date.now() / 1000), edited_by: "human", delta_summary: "Recovered history" },
    ] });
    await user.click(within(info).getByRole("button", { name: i18n.t("pageInfo.retryRevisions") }));
    expect(await within(info).findByText("Recovered history")).toBeInTheDocument();
    expect(within(info).queryByRole("alert")).toBeNull();
    expect(screen.getByText("Intro sentence.", { exact: false })).toBeInTheDocument();
  });
});
