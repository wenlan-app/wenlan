// SPDX-License-Identifier: AGPL-3.0-only
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  replaceDocument,
} from "./editor/editorTestUtils";
import type { BasisRef, Page } from "../../lib/tauri";

vi.mock("./PageCanvas", () => ({
  default: ({ pageTitle }: { pageTitle: string }) => <section aria-label={`Canvas for ${pageTitle}`} />,
}));

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({ entities: [], relations: [], memories: [], memory_links: [], pages: [], page_links: [] }),
  pageReviewSupported: vi.fn().mockResolvedValue("daemon_unsupported"),
  reviewPage: vi.fn(),
  agentDisplayName: (slug: string | null) => slug,
  getPage: vi.fn(),
  getPageSources: vi.fn().mockResolvedValue([]),
  clipboardWrite: vi.fn().mockResolvedValue(undefined),
  exportPagesToObsidian: vi.fn(),
  exportPageToObsidian: vi.fn(),
  listRegisteredSources: vi.fn().mockResolvedValue([]),
  getPageLinks: vi.fn().mockResolvedValue({ outbound: [], inbound: [] }),
  getPageRevisions: vi.fn().mockResolvedValue({ page_id: "note_a", current_version: 3, user_edited: false, stale_reason: null, entries: [] }),
  listOrphanLinks: vi.fn().mockResolvedValue({ min_count: 2, orphan_labels: [] }),
  listPages: vi.fn().mockResolvedValue([]),
  listPagesExplicitBrowse: vi.fn().mockResolvedValue([]),
  searchPages: vi.fn().mockResolvedValue([]),
  redistillPage: vi.fn(),
  updatePage: vi.fn(),
  renamePage: vi.fn(),
  getDaemonVersion: vi.fn().mockResolvedValue("0.14.1"),
  getSystemInfo: vi.fn().mockResolvedValue({ os: "macos" }),
  daemonMeetsFloor: vi.fn().mockReturnValue(true),
  recordPageEditorDiagnostic: vi.fn().mockResolvedValue(undefined),
  deletePage: vi.fn(),
  parseEntityGuardError: vi.fn().mockReturnValue(null),
  FACET_COLORS: {},
  STABILITY_TIERS: {},
  getPendingRevision: vi.fn().mockResolvedValue(null),
  acceptPendingRevision: vi.fn(),
  dismissPendingRevision: vi.fn(),
  getTruthStatus: vi.fn().mockResolvedValue(null),
  listPendingRevisions: vi.fn().mockResolvedValue([]),
  listRefinements: vi.fn().mockResolvedValue({ proposals: [] }),
  listBasisPending: vi.fn().mockResolvedValue([]),
  getPageBasisDiff: vi.fn(),
  confirmPageBasis: vi.fn(),
  markPageBasis: vi.fn(),
  unmarkPageBasis: vi.fn(),
}));

import { getPage, getPageSources, listPagesExplicitBrowse, markPageBasis, redistillPage, updatePage } from "../../lib/tauri";
import { EditorSelection } from "@codemirror/state";

const oneSource = [{
  source: { page_id: "note_a", memory_source_id: "mem_1", linked_at: 1712000000, link_reason: "page_growth" },
  memory: {
    source_id: "mem_1", title: "Plan price", content: "The plan costs 10 dollars.", summary: null,
    memory_type: "fact", domain: null, source_agent: "claude", confidence: 0.9, confirmed: true, last_modified: 1712000000,
  },
}];

const pendingBasis: BasisRef = { page_id: "basis_a", title: "Pricing", state: "pending", from_version: 1, to_version: 3 };

const notePage = (over: Partial<Page> = {}): Page => ({
  id: "note_a",
  title: "Pricing summary",
  summary: null,
  content: "The plan costs 10 dollars.",
  entity_id: null,
  domain: null,
  source_memory_ids: ["mem_1"],
  version: 3,
  status: "active",
  created_at: "2026-04-01T00:00:00+00:00",
  last_compiled: "2026-04-07T12:00:00+00:00",
  last_modified: "2026-04-07T12:00:00+00:00",
  ...over,
});

function renderPage(over: Partial<Page> = {}, props: Partial<React.ComponentProps<typeof PageDetail>> = {}) {
  vi.mocked(getPage).mockResolvedValue(notePage(over));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenReview = vi.fn();
  const view = render(<QueryClientProvider client={client}>
    <PageDetail
      pageId="note_a"
      onBack={vi.fn()}
      onMemoryClick={vi.fn()}
      onPageClick={vi.fn()}
      onOpenReview={onOpenReview}
      {...props}
    />
  </QueryClientProvider>);
  return { ...view, client, onOpenReview, user: userEvent.setup() };
}

