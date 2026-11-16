// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import PageDetail from "./PageDetail";
import { i18n } from "../../i18n";
import { TeamAccessProvider, teamAccessFor, type TeamAccess } from "../../web/teamAccess";

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

vi.mock("../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../lib/tauri")>()),
  getKnowledgeGraph: vi.fn().mockResolvedValue({ entities: [], relations: [], memories: [], memory_links: [], pages: [], page_links: [] }),
  ...tauriMocks,
  FACET_COLORS: {},
  STABILITY_TIERS: {},
}));

const PAGE = {
  id: "page-1",
  title: "Trail survey",
  space: "Field Notes",
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

function renderPage(access?: TeamAccess) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const page = (
    <QueryClientProvider client={client}>
      <PageDetail pageId="page-1" onBack={vi.fn()} onMemoryClick={vi.fn()} onOpenGraph={vi.fn()} />
    </QueryClientProvider>
  );
  return { user: userEvent.setup(), ...render(access ? <TeamAccessProvider value={access}>{page}</TeamAccessProvider> : page) };
}

const member = (access: "read" | "write") =>
  teamAccessFor({ member_id: "m-amy", name: "Amy", spaces: [{ space: "Field Notes", access }] });

const HISTORY = {
  page_id: "page-1", current_version: 2, user_edited: false, entries: [],
  history: [
    { kind: "ai_update", at: 1_700_000_100, version: 2, edited_by: "agent_refresh", agent: "claude-code", text: "Some body text. An AI tool's sentence." },
    { kind: "person_edit", at: 1_700_000_000, version: 1, edited_by: "manual_edit", text: "Some body text." },
  ],
};

