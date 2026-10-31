// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import PageDetail from "./PageDetail";
import { editorViewFromTextbox, installCodeMirrorDomPolyfills } from "./editor/editorTestUtils";
import { i18n } from "../../i18n";

const tauriMocks = vi.hoisted(() => ({
  getPage: vi.fn(),
  getKnowledgeGraph: vi.fn(),
  getPageSources: vi.fn(),
  listRegisteredSources: vi.fn(),
  getPageLinks: vi.fn(),
  getPageRevisions: vi.fn(),
  updatePage: vi.fn(),
  getDaemonVersion: vi.fn(),
  getSystemInfo: vi.fn(),
  recordPageEditorDiagnostic: vi.fn(),
  getPageSentences: vi.fn(),
  acceptSentenceProposal: vi.fn(),
  rejectSentenceProposal: vi.fn(),
}));

vi.mock("../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../lib/tauri")>()),
  ...tauriMocks,
}));

const PAGE = {
  id: "page-proposals",
  title: "New tagline",
  summary: null,
  content: "The tagline is short. It names a noun.\n",
  entity_id: null,
  domain: "testing",
  source_memory_ids: [],
  version: 3,
  status: "active",
  user_edited: true,
  created_at: "2026-10-31T00:00:00+00:00",
  last_compiled: "2026-10-31T00:00:00+00:00",
  last_modified: "2026-10-31T00:00:00+00:00",
};

const PROPOSAL = {
  id: "sp_1",
  page_id: PAGE.id,
  version: 3,
  sentence_hash: "h1",
  before: "It names a noun.",
  after: "It names a concrete noun.",
  actor: { kind: "ai", name: "codex" },
  ts: 1_793_480_000,
  status: "pending",
};

function sentences(proposals: unknown[]) {
  return { page_id: PAGE.id, version: 3, ai_edits: "propose", sentences: [], proposals };
}

beforeAll(() => {
  installCodeMirrorDomPolyfills();
});

beforeEach(() => {
  vi.clearAllMocks();
  tauriMocks.getPage.mockResolvedValue(PAGE);
  tauriMocks.getKnowledgeGraph.mockResolvedValue({ entities: [], relations: [], memories: [], memory_links: [], pages: [], page_links: [] });
  tauriMocks.getPageSources.mockResolvedValue([]);
  tauriMocks.listRegisteredSources.mockResolvedValue([]);
  tauriMocks.getPageLinks.mockResolvedValue({ outbound: [], inbound: [] });
  tauriMocks.getPageRevisions.mockResolvedValue({ page_id: PAGE.id, current_version: 3, user_edited: true, entries: [] });
  tauriMocks.getDaemonVersion.mockResolvedValue("0.20.5");
  tauriMocks.getSystemInfo.mockResolvedValue({ os: "macos" });
  tauriMocks.recordPageEditorDiagnostic.mockResolvedValue(undefined);
  tauriMocks.getPageSentences.mockResolvedValue(sentences([PROPOSAL]));
});

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <PageDetail pageId={PAGE.id} initialMode="edit" onBack={vi.fn()} onMemoryClick={vi.fn()} onPageClick={vi.fn()} />
    </QueryClientProvider>,
  );
}

async function proposal() {
  const textbox = await screen.findByRole("textbox", { name: "Page editor" });
  const view = editorViewFromTextbox(textbox);
  await waitFor(() => expect(view.dom.querySelector("[data-testid=sentence-proposal]")).not.toBeNull());
  return { view, widget: view.dom.querySelector<HTMLElement>("[data-testid=sentence-proposal]")! };
}

describe("PageDetail sentence proposals", () => {
  it("UX-WHO-2 shows the AI's proposal under the person's sentence and accepts it as their edit", async () => {
    renderPage();
    const { view, widget } = await proposal();
    expect(widget.textContent).toContain(i18n.t("sentenceProposals.suggests"));
    expect(widget.textContent).toContain("It names a concrete noun.");

    const accepted = { ...PAGE, content: "The tagline is short. It names a concrete noun.\n", version: 4 };
    tauriMocks.acceptSentenceProposal.mockImplementation(async () => {
      tauriMocks.getPage.mockResolvedValue(accepted);
      tauriMocks.getPageSentences.mockResolvedValue(sentences([]));
      return { proposal: { ...PROPOSAL, status: "accepted" }, version: 4 };
    });
    widget.querySelector<HTMLButtonElement>("[data-testid=sentence-proposal-accept]")!.click();
    await waitFor(() => expect(tauriMocks.acceptSentenceProposal).toHaveBeenCalledWith(PAGE.id, "sp_1"));
    await waitFor(() => expect(view.dom.querySelector("[data-testid=sentence-proposal]")).toBeNull());
    await waitFor(() => expect(
      editorViewFromTextbox(screen.getByRole("textbox", { name: "Page editor" })).state.doc.toString(),
    ).toContain("It names a concrete noun."));
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("turns a proposal down without touching the page", async () => {
    renderPage();
    const { view, widget } = await proposal();
    tauriMocks.rejectSentenceProposal.mockImplementation(async () => {
      tauriMocks.getPageSentences.mockResolvedValue(sentences([]));
      return { proposal: { ...PROPOSAL, status: "rejected" } };
    });
    widget.querySelector<HTMLButtonElement>("[data-testid=sentence-proposal-reject]")!.click();
    await waitFor(() => expect(tauriMocks.rejectSentenceProposal).toHaveBeenCalledWith(PAGE.id, "sp_1"));
    await waitFor(() => expect(view.dom.querySelector("[data-testid=sentence-proposal]")).toBeNull());
    expect(view.state.doc.toString()).toContain("It names a noun.");
    expect(tauriMocks.updatePage).not.toHaveBeenCalled();
  });

  it("says so when accepting fails, and keeps the proposal", async () => {
    tauriMocks.acceptSentenceProposal.mockRejectedValue(new Error("sentence_changed"));
    renderPage();
    const { widget } = await proposal();
    widget.querySelector<HTMLButtonElement>("[data-testid=sentence-proposal-accept]")!.click();
    expect(await screen.findByText(i18n.t("sentenceProposals.acceptFailed"))).toBeInTheDocument();
    await proposal();
  });
});