beforeAll(() => installCodeMirrorDomPolyfills());

beforeEach(() => {
  vi.mocked(updatePage).mockReset().mockResolvedValue({ outcome: "saved" } as never);
  vi.mocked(getPageSources).mockReset().mockResolvedValue(oneSource as never);
  vi.mocked(redistillPage).mockReset().mockResolvedValue({ status: "ok", updated: true } as never);
});

async function typeAtEnd(text: string) {
  const view = editorViewFromTextbox(await screen.findByRole("textbox", { name: "Page editor" }));
  act(() => {
    for (const char of text) {
      const from = view.state.doc.length;
      view.dispatch({ changes: { from, insert: char }, selection: EditorSelection.cursor(from + 1), userEvent: "input.type" });
    }
  });
  return view;
}

const linkCandidates = [
  notePage({ id: "note_a", title: "Pricing summary" }),
  notePage({ id: "basis_b", title: "Price list" }),
  notePage({ id: "entity_c", title: "Price Corp", creation_kind: "entity" } as Partial<Page>),
  notePage({ id: "elsewhere_d", title: "Price elsewhere", space: "other" } as Partial<Page>),
];

async function typeIntoEditor(text: string) {
  const editor = await screen.findByRole("textbox", { name: "Page editor" });
  act(() => replaceDocument(editorViewFromTextbox(editor), text));
}

