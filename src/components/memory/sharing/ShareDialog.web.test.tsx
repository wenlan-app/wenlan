// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { liveInvoke } from "../../../web/live-invoke";
import { ShareDialog } from "./ShareDialog";

const api = vi.hoisted(() => ({
  listShares: vi.fn(),
  teamStatus: vi.fn(),
  teamLibrarySpaces: vi.fn(),
  teamLibraryDraft: vi.fn(),
  teamLibraryPreview: vi.fn(),
  teamLibraryPublish: vi.fn(),
  teamLibraryCreateSpace: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => ({
  ...api,
  lookInSharedFolder: vi.fn(),
  revealSharedFolder: vi.fn(),
  sharePage: vi.fn(),
  shareSpace: vi.fn(),
  stopSharingPage: vi.fn(),
  stopSharingSpace: vi.fn(),
  listPages: vi.fn(),
}));
vi.mock("../../../web/runtime", () => ({ isWebBuild: () => true }));

afterEach(() => {
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

beforeEach(async () => {
  await i18n.changeLanguage("en");
  api.listShares.mockResolvedValue([]);
  api.teamStatus.mockResolvedValue({ listening: "100.70.1.2:7879", team: { id: "team-1", name: "Acme" }, members: [], spaces: [] });
  api.teamLibrarySpaces.mockResolvedValue([]);
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) { this.removeAttribute("open"); };
});

describe("web team sharing", () => {
  it("UX-SHARE-1 shows the team option and hides the folder option", async () => {
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ShareDialog kind="page" target="page-1" title="Launch plan" onClose={() => undefined} />
      </QueryClientProvider>,
    );
    expect(await screen.findByRole("button", { name: /With Acme/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /With one person/ })).not.toBeInTheDocument();
  });

  it("UX-SHARE-2 clicks through the web team review and publishes through daemon routes", async () => {
    const draft = {
      item: { original: { kind: "page", id: "page-1", revision: 4 }, text: { title: "Original", body: "Original body" } },
      sources: [{ original: { kind: "memory", id: "memory-1", revision: 2 }, text: { title: "Private memory", body: "Memory body" } }],
      omitted_sources: false,
      authorship: "human",
    };
    const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/team") return new Response(JSON.stringify({ listening: "100.70.1.2:7879", team: { id: "team-1", name: "Acme" }, members: [], spaces: [] }));
      if (url === "/api/team/library/spaces") return new Response(JSON.stringify([{ id: "space-1", name: "Product", restricted: false }]));
      if (url.startsWith("/api/team/library/draft")) return new Response(JSON.stringify(draft));
      if (url === "/api/team/library/terminal") {
        const body = JSON.parse(String(init?.body ?? "{}")) as { action: string; draft?: Record<string, unknown> };
        if (body.action === "preview") {
          return new Response(JSON.stringify({
            result: "preview",
            preview: {
              id: "preview-1",
              team: { id: "team-1", name: "Acme" },
              space: null,
              audience: { now: [], later: "" },
              draft: { ...body.draft, title: "Server preview title", body: "Server preview body" },
            },
          }));
        }
        return new Response(JSON.stringify({ result: "published", receipt: { team_id: "team-1", item_id: "page-1", revision: 5, memory_ids: [] } }));
      }
      return new Response(JSON.stringify({ error: "unexpected route" }), { status: 404 });
    });
    vi.stubGlobal("fetch", fetch);
    sessionStorage.setItem("wenlan-person-key", "ab".repeat(32));
    api.teamStatus.mockImplementation(() => liveInvoke("team_status"));
    api.teamLibrarySpaces.mockImplementation(() => liveInvoke("team_library_spaces"));
    api.teamLibraryDraft.mockImplementation((kind: string, id: string) => liveInvoke("team_library_draft", { kind, id }));
    api.teamLibraryPreview.mockImplementation((draftValue: unknown, originals: unknown[]) => liveInvoke("team_library_preview", { draft: draftValue, originals }));
    api.teamLibraryPublish.mockImplementation((previewId: string) => liveInvoke("team_library_publish", { previewId }));

    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ShareDialog kind="page" target="page-1" title="Launch plan" onClose={() => undefined} />
      </QueryClientProvider>,
    );
    await userEvent.click(await screen.findByRole("button", { name: /With Acme/ }));
    expect(await screen.findByText("Server preview title")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Publish to Acme" }));
    await waitFor(() => expect(fetch).toHaveBeenCalledWith("/api/team/library/terminal", expect.objectContaining({ method: "POST" })));
    expect(fetch.mock.calls.some(([url, init]) => String(url) === "/api/team/library/terminal" && String(init?.body).includes('"action":"publish"'))).toBe(true);
  });
});
