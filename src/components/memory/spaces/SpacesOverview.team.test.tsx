// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { makeSpace, renderOverview, labels, createQueryClient } from "./SpacesOverview.testUtils";
import { ScopeProvider, type ScopeState } from "../team/scope";
import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { SpacesOverview } from "./SpacesOverview";

const api = vi.hoisted(() => ({
  listSpaces: vi.fn(),
  listPages: vi.fn(),
  listShares: vi.fn(),
  createSpace: vi.fn(),
  teamLibraryCreateSpace: vi.fn(),
  teamLibrarySpaces: vi.fn(),
  teamJoinedLibrary: vi.fn(),
}));
vi.mock("../../../lib/tauri", () => api);

const share = (target: string, extra: object) => ({
  kind: "space", target, folder: "", member_id: "me", member_name: "Me", members: [], ...extra,
});

beforeEach(async () => {
  vi.clearAllMocks();
  await i18n.changeLanguage("en");
  api.listPages.mockResolvedValue([]);
  api.teamJoinedLibrary.mockResolvedValue({ teams: [] });
  api.teamLibraryCreateSpace.mockResolvedValue({ result: "space", space: { id: "team-space", name: "Docs", restricted: false } });
  api.teamLibrarySpaces.mockResolvedValue([]);
  api.listSpaces.mockResolvedValue([
    makeSpace({ id: "a", name: "Reading", sort_order: 0 }),
    makeSpace({ id: "b", name: "Product", sort_order: 1 }),
  ]);
});