describe("PageDetail with note bases", () => {
  it("saves a human edit with no minor-edit choice", async () => {
    renderPage({}, { initialMode: "edit" });

    await screen.findByRole("textbox", { name: "Page editor" });
    expect(screen.queryByRole("checkbox")).toBeNull();
    await typeIntoEditor("The plan costs 11 dollars.");

    await waitFor(() => expect(updatePage).toHaveBeenCalled(), { timeout: 4000 });
    expect(vi.mocked(updatePage).mock.calls[0][0]).not.toHaveProperty("minor");
  });

  it("does not carry a basis confirmation over to another note", async () => {
    const { user, rerender, client } = renderPage({ basis: [pendingBasis] });
    await user.click(await screen.findByRole("button", { name: "More actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit it yourself" }));

    vi.mocked(getPage).mockImplementation(async (id: string) => notePage({
      id, title: "Other note", basis: [],
    }));
    rerender(<QueryClientProvider client={client}>
      <PageDetail
        pageId="note_b"
        onBack={vi.fn()}
        onMemoryClick={vi.fn()}
        onPageClick={vi.fn()}
        initialMode="edit"
      />
    </QueryClientProvider>);

    await typeIntoEditor("Something else entirely.");
    await waitFor(() => expect(updatePage).toHaveBeenCalled(), { timeout: 4000 });
    const calls = vi.mocked(updatePage).mock.calls;
    const sent = calls[calls.length - 1][0];
    expect(sent).toMatchObject({ id: "note_b" });
    expect(sent).not.toHaveProperty("minor");
    expect(sent).not.toHaveProperty("confirmBasis");
  });

  it("carries the basis confirmation on the next save after 'Edit it yourself'", async () => {
    const { user } = renderPage({ basis: [pendingBasis] });

    await user.click(await screen.findByRole("button", { name: "More actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit it yourself" }));
    await typeIntoEditor("The plan costs 12 dollars.");

    await waitFor(() => expect(updatePage).toHaveBeenCalled(), { timeout: 4000 });
    expect(vi.mocked(updatePage).mock.calls[0][0]).toMatchObject({
      content: "The plan costs 12 dollars.",
      confirmBasis: { basis_page_id: "basis_a", version: 3 },
    });
  });

  it("says the source changed again when the save could not confirm it", async () => {
    vi.mocked(updatePage).mockResolvedValue({ outcome: "saved", basisConfirm: "moved_on" } as never);
    const { user } = renderPage({ basis: [pendingBasis] });

    await user.click(await screen.findByRole("button", { name: "More actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Edit it yourself" }));
    await typeIntoEditor("The plan costs 12 dollars.");

    expect(await screen.findByText(/changed again/, undefined, { timeout: 4000 })).toBeInTheDocument();
  });

  it("opens the staged preview when updating from sources was held for review", async () => {
    vi.mocked(redistillPage).mockResolvedValue({ status: "ok", updated: false, gated: true, revision_card_id: "card_9" } as never);
    const { user, onOpenReview } = renderPage();

    await screen.findByText("Pricing summary");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));

    expect(await screen.findByText("Preview ready to review")).toBeInTheDocument();
    expect(onOpenReview).toHaveBeenCalledWith("revision:note_a");
    expect(screen.queryByText("Page already up to date.")).toBeNull();
  });

  it("says the note is already up to date when the rebuild changed nothing", async () => {
    vi.mocked(redistillPage).mockResolvedValue({ status: "ok", updated: false, unchanged: true } as never);
    const { user, onOpenReview } = renderPage();

    await screen.findByText("Pricing summary");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    await user.click(screen.getByRole("menuitem", { name: "Update from sources" }));

    expect(await screen.findByText("Page already up to date.")).toBeInTheDocument();
    expect(onOpenReview).not.toHaveBeenCalled();
  });

  it("allows updating from sources when the note has no memories but still has a live source note", async () => {
    vi.mocked(getPageSources).mockResolvedValue([]);
    const { user } = renderPage({ source_memory_ids: [], basis: [{ ...pendingBasis, state: "ok" }] });

    await screen.findByText("Pricing summary");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.getByRole("menuitem", { name: "Update from sources" })).toBeEnabled();
  });

  it("hides updating from sources when every source note is gone and there are no memories", async () => {
    vi.mocked(getPageSources).mockResolvedValue([]);
    const { user } = renderPage({
      source_memory_ids: [],
      basis: [{ ...pendingBasis, state: "deleted" }, { ...pendingBasis, page_id: "basis_b", state: "unavailable" }],
    });

    await screen.findByText("Pricing summary");
    await user.click(screen.getByRole("button", { name: "Page actions" }));
    expect(screen.getByRole("menuitem", { name: "Edit page" })).toBeVisible();
    expect(screen.queryByRole("menuitem", { name: "Update from sources" })).toBeNull();
  });

  it("reads has_basis as a plain source update in the dateline, and keeps other blocks as blocked", async () => {
    const { unmount } = renderPage({ stale_reason: "source_updated", refresh_blocked_reason: "has_basis" });
    await userEvent.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
    await screen.findByRole("dialog", { name: i18n.t("pageInspector.label") });
    expect(screen.queryByText("update blocked: citations couldn't be verified")).toBeNull();
    expect(screen.queryByText("updating...")).toBeNull();
    unmount();

    renderPage({ stale_reason: "source_updated", refresh_blocked_reason: "citation_gate" });
    await userEvent.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
    expect(await screen.findByText("update blocked: citations couldn't be verified")).toBeInTheDocument();
  });

  it("links a same-Space note from [[ and can use it as a source in the same step", async () => {
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue(linkCandidates);
    vi.mocked(markPageBasis).mockReset().mockResolvedValue({ page_id: "basis_b", title: "Price list", state: "current" } as never);
    renderPage({}, { initialMode: "edit" });
    const view = await typeAtEnd(" [[pri");

    const menu = await screen.findByRole("listbox", { name: "Link a page" });
    expect(listPagesExplicitBrowse).toHaveBeenCalledWith("active", "uncategorized", 500, 0);
    const rows = await waitFor(() => {
      const options = menu.querySelectorAll('[role="option"]');
      expect(options.length).toBeGreaterThan(0);
      return [...options];
    });
    // An entity never resolves as a [[link]], so it is not offered.
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual(["Price list"]);

    act(() => { rows[0].querySelector("button")!.click(); });
    expect(view.state.doc.toString()).toBe("The plan costs 10 dollars. [[Price list]]");
    await waitFor(() => expect(markPageBasis).toHaveBeenCalledWith("note_a", "basis_b"));
    await waitFor(() => expect(view.dom.querySelector(".cm-wiki-page-link")?.textContent).toBe("Price list"));
    await waitFor(() => expect(updatePage).toHaveBeenCalled(), { timeout: 4000 });
  });

  it("says so when using a linked note as a source fails, and keeps the link", async () => {
    vi.mocked(listPagesExplicitBrowse).mockResolvedValue(linkCandidates);
    vi.mocked(markPageBasis).mockReset().mockRejectedValue(new Error("boom"));
    renderPage({}, { initialMode: "edit" });
    const view = await typeAtEnd(" [[price l");
    const menu = await screen.findByRole("listbox", { name: "Link a page" });
    const action = await waitFor(() => {
      const button = menu.querySelector("button");
      expect(button).not.toBeNull();
      return button!;
    });
    act(() => { action.click(); });
    expect(await screen.findByText(i18n.t("noteBasis.markFailed"))).toBeInTheDocument();
    expect(view.state.doc.toString()).toBe("The plan costs 10 dollars. [[Price list]]");
  });
});
