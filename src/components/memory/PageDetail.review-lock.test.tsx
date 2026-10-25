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
    tauriMocks.setPageReviewRequired.mockImplementation(async (_id: string, required: boolean) => {
      current = required ? { ...current, review_required: true } : { ...PAGE };
      return { page_id: "page-1", review_required: required };
    });
  });

  async function openInfo(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
    await user.click(screen.getByRole("tab", { name: i18n.t("pageInspector.info") }));
    return screen.getByRole("radiogroup", { name: i18n.t("pageDetail.aiChanges") });
  }
  const allowed = () => screen.getByRole("radio", { name: new RegExp(`^${i18n.t("pageDetail.aiAllowed")}`) });
  const askFirst = () => screen.getByRole("radio", { name: new RegExp(`^${i18n.t("pageDetail.aiAskFirst")}`) });

  it("shows Allowed on an AI-written page, with no lock left in the menu", async () => {
    const { user } = renderPage();
    await openInfo(user);
    expect(allowed()).toBeChecked();
    expect(screen.getByText(i18n.t("pageDetail.aiFromAi"))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("pageDetail.actions") }));
    expect(screen.queryByRole("menuitemcheckbox")).toBeNull();
  });

  it("shows Ask me first on a locked page, and says why when the person wrote it", async () => {
    current = { ...PAGE, review_required: true, user_edited: true } as typeof current;
    const { user } = renderPage();
    await openInfo(user);
    expect(askFirst()).toBeChecked();
    expect(screen.getByText(i18n.t("pageDetail.aiFromWrote"))).toBeInTheDocument();
  });

  it("never calls a page written here AI-written, even before anyone edits it", async () => {
    current = { ...PAGE, creation_kind: "authored" } as typeof current;
    const { user } = renderPage();
    await openInfo(user);
    expect(allowed()).toBeChecked();
    expect(screen.queryByText(i18n.t("pageDetail.aiFromAi"))).toBeNull();
    await user.click(askFirst());
    await waitFor(() => expect(askFirst()).toBeChecked());
    expect(screen.getByText(i18n.t("pageDetail.aiFromWrote"))).toBeInTheDocument();
  });

  it("locks the page on Ask me first, then unlocks it on Allowed", async () => {
    const { user } = renderPage();
    await openInfo(user);
    await user.click(askFirst());
    await waitFor(() => expect(tauriMocks.setPageReviewRequired).toHaveBeenLastCalledWith("page-1", true));
    await waitFor(() => expect(askFirst()).toBeChecked());
    expect(screen.getByText(i18n.t("pageDetail.aiFromPage"))).toBeInTheDocument();

    await user.click(allowed());
    await waitFor(() => expect(tauriMocks.setPageReviewRequired).toHaveBeenLastCalledWith("page-1", false));
    await waitFor(() => expect(allowed()).toBeChecked());
  });

  it("offers no setting on an entity shadow page", async () => {
    current = { ...PAGE, creation_kind: "entity" } as typeof current;
    const { user } = renderPage();
    await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.open") }));
    await user.click(screen.getByRole("tab", { name: i18n.t("pageInspector.info") }));
    await screen.findByText(i18n.t("detailInfo.details"));
    expect(screen.queryByRole("radiogroup", { name: i18n.t("pageDetail.aiChanges") })).toBeNull();
  });

  it("keeps the choice enabled on a page opened while another page's request is in flight", async () => {
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
    await user.click(askFirst());
    expect(tauriMocks.setPageReviewRequired).toHaveBeenCalledWith("page-1", true);
    expect(askFirst()).toBeDisabled();

    rerender(view("page-2"));
    await waitFor(() => expect(tauriMocks.getPage).toHaveBeenCalledWith("page-2", "explicit"));
    await waitFor(() => expect(askFirst()).toBeEnabled());
  });

  it("reports a failure and leaves the page on Allowed", async () => {
    tauriMocks.setPageReviewRequired.mockRejectedValue(new Error("offline"));
    const { user } = renderPage();
    await openInfo(user);
    await user.click(askFirst());
    expect(await screen.findByText(i18n.t("pageDetail.aiSaveFailed"))).toBeInTheDocument();
    await waitFor(() => expect(allowed()).toBeChecked());
  });
});
