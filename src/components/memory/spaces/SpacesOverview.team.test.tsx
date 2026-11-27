// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, screen, waitFor } from "@testing-library/react";
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
}));
vi.mock("../../../lib/tauri", () => api);

const share = (target: string, extra: object) => ({
  kind: "space", target, folder: "", member_id: "me", member_name: "Me", members: [], ...extra,
});

beforeEach(async () => {
  vi.clearAllMocks();
  await i18n.changeLanguage("en");
  api.listPages.mockResolvedValue([]);
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
