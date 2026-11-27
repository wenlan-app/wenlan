// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import type { Page, TeamDraft, TeamPublication, TeamPublicationPreview, TeamStatus } from "../../../lib/tauri";
import { ShareDialog } from "./ShareDialog";
import { TeamSpaceShareFlow } from "./TeamReviewWindow";

const api = vi.hoisted(() => ({
  listPages: vi.fn(),
  listShares: vi.fn(),
  teamStatus: vi.fn(),
  teamLibraryDraft: vi.fn(),
  teamLibrarySpaces: vi.fn(),
  teamLibraryPreview: vi.fn(),
  teamLibraryPublish: vi.fn(),
  teamLibraryCreateSpace: vi.fn(),
  lookInSharedFolder: vi.fn(),
  revealSharedFolder: vi.fn(),
  sharePage: vi.fn(),
  shareSpace: vi.fn(),
  stopSharingPage: vi.fn(),
  stopSharingSpace: vi.fn(),
}));
const toastApi = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));

vi.mock("../../../lib/tauri", () => api);
vi.mock("sonner", () => ({ toast: toastApi }));

const teamStatus: TeamStatus = {
  listening: "100.70.1.2:7879",
  team: { id: "team-1", name: "Acme" },
  members: [],
  spaces: [],
  everyone: [{ space: "Product", access: "write" }],
};

const draft: TeamDraft = {
  item: { original: { kind: "page", id: "page-1", revision: 4 }, text: { title: "Original title", body: "Original body" } },
  sources: [
    { original: { kind: "memory", id: "memory-1", revision: 2 }, text: { title: "Private memory one", body: "One" } },
    { original: { kind: "memory", id: "memory-2", revision: 3 }, text: { title: "Private memory two", body: "Two" } },
  ],
  omitted_sources: false,
  authorship: "human",
};

function preview(id: string, title = "Server title", body = "Server preview body"): TeamPublicationPreview {
  return {
    id,
    team: { id: "team-1", name: "Acme" },
    space: null,
    audience: { now: [], later: "" },
    draft: {
      kind: "page",
      title,
      body,
      authorship: "human",
      space_id: null,
      item_id: "page-1",
      expected_revision: 4,
      memories: [],
      private_source: true,
      brief: false,
    },
  };
}

function wrap(node: React.ReactNode) {
  return render(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>{node}</QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) { this.removeAttribute("open"); };
  vi.clearAllMocks();
  api.listShares.mockResolvedValue([]);
  api.teamStatus.mockResolvedValue(teamStatus);
  api.teamLibraryDraft.mockResolvedValue(draft);
  api.teamLibrarySpaces.mockResolvedValue([{ id: "space-1", name: "Product", restricted: false }]);
  api.teamLibraryPreview.mockResolvedValue({ result: "preview", preview: preview("preview-1") });
  api.teamLibraryPublish.mockResolvedValue({ result: "published", receipt: { team_id: "team-1", item_id: "page-1", revision: 5, memory_ids: [] } });
  api.listPages.mockResolvedValue([]);
});

