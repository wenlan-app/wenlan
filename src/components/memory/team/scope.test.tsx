// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { listShares, teamStatus, type Page, type SharedStatus } from "../../../lib/tauri";
import { ScopeProvider, useScope, useScopeState } from "./scope";
import { ScopeSwitcher } from "./ScopeSwitcher";
import { scopedFolders } from "../pages/PageInventoryPanel";

vi.mock("../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../lib/tauri")>("../../../lib/tauri");
  return { ...actual, listShares: vi.fn(), teamStatus: vi.fn(), listSpaces: vi.fn().mockResolvedValue([]) };
});

const counts = { exported: 0, applied: 0, conflicts: 0, skipped: 0 };
const share = (target: string, extra: Partial<SharedStatus>): SharedStatus => ({
  kind: "space", target, folder: "/x", member_id: "me", member_name: "Dee",
  members: [{ id: "me", name: "Dee" }], last: { at: "2026-11-11T00:00:00Z", counts }, ...extra,
});
const acme = { id: "t1", name: "Acme" };

function Probe() {
  const scope = useScope();
  return <p data-testid="probe">{["Wedding", "Product", null].map((space) => `${space ?? "none"}:${scope.inScope(space)}`).join(" ")}</p>;
}

function Shell({ onSwitched = () => undefined }: { readonly onSwitched?: () => void }) {
  const scope = useScopeState();
  return (
    <ScopeProvider value={scope}>
      <ScopeSwitcher onSwitched={onSwitched} onJoined={() => undefined} />
      <Probe />
    </ScopeProvider>
  );
}

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.mocked(teamStatus).mockRejectedValue(new Error("off"));
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("the Private / team switch", () => {
  it("is not there for someone on no team, who sees the whole library", async () => {
    vi.mocked(listShares).mockResolvedValue([share("Wedding", { host: null })]);
    wrap(<Shell />);
    await waitFor(() => expect(listShares).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: /Switch/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/team/i)).not.toBeInTheDocument();
    expect(screen.getByTestId("probe")).toHaveTextContent("Wedding:true Product:true none:true");
  });

  it("shows one side at a time: Private leaves the team's Spaces out, the team shows only them", async () => {
    vi.mocked(listShares).mockResolvedValue([share("Product", { host: "https://team.example.com", team: acme })]);
    const onSwitched = vi.fn();
    wrap(<Shell onSwitched={onSwitched} />);
    const button = await screen.findByRole("button", { name: "Showing Private. Switch" });
    expect(screen.getByTestId("probe")).toHaveTextContent("Wedding:true Product:false none:true");

    await userEvent.click(button);
    await userEvent.click(screen.getByRole("menuitemradio", { name: "Acme" }));
    expect(onSwitched).toHaveBeenCalled();
    expect(await screen.findByRole("button", { name: "Showing Acme. Switch" })).toBeInTheDocument();
    expect(screen.getByTestId("probe")).toHaveTextContent("Wedding:false Product:true none:false");
    // The side is remembered for the next start.
    expect(localStorage.getItem("wenlan-scope")).toBe("team:t1");
  });

  it("falls back to Private when the remembered team is gone", async () => {
    localStorage.setItem("wenlan-scope", "team:gone");
    vi.mocked(listShares).mockResolvedValue([share("Product", { host: "https://team.example.com", team: acme })]);
    wrap(<Shell />);
    expect(await screen.findByRole("button", { name: "Showing Private. Switch" })).toBeInTheDocument();
  });
});

describe("folders on one side", () => {
  const page = (id: string, path: string, space: string | null) =>
    ({ id, status: "active", space, storage_path: path } as unknown as Page);
  const all = [page("1", "Work/a.md", "Wedding"), page("2", "Acme/Product/b.md", "Product")];

  it("shows every folder when there is no team", () => {
    const shown = scopedFolders(all, all, false, false);
    expect(shown("Acme")).toBe(true);
  });

  it("hides a folder holding only the other side's pages", () => {
    const privateSide = scopedFolders(all, [all[0]!], true, false);
    expect(privateSide("Work")).toBe(true);
    expect(privateSide("Acme")).toBe(false);
    expect(privateSide("Empty")).toBe(true);
    const teamSide = scopedFolders(all, [all[1]!], true, true);
    expect(teamSide("Acme")).toBe(true);
    expect(teamSide("Acme/Product")).toBe(true);
    expect(teamSide("Work")).toBe(false);
    expect(teamSide("Empty")).toBe(false);
  });
});