describe("SpacesOverview team groups", () => {
  it("UX-SHARE-5 lists team-library Spaces returned by the person-only query", async () => {
    api.teamLibrarySpaces.mockResolvedValue([{ id: "library-space", name: "Roadmap", restricted: false }]);
    const teamScope: ScopeState = {
      teams: [],
      team: { key: "hosted", team: { id: "t1", name: "Acme" }, hosted: true, owner: true, host: null, shares: [] },
      setScope: () => undefined,
      teamSpaces: new Set(),
      inScope: () => true,
      teamWritable: [],
    };
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <ScopeProvider value={teamScope}>
          <SpacesOverview labels={labels} onSelectSpace={() => undefined} />
        </ScopeProvider>
      </QueryClientProvider>,
    );
    expect(await screen.findByText("Roadmap")).toBeInTheDocument();
  });

  it("titles the person's own Spaces only when a team group exists", async () => {
    api.listShares.mockResolvedValue([share("Product", { host: "https://team.example.com", team: { id: "t1", name: "Acme" } })]);
    renderOverview();
    expect(await screen.findByRole("heading", { name: "Acme" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Private" })).toBeInTheDocument();
  });

  it("shows no title when nothing is shared with a team", async () => {
    api.listShares.mockResolvedValue([]);
    renderOverview();
    await screen.findByText("Reading");
    expect(screen.queryByRole("heading", { name: "Private" })).not.toBeInTheDocument();
  });

  it("UX-TEAM-MEMBER-1: renders live team Spaces and published pages without Space actions", async () => {
    const open = vi.fn();
    api.listShares.mockResolvedValue([share("Product", { host: "https://team.example.com", team: { id: "t1", name: "Acme" } })]);
    api.teamJoinedLibrary.mockResolvedValue({
      teams: [{
        team: { id: "t1", name: "Acme" },
        library: {
          state: "ready",
          spaces: [{
            space: { id: "library-roadmap", name: "Roadmap", restricted: false },
            pages: [{ id: "page-1", title: "Launch plan", space: "Roadmap", published_by: null, published_at: null }],
          }],
        },
      }],
    });
    renderOverview({ onOpenTeamPage: open });
    const group = await screen.findByTestId("team-joined-library");
    expect(within(group).getByRole("heading", { name: "Team · Acme" })).toBeInTheDocument();
    expect(within(group).getByRole("heading", { name: "Roadmap" })).toBeInTheDocument();
    fireEvent.click(within(group).getByRole("button", { name: "Launch plan" }));
    expect(open).toHaveBeenCalledWith("t1", "page-1");
    expect(within(group).queryByRole("button", { name: /Actions for|Rename|Delete|New/ })).not.toBeInTheDocument();
  });

  it("UX-TEAM-MEMBER-1: renders whole-team pages above team Spaces", async () => {
    const open = vi.fn();
    api.listShares.mockResolvedValue([]);
    api.teamJoinedLibrary.mockResolvedValue({
      teams: [{
        team: { id: "t1", name: "Acme" },
        library: {
          state: "ready",
          team_pages: [{ id: "team-page", title: "Team announcement", space: null, published_by: null, published_at: null }],
          spaces: [],
        },
      }],
    });
    renderOverview({ onOpenTeamPage: open });
    const group = await screen.findByTestId("team-joined-library");
    expect(within(group).getByRole("heading", { name: "Whole team" })).toBeInTheDocument();
    fireEvent.click(within(group).getByRole("button", { name: "Team announcement" }));
    expect(open).toHaveBeenCalledWith("t1", "team-page");
  });

  it("shows Private when a joined team exists without legacy shares", async () => {
    api.listShares.mockResolvedValue([]);
    api.teamJoinedLibrary.mockResolvedValue({
      teams: [{ team: { id: "t1", name: "Acme" }, library: { state: "ready", team_pages: [], spaces: [] } }],
    });
    renderOverview();
    expect(await screen.findByRole("heading", { name: "Private" })).toBeInTheDocument();
  });

  it("UX-SHARE-4 creates a team-side Space with CreateSpace and does not create a private one", async () => {
    const teamScope: ScopeState = {
      teams: [],
      team: { key: "team:t1", team: { id: "t1", name: "Acme" }, hosted: false, owner: true, host: "https://team.example.com", shares: [] },
      setScope: () => undefined,
      teamSpaces: new Set(),
      inScope: () => true,
      teamWritable: [],
    };
    api.listSpaces.mockResolvedValue([]);
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <ScopeProvider value={teamScope}>
          <SpacesOverview labels={labels} onSelectSpace={() => undefined} />
        </ScopeProvider>
      </QueryClientProvider>,
    );
    await screen.findByText("No confirmed spaces");
    fireEvent.click(screen.getByRole("button", { name: labels.newSpace }));
    fireEvent.change(screen.getByLabelText(labels.nameLabel), { target: { value: "Docs" } });
    fireEvent.submit(screen.getByLabelText(labels.nameLabel).closest("form")!);
    await waitFor(() => expect(api.teamLibraryCreateSpace).toHaveBeenCalledWith("Docs", false));
    expect(api.listSpaces).toHaveBeenCalled();
  });

  it("UX-SHARE-4 leaves no private Space behind when CreateSpace fails", async () => {
    const teamScope: ScopeState = {
      teams: [],
      team: { key: "team:t1", team: { id: "t1", name: "Acme" }, hosted: false, owner: true, host: "https://team.example.com", shares: [] },
      setScope: () => undefined,
      teamSpaces: new Set(),
      inScope: () => true,
      teamWritable: [],
    };
    api.listSpaces.mockResolvedValue([]);
    api.teamLibraryCreateSpace.mockRejectedValue(new Error("team unavailable"));
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <ScopeProvider value={teamScope}>
          <SpacesOverview labels={labels} onSelectSpace={() => undefined} />
        </ScopeProvider>
      </QueryClientProvider>,
    );
    await screen.findByText("No confirmed spaces");
    fireEvent.click(screen.getByRole("button", { name: labels.newSpace }));
    const nameInput = screen.getByLabelText(labels.nameLabel);
    fireEvent.change(nameInput, { target: { value: "Docs" } });
    fireEvent.submit(nameInput.closest("form")!);

    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(labels.mutationError));
    expect(api.createSpace).not.toHaveBeenCalled();
  });
});