describe("team page sharing", () => {
  it("UX-SHARE-1 offers the team option for a page when this computer listens", async () => {
    wrap(<ShareDialog kind="page" target="page-1" title="Launch plan" onClose={() => undefined} />);
    expect(await screen.findByRole("button", { name: /With Acme/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /With one person/ })).toBeInTheDocument();
  });

  it("UX-SHARE-2 leaves cited memories unchecked and shows the preview reply text", async () => {
    wrap(<ShareDialog kind="page" target="page-1" title="Launch plan" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    expect(await screen.findByText("Server title")).toBeInTheDocument();
    expect(screen.getByText("Server preview body")).toBeInTheDocument();
    expect(screen.getByLabelText("Private memory one")).not.toBeChecked();
    expect(screen.getByLabelText("Private memory two")).not.toBeChecked();
  });

  it("UX-SHARE-2 renders the preview body as a page without repeating its title heading", async () => {
    api.teamLibraryPreview.mockResolvedValue({ result: "preview", preview: preview("preview-1", "Launch checklist", "# Launch checklist\n\nRollback plan **signed off**.") });
    wrap(<ShareDialog kind="page" target="page-1" title="Launch plan" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    expect(await screen.findByRole("heading", { name: "Launch checklist" })).toBeInTheDocument();
    expect(screen.getAllByText(/Launch checklist/)).toHaveLength(1);
    expect(screen.queryByText(/# Launch checklist/)).not.toBeInTheDocument();
    expect(screen.getByText("signed off").tagName).toBe("STRONG");
  });

  it("UX-SHARE-6 localizes draft failures without exposing daemon routes or ids", async () => {
    api.teamLibraryDraft.mockRejectedValue(new Error("HTTP GET /api/team/library/draft?id=secret-page returned 404"));
    wrap(<ShareDialog kind="page" target="secret-page" title="Launch plan" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("This page changed. Close and reopen it.");
    expect(alert).not.toHaveTextContent("/api/");
    expect(alert).not.toHaveTextContent("secret-page");
  });

  it("UX-SHARE-2 re-previews the current selection and UX-SHARE-3 publishes only ticked memories with its latest preview", async () => {
    api.teamLibraryPreview
      .mockResolvedValueOnce({ result: "preview", preview: preview("preview-1") })
      .mockResolvedValueOnce({ result: "preview", preview: preview("preview-2", "Updated title", "Updated body") });
    wrap(<ShareDialog kind="page" target="page-1" title="Launch plan" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    await userEvent.click(await screen.findByLabelText("Private memory two"));
    expect(await screen.findByText("Updated body")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Publish to Acme" }));
    await waitFor(() => expect(api.teamLibraryPublish).toHaveBeenCalledWith("preview-2"));
    expect(toastApi.success).toHaveBeenCalledWith("Published to Acme.");
    expect(api.teamLibraryPreview).toHaveBeenLastCalledWith(
      expect.objectContaining({ memories: [{ title: "Private memory two", body: "Two" }], private_source: true, item_id: null, expected_revision: null }),
      [draft.item.original, draft.sources[1].original],
    );
  });

  it("UX-SHARE-3 creates a team Space, then walks only ticked pages through review", async () => {
    const page = (id: string, title: string): Page => ({
      id, title, summary: null, content: `${title} body`, entity_id: null, domain: null, space: "Product",
      source_memory_ids: [], version: 1, status: "active", created_at: "2026-11-26", last_compiled: "2026-11-26", last_modified: "2026-11-26",
    });
    api.teamLibraryCreateSpace.mockResolvedValue({ result: "space", space: { id: "new-space", name: "Product", restricted: false } });
    api.listPages.mockResolvedValue([page("page-1", "First page"), page("page-2", "Second page")]);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    await waitFor(() => expect(api.teamLibraryCreateSpace).toHaveBeenCalledWith("Product", false));
    await userEvent.click(await screen.findByLabelText("First page"));
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByRole("heading", { name: "Check before sending to Acme" })).toBeInTheDocument();
    expect(api.teamLibraryDraft).toHaveBeenCalledWith("page", "page-1");
    await userEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(await screen.findByText("No pages published; skipped 1.")).toBeInTheDocument();
    expect(api.teamLibraryDraft).not.toHaveBeenCalledWith("page", "page-2");
  });

  it("UX-SHARE-6 explains a team Space name conflict instead of asking to reopen", async () => {
    api.teamLibraryCreateSpace.mockRejectedValue(new Error("team_space_taken"));
    api.listPages.mockResolvedValue([]);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The team already has a Space with this name but different access. Pick another name.");
  });

  it("UX-SHARE-5 resets memory selections and previews when the Space walk changes pages", async () => {
    const page = (id: string, title: string): Page => ({
      id, title, summary: null, content: `${title} body`, entity_id: null, domain: null, space: "Product",
      source_memory_ids: ["memory-1"], version: 1, status: "active", created_at: "2026-11-26", last_compiled: "2026-11-26", last_modified: "2026-11-26",
    });
    const pageDraft = (id: string): TeamDraft => ({
      ...draft,
      sources: [draft.sources[0]],
      item: { ...draft.item, original: { ...draft.item.original, id }, text: { title: id, body: `${id} body` } },
    });
    api.teamLibraryCreateSpace.mockResolvedValue({ result: "space", space: { id: "new-space", name: "Product", restricted: false } });
    api.listPages.mockResolvedValue([page("page-1", "First page"), page("page-2", "Second page")]);
    api.teamLibraryDraft.mockImplementation((_kind: string, id: string) => Promise.resolve(pageDraft(id)));
    api.teamLibraryPreview.mockImplementation(async (publication: TeamPublication) => ({
      result: "preview",
      preview: preview(publication.memories.length > 0 ? "with-memory" : "without-memory", publication.title, publication.body),
    }));

    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    await userEvent.click(await screen.findByLabelText("First page"));
    await userEvent.click(screen.getByLabelText("Second page"));
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    await userEvent.click(await screen.findByLabelText("Private memory one"));
    await userEvent.click(await screen.findByRole("button", { name: "Publish to Acme" }));
    await waitFor(() => expect(api.teamLibraryDraft).toHaveBeenLastCalledWith("page", "page-2"));
    expect(screen.getByLabelText("Private memory one")).not.toBeChecked();
    await waitFor(() => expect(api.teamLibraryPreview).toHaveBeenLastCalledWith(
      expect.objectContaining({ memories: [], private_source: true }),
      [expect.objectContaining({ id: "page-2" })],
    ));
  });

  it("UX-SHARE-4 reopens the team Space flow after cancelling a previous attempt", async () => {
    api.teamLibraryCreateSpace.mockResolvedValue({ result: "space", space: { id: "space-1", name: "Product", restricted: false } });
    api.listPages.mockResolvedValue([]);
    const onClose = vi.fn();
    const { rerender } = wrap(<TeamSpaceShareFlow spaceName="Product" onClose={onClose} />);
    expect(await screen.findByText("There are no pages in this Space yet.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(onClose).toHaveBeenCalledTimes(1);

    rerender(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <TeamSpaceShareFlow key="reopened" spaceName="Product" onClose={onClose} />
      </QueryClientProvider>,
    );
    await waitFor(() => expect(api.teamLibraryCreateSpace).toHaveBeenCalledTimes(2));
    expect(await screen.findByText("There are no pages in this Space yet.")).toBeInTheDocument();
  });
});
