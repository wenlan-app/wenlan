// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { teamGroups } from "./TeamPage";
import { ScopeProvider, useScopeState } from "./scope";
import { ScopeSwitcher } from "./ScopeSwitcher";
import {
  listShares,
  listSpaces,
  stopSharingSpace,
  teamAddMember,
  teamRename,
  teamStatus,
  type SharedStatus,
  type TeamMember,
  type TeamStatus,
} from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";

vi.mock("../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../lib/tauri")>("../../../lib/tauri");
  return {
    ...actual,
    listShares: vi.fn(),
    listSpaces: vi.fn(),
    listRecentChanges: vi.fn().mockResolvedValue([]),
    listPendingRevisions: vi.fn().mockResolvedValue([]),
    teamStatus: vi.fn(),
    teamAddMember: vi.fn(),
    teamUpdateMember: vi.fn(),
    teamSetEveryone: vi.fn(),
    teamRename: vi.fn(),
    teamHostSpace: vi.fn(),
    stopSharingSpace: vi.fn(),
  };
});
vi.mock("../../../lib/confirmAction", () => ({ confirmAction: vi.fn() }));

const person = (id: string, name: string, spaces: TeamMember["spaces"]): TeamMember =>
  ({ id, name, spaces, added_at: "2026-11-01T00:00:00Z", removed_at: null });

const counts = { exported: 0, applied: 0, conflicts: 0, skipped: 0 };
const share = (target: string, extra: Partial<SharedStatus>): SharedStatus => ({
  kind: "space", target, folder: "/x", member_id: "me", member_name: "Dee",
  members: [{ id: "me", name: "Dee" }], last: { at: "2026-11-11T00:00:00Z", counts }, ...extra,
});

const hostedStatus: TeamStatus = {
  listening: "100.70.1.2:7879",
  spaces: ["Product", "Roadmap"],
  everyone: [{ space: "Product", access: "write" }],
  members: [
    person("a", "Amy", [{ space: "Roadmap", access: "write" }]),
    person("b", "Bo", []),
  ],
};

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) { this.removeAttribute("open"); };
  vi.mocked(listSpaces).mockResolvedValue([]);
  vi.mocked(teamStatus).mockRejectedValue(new Error("off"));
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("teamGroups", () => {
  it("is empty for a computer in no team, so no switch shows", () => {
    expect(teamGroups([share("Wedding", { host: null })], undefined)).toEqual([]);
  });

  it("has the hosted team first, then one per team, known by its id", () => {
    const acme = { id: "t1", name: "Acme" };
    const groups = teamGroups(
      [
        share("Sales", { host: "https://team.example.com/", team: acme }),
        share("Ops", { host: "https://moved.example.com", team: acme }),
        share("Product", { hosted: true }),
      ],
      hostedStatus,
    );
    expect(groups.map((group) => [group.hosted, group.team?.name ?? null, group.shares.length])).toEqual([[true, null, 1], [false, "Acme", 2]]);
  });
});

function Shell() {
  const scope = useScopeState();
  return (
    <ScopeProvider value={scope}>
      <ScopeSwitcher onSwitched={() => undefined} onJoined={() => undefined} />
      <p data-testid="side">{scope.team?.key ?? "private"}</p>
    </ScopeProvider>
  );
}

async function openMenu(current: string) {
  await userEvent.click(await screen.findByRole("button", { name: `Showing ${current}. Switch` }));
  return screen.getByRole("menu");
}