describe("PageDetail info panel for a team member", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.clearAllMocks();
    tauriMocks.getPage.mockResolvedValue({ ...PAGE, version: 2 });
    tauriMocks.getPageSources.mockResolvedValue([]);
    tauriMocks.listRegisteredSources.mockResolvedValue([]);
    tauriMocks.getPageLinks.mockResolvedValue({ outbound: [], inbound: [] });
    tauriMocks.listOrphanLinks.mockResolvedValue({ min_count: 2, orphan_labels: [] });
    tauriMocks.getPageRevisions.mockResolvedValue(HISTORY);
    tauriMocks.listPages.mockResolvedValue([]);
    tauriMocks.getDaemonVersion.mockResolvedValue("0.14.1");
    tauriMocks.getSystemInfo.mockResolvedValue({ os: "macos" });
    tauriMocks.daemonMeetsFloor.mockReturnValue(true);
    tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined);
    tauriMocks.getTruthStatus.mockResolvedValue(null);
    tauriMocks.pageReviewSupported.mockResolvedValue("ready");
    tauriMocks.getPageSentences.mockResolvedValue({ page_id: "page-1", version: 2, ai_edits: "propose", sentences: [], proposals: [] });
    tauriMocks.updatePage.mockResolvedValue({ outcome: "saved" });
  });

  async function openInfo(user: ReturnType<typeof userEvent.setup>) {
    await user.click(await screen.findByRole("button", { name: i18n.t("pageInspector.info") }));
    await screen.findByText(i18n.t("detailInfo.details"));
  }
  const lock = () => screen.queryByRole("button", { name: i18n.t("pageLock.title") });
  const graph = () => document.querySelector(".knowledge-context");
  const basis = () => document.querySelector(".note-info-notes");
  const putBack = () => screen.queryByRole("button", { name: i18n.t("team.changes.revert") });
  async function openNewestChange(user: ReturnType<typeof userEvent.setup>) {
    await waitFor(() => expect(document.querySelector(".note-revision-open")).not.toBeNull());
    await user.click(document.querySelector(".note-revision-open") as HTMLElement);
    await waitFor(() => expect(screen.getAllByRole("dialog").length).toBeGreaterThan(1));
  }

  it("the owner keeps the lock, the graph and the Pages section", async () => {
    const { user } = renderPage();
    await openInfo(user);
    expect(lock()).not.toBeNull();
    await waitFor(() => expect(graph()).not.toBeNull());
    expect(screen.getByText(i18n.t("knowledgeContext.openInGraph"))).toBeInTheDocument();
    expect(basis()).not.toBeNull();
  });

  it("a reader sees the graph, with no lock switch and no Pages section", async () => {
    const { user } = renderPage(member("read"));
    await openInfo(user);
    expect(lock()).toBeNull();
    expect(screen.queryByText(i18n.t("pageLock.offHint"))).toBeNull();
    await waitFor(() => expect(graph()).not.toBeNull());
    expect(basis()).toBeNull();
    expect(tauriMocks.setPageReviewRequired).not.toHaveBeenCalled();
  });

  it("a writer sees the graph and the Pages section, with no lock switch", async () => {
    const { user } = renderPage(member("write"));
    await openInfo(user);
    expect(lock()).toBeNull();
    await waitFor(() => expect(graph()).not.toBeNull());
    expect(basis()).not.toBeNull();
  });

  for (const access of ["read", "write"] as const) {
    it(`a ${access === "read" ? "reader" : "writer"} sees that a locked page is locked, with no switch`, async () => {
      tauriMocks.getPage.mockResolvedValue({ ...PAGE, version: 2, review_required: true });
      const { user } = renderPage(member(access));
      await openInfo(user);
      expect(await screen.findByText(i18n.t("pageLock.lockedNotice"))).toBeInTheDocument();
      expect(lock()).toBeNull();
    });
  }

  it("only the owner gets Update from sources, which has no team row", async () => {
    tauriMocks.getPage.mockResolvedValue({ ...PAGE, version: 2, basis: [{ page_id: "page-2", title: "Bird counts", state: "ok", from_version: 1, to_version: 1, minor_only: false }] });
    const update = () => screen.queryByRole("button", { name: i18n.t("pageDetail.redistillPage") });
    const owner = renderPage();
    await openInfo(owner.user);
    await waitFor(() => expect(update()).not.toBeNull());
    owner.unmount();
    const writer = renderPage(member("write"));
    await openInfo(writer.user);
    expect(await screen.findByText("Bird counts")).toBeInTheDocument();
    expect(update()).toBeNull();
  });

  it("an unlocked page shows a member no lock notice", async () => {
    const { user } = renderPage(member("write"));
    await openInfo(user);
    expect(screen.queryByText(i18n.t("pageLock.lockedNotice"))).toBeNull();
  });

  const actions = () => screen.queryByRole("button", { name: i18n.t("pageDetail.actions") });

  it("a writer's page menu holds only Archive page", async () => {
    const { user } = renderPage(member("write"));
    await user.click(await screen.findByRole("button", { name: i18n.t("pageDetail.actions") }));
    const items = (await screen.findAllByRole("menuitem")).map((item) => item.textContent);
    expect(items).toEqual([i18n.t("pageDetail.archivePage")]);
  });

  for (const access of ["read", "write"] as const) {
    it(`an archived page offers Restore to a ${access === "read" ? "reader: no" : "writer: yes"}`, async () => {
      tauriMocks.getPage.mockResolvedValue({ ...PAGE, version: 2, status: "archived" });
      renderPage(member(access));
      await screen.findByText(i18n.t("pages.archive.readOnly"));
      const restore = screen.queryByRole("button", { name: i18n.t("pages.archive.restore") });
      if (access === "write") expect(restore).not.toBeNull();
      else expect(restore).toBeNull();
    });
  }

  it("a reader gets no page menu", async () => {
    renderPage(member("read"));
    await screen.findByRole("button", { name: i18n.t("pageInspector.info") });
    expect(actions()).toBeNull();
  });

  it("a writer puts an AI tool's change back through the same save as a hand edit", async () => {
    const { user } = renderPage(member("write"));
    await openInfo(user);
    await openNewestChange(user);
    await user.click(await screen.findByRole("button", { name: i18n.t("team.changes.revert") }));
    await waitFor(() => expect(tauriMocks.updatePage).toHaveBeenCalledWith(expect.objectContaining({
      id: "page-1", content: "Some body text.", expectedVersion: 2,
    })));
  });

  it("a reader sees the change with no put back", async () => {
    const { user } = renderPage(member("read"));
    await openInfo(user);
    await openNewestChange(user);
    await waitFor(() => expect(tauriMocks.getPageRevisions).toHaveBeenCalled());
    expect(putBack()).toBeNull();
  });
});
