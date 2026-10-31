// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";

const tauriMocks = vi.hoisted(() => ({
  getPage: vi.fn(),
  getPageSources: vi.fn(),
  listRegisteredSources: vi.fn(),
  getPageLinks: vi.fn(),
  listOrphanLinks: vi.fn(),
  getPageRevisions: vi.fn(),
  listPages: vi.fn(),
  getDaemonVersion: vi.fn(),
  getSystemInfo: vi.fn(),
  daemonMeetsFloor: vi.fn(),
  recordPageEditorDiagnostic: vi.fn(),
  getTruthStatus: vi.fn(),
  pageReviewSupported: vi.fn(),
  setPageReviewRequired: vi.fn(),
  getPageSentences: vi.fn(),
  acceptSentenceProposal: vi.fn(),
  rejectSentenceProposal: vi.fn(),
  redistillPage: vi.fn(),
  updatePage: vi.fn(),
  deletePage: vi.fn(),
  clipboardWrite: vi.fn(),
  exportPageToObsidian: vi.fn(),
  reviewPage: vi.fn(),
  parseEntityGuardError: vi.fn().mockReturnValue(null),
}));

vi.mock("./PageCanvas", () => ({ default: () => <div /> }));

vi.mock("../../lib/tauri", () => ({
  getKnowledgeGraph: vi.fn().mockResolvedValue({ entities: [], relations: [], memories: [], memory_links: [], pages: [], page_links: [] }),
  ...tauriMocks,
  FACET_COLORS: {},
  STABILITY_TIERS: {},
}));

const PAGE = {
  id: "page-1",
  title: "Lock Test Page",
  summary: null,
  content: "Some body text.",
  entity_id: null,
  domain: "testing",
  source_memory_ids: ["mem-1"],
  version: 1,
  status: "active",
  created_at: "2026-06-26T00:00:00+00:00",
  last_compiled: "2026-06-26T00:00:00+00:00",
  last_modified: "2026-06-26T00:00:00+00:00",
};

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    user: userEvent.setup(),
    ...render(
      <QueryClientProvider client={client}>
        <PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} />
      </QueryClientProvider>,
    ),
  };
}

describe("PageDetail review lock", () => {
  let current: typeof PAGE & { review_required?: boolean };
  beforeEach(() => {
    vi.clearAllMocks();
    current = { ...PAGE };
    tauriMocks.getPage.mockImplementation(async () => current);
    tauriMocks.getPageSources.mockResolvedValue([]);
    tauriMocks.listRegisteredSources.mockResolvedValue([]);
    tauriMocks.getPageLinks.mockResolvedValue({ outbound: [], inbound: [] });
    tauriMocks.listOrphanLinks.mockResolvedValue({ min_count: 2, orphan_labels: [] });
    tauriMocks.getPageRevisions.mockResolvedValue({ page_id: "page-1", current_version: 1, user_edited: false, entries: [] });
    tauriMocks.listPages.mockResolvedValue([]);
    tauriMocks.getDaemonVersion.mockResolvedValue("0.14.1");
    tauriMocks.getSystemInfo.mockResolvedValue({ os: "macos" });
    tauriMocks.daemonMeetsFloor.mockReturnValue(true);
    tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined);
    tauriMocks.getTruthStatus.mockResolvedValue(null);
    tauriMocks.pageReviewSupported.mockResolvedValue("ready");
    tauriMocks.getPageSentences.mockResolvedValue({ page_id: "page-1", version: 1, ai_edits: "propose", sentences: [], proposals: [] });
    tauriMocks.setPageReviewRequired.mockImplementation(async (_id: string, required: boolean) => {
      current = required ? { ...current, review_required: true } : { ...PAGE };
      return { page_id: "page-1", review_required: required };
    });
  });

  async function openInfo(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
    await user.click(screen.getByRole("tab", { name: i18n.t("pageInspector.info") }));
    return lock();
  }
  const lock = () => screen.getByRole("button", { name: i18n.t("pageLock.title") });

  it("UX-WHO-2 offers one Lock switch, off by default, with no direct-edit mode and no lock in the menu", async () => {
    const { user } = renderPage();
    await openInfo(user);
    expect(lock()).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText(i18n.t("pageLock.offHint"))).toBeInTheDocument();
    expect(screen.queryByRole("radiogroup")).toBeNull();
    await user.click(screen.getByRole("button", { name: i18n.t("pageDetail.actions") }));
    expect(screen.queryByRole("menuitemcheckbox")).toBeNull();
  });

  it("shows the switch on for a locked page", async () => {
    current = { ...PAGE, review_required: true, user_edited: true } as typeof current;
    const { user } = renderPage();
    await openInfo(user);
    expect(lock()).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(i18n.t("pageLock.onHint"))).toBeInTheDocument();
  });

  it("keeps a page the person wrote unlocked until they lock it", async () => {
    current = { ...PAGE, creation_kind: "authored", user_edited: true } as typeof current;
    const { user } = renderPage();
    await openInfo(user);
    expect(lock()).toHaveAttribute("aria-pressed", "false");
  });

  it("locks the page, then unlocks it", async () => {
    const { user } = renderPage();
    await openInfo(user);
    await user.click(lock());
    await waitFor(() => expect(tauriMocks.setPageReviewRequired).toHaveBeenLastCalledWith("page-1", true));
    await waitFor(() => expect(lock()).toHaveAttribute("aria-pressed", "true"));

    await user.click(lock());
    await waitFor(() => expect(tauriMocks.setPageReviewRequired).toHaveBeenLastCalledWith("page-1", false));
    await waitFor(() => expect(lock()).toHaveAttribute("aria-pressed", "false"));
  });

  it("offers no switch on an entity shadow page", async () => {
    current = { ...PAGE, creation_kind: "entity" } as typeof current;
    const { user } = renderPage();
    await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
    await user.click(screen.getByRole("tab", { name: i18n.t("pageInspector.info") }));
    await screen.findByText(i18n.t("detailInfo.details"));
    expect(screen.queryByRole("button", { name: i18n.t("pageLock.title") })).toBeNull();
  });

  it("keeps the switch usable on a page opened while another page's request is in flight", async () => {
    tauriMocks.getPage.mockImplementation(async (id: string) => ({ ...current, id }));
    tauriMocks.setPageReviewRequired.mockImplementation(() => new Promise(() => {}));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    const view = (pageId: string) => (
      <QueryClientProvider client={client}>
        <PageDetail pageId={pageId} onBack={vi.fn()} onMemoryClick={vi.fn()} />
      </QueryClientProvider>
    );
    const { rerender } = render(view("page-1"));
    await openInfo(user);
    await user.click(lock());
    expect(tauriMocks.setPageReviewRequired).toHaveBeenCalledWith("page-1", true);
    expect(lock()).toHaveAttribute("aria-busy", "true");

    rerender(view("page-2"));
    await waitFor(() => expect(tauriMocks.getPage).toHaveBeenCalledWith("page-2", "explicit"));
    await waitFor(() => expect(lock()).not.toHaveAttribute("aria-busy"));
  });

  it("reports a failure and leaves the page unlocked", async () => {
    tauriMocks.setPageReviewRequired.mockRejectedValue(new Error("offline"));
    const { user } = renderPage();
    await openInfo(user);
    await user.click(lock());
    expect(await screen.findByText(i18n.t("pageDetail.aiSaveFailed"))).toBeInTheDocument();
    await waitFor(() => expect(lock()).toHaveAttribute("aria-pressed", "false"));
  });
});