describe("the team's own things, in the switch at the top left, on the computer that hosts it", () => {
  beforeEach(() => {
    vi.mocked(listShares).mockResolvedValue([share("Product", { hosted: true }), share("Roadmap", { hosted: true })]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    localStorage.setItem("wenlan-scope", "hosted");
  });

  it("lists the people in a box, however many there are, never on a page", async () => {
    wrap(<Shell />);
    const menu = await openMenu("Team");
    expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "People in Team…", "Invite someone…", "Rename Team…", "Join a team with an invite link…",
    ]);
    await userEvent.click(within(menu).getByRole("menuitem", { name: "People in Team…" }));
    const box = await screen.findByRole("dialog");
    expect(within(box).getByText("Dee (you) · Owner")).toBeInTheDocument();
    expect(within(box).getByText("Amy")).toBeInTheDocument();
    expect(within(box).getByText("Bo")).toBeInTheDocument();
  });

  it("names the team; everyone on it sees the name", async () => {
    vi.mocked(teamRename).mockResolvedValue({ id: "t1", name: "Acme" });
    wrap(<Shell />);
    await userEvent.click(within(await openMenu("Team")).getByRole("menuitem", { name: "Rename Team…" }));
    await userEvent.type(screen.getByLabelText("Name your team. Everyone on it sees this name."), "Acme");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(teamRename).toHaveBeenCalledWith("Acme"));
  });

  it("invites someone by name and copies a link; no address or key is printed", async () => {
    localStorage.setItem("wenlan-team-url", "https://team.example.com");
    vi.mocked(teamAddMember).mockResolvedValue({ member: person("c", "Cy", []), key: "wlt_k", join_code: "https://team.example.com/join#wlt_k" });
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    wrap(<Shell />);
    await userEvent.click(within(await openMenu("Team")).getByRole("menuitem", { name: "Invite someone…" }));
    await userEvent.type(screen.getByLabelText("Name"), "Cy");
    await userEvent.click(screen.getByRole("button", { name: "Create invite link" }));
    await waitFor(() => expect(teamAddMember).toHaveBeenCalledWith("Cy", [], "https://team.example.com"));
    expect(await screen.findByText("Invite link for Cy")).toBeInTheDocument();
    expect(screen.queryByDisplayValue(/wlt_k/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenCalledWith("https://team.example.com/join#wlt_k");
  });
});

describe("the team's own things on a member's computer", () => {
  const joined = [
    share("Product", { host: "https://team.example.com", team: { id: "t1", name: "Acme" }, members: [{ id: "me", name: "Dee" }, { id: "a", name: "Amy" }] }),
    share("Sales", { host: "https://team.example.com", team: { id: "t1", name: "Acme" }, read_only: true, members: [{ id: "me", name: "Dee" }] }),
  ];
  beforeEach(() => {
    vi.mocked(listShares).mockResolvedValue(joined);
    localStorage.setItem("wenlan-scope", "team:t1");
  });

  it("shows the people by the team's name, with no way to invite or rename", async () => {
    wrap(<Shell />);
    const menu = await openMenu("Acme");
    expect(within(menu).queryByRole("menuitem", { name: "Invite someone…" })).not.toBeInTheDocument();
    expect(within(menu).queryByRole("menuitem", { name: /Rename/ })).not.toBeInTheDocument();
    await userEvent.click(within(menu).getByRole("menuitem", { name: "People in Acme…" }));
    const box = await screen.findByRole("dialog");
    expect(within(box).getByText("Dee (you)")).toBeInTheDocument();
    expect(within(box).getByText("Amy")).toBeInTheDocument();
    expect(within(box).queryByRole("button", { name: "Invite someone" })).not.toBeInTheDocument();
  });

  it("leaves the team: every Space of that server stops syncing here, and Private shows", async () => {
    vi.mocked(confirmAction).mockResolvedValue(true);
    vi.mocked(stopSharingSpace).mockResolvedValue();
    wrap(<Shell />);
    await userEvent.click(within(await openMenu("Acme")).getByRole("menuitem", { name: "Leave Acme…" }));
    await waitFor(() => expect(stopSharingSpace).toHaveBeenCalledTimes(2));
    expect(stopSharingSpace).toHaveBeenCalledWith("Product");
    expect(stopSharingSpace).toHaveBeenCalledWith("Sales");
    await waitFor(() => expect(screen.getByTestId("side")).toHaveTextContent("private"));
  });

  it("stays on the team when leaving is cancelled", async () => {
    vi.mocked(confirmAction).mockResolvedValue(false);
    wrap(<Shell />);
    await userEvent.click(within(await openMenu("Acme")).getByRole("menuitem", { name: "Leave Acme…" }));
    await waitFor(() => expect(confirmAction).toHaveBeenCalled());
    expect(stopSharingSpace).not.toHaveBeenCalled();
    expect(screen.getByTestId("side")).toHaveTextContent("team:t1");
  });
});
