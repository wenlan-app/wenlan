// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { TeamPage, teamGroups } from "./TeamPage";
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
  it("is empty for a computer in no team, so the navigation shows no Team", () => {
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

describe("TeamPage on the computer that hosts the team", () => {
  beforeEach(() => {
    vi.mocked(listShares).mockResolvedValue([share("Product", { hosted: true }), share("Roadmap", { hosted: true })]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
  });

  it("shows the team's Spaces first, who can use each, then its people", async () => {
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    const product = (await screen.findByRole("button", { name: "Product" })).closest("li")!;
    expect(within(product).getByText("Everyone · can edit")).toBeInTheDocument();
    expect(within(screen.getByRole("button", { name: "Roadmap" }).closest("li")!).getByText("Amy")).toBeInTheDocument();
    expect(screen.getByText("Dee (you) · Owner")).toBeInTheDocument();
    expect(screen.getByText("Bo", { selector: ".team-page-name" })).toBeInTheDocument();
    const headings = screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent);
    expect(headings.slice(0, 2)).toEqual(["Spaces", "People"]);
    // No counts after people, no dropdowns on the page.
    expect(screen.queryByText(/\d Spaces?$/)).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("asks the owner to name the team once; everyone sees the name", async () => {
    vi.mocked(teamRename).mockResolvedValue({ id: "t1", name: "Acme" });
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    expect(await screen.findByRole("heading", { name: "Team", level: 1 })).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText("Name your team. Everyone on it sees this name."), "Acme");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(teamRename).toHaveBeenCalledWith("Acme"));
  });

  it("opens one Space's access box from its row", async () => {
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: "Who can use Roadmap" }));
    expect(await screen.findByRole("heading", { name: "Roadmap · Team" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Only some people" })).toBeChecked();
  });

  it("invites someone by name and copies a link; no address or key is printed", async () => {
    localStorage.setItem("wenlan-team-url", "https://team.example.com");
    vi.mocked(teamAddMember).mockResolvedValue({ member: person("c", "Cy", []), key: "wlt_k", join_code: "https://team.example.com/join#wlt_k" });
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: "Invite someone" }));
    await userEvent.type(screen.getByLabelText("Name"), "Cy");
    await userEvent.click(screen.getByRole("button", { name: "Create invite link" }));
    await waitFor(() => expect(teamAddMember).toHaveBeenCalledWith("Cy", [], "https://team.example.com"));
    expect(await screen.findByText("Invite link for Cy")).toBeInTheDocument();
    expect(screen.queryByDisplayValue(/wlt_k/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Copy link" }));
    expect(writeText).toHaveBeenCalledWith("https://team.example.com/join#wlt_k");
  });
});

describe("TeamPage on a member's computer", () => {
  const joined = [
    share("Product", { host: "https://team.example.com", team: { id: "t1", name: "Acme" }, members: [{ id: "me", name: "Dee" }, { id: "a", name: "Amy" }] }),
    share("Sales", { host: "https://team.example.com", team: { id: "t1", name: "Acme" }, read_only: true, members: [{ id: "me", name: "Dee" }] }),
  ];

  it("shows the same page by the team's name, with no way to invite", async () => {
    vi.mocked(listShares).mockResolvedValue(joined);
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    expect(await screen.findByRole("heading", { name: "Acme" })).toBeInTheDocument();
    expect(screen.getByText("Dee (you)")).toBeInTheDocument();
    expect(screen.getByText("Amy")).toBeInTheDocument();
    expect(within(screen.getByRole("button", { name: "Sales" }).closest("li")!).getByText("Read only")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Invite someone" })).not.toBeInTheDocument();
  });

  it("leaves the team: every Space of that server stops syncing here", async () => {
    vi.mocked(listShares).mockResolvedValue(joined);
    vi.mocked(confirmAction).mockResolvedValue(true);
    vi.mocked(stopSharingSpace).mockResolvedValue();
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: "More for Acme" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Leave Acme" }));
    await waitFor(() => expect(stopSharingSpace).toHaveBeenCalledTimes(2));
    expect(stopSharingSpace).toHaveBeenCalledWith("Product");
    expect(stopSharingSpace).toHaveBeenCalledWith("Sales");
  });
});
