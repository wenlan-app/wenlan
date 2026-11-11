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
  teamSetEveryone,
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
  vi.mocked(listSpaces).mockResolvedValue([]);
  vi.mocked(teamStatus).mockRejectedValue(new Error("off"));
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("teamGroups", () => {
  it("is empty for a computer in no team, so the navigation shows no Team", () => {
    expect(teamGroups([share("Wedding", { host: null })], undefined)).toEqual([]);
  });

  it("has the hosted team first, then one per server", () => {
    const groups = teamGroups(
      [share("Sales", { host: "https://team.example.com/" }), share("Product", { hosted: true })],
      hostedStatus,
    );
    expect(groups.map((group) => group.host)).toEqual([null, "team.example.com"]);
  });
});

describe("TeamPage on the computer that hosts the team", () => {
  beforeEach(() => {
    vi.mocked(listShares).mockResolvedValue([share("Product", { hosted: true }), share("Roadmap", { hosted: true })]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
  });

  it("lists the members with how many Spaces each can use, and who can use each Space", async () => {
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    const amy = (await screen.findByText("Amy", { selector: ".team-page-name" })).closest("li")!;
    expect(within(amy).getByText("2 Spaces")).toBeInTheDocument();
    expect(within(screen.getByText("Bo", { selector: ".team-page-name" }).closest("li")!).getByText("1 Space")).toBeInTheDocument();
    expect(screen.getByLabelText("Who can use Product")).toHaveValue("write");
    expect(screen.getByLabelText("Who can use Roadmap")).toHaveValue("nobody");
  });

  it("opens a Space to everyone on the team", async () => {
    vi.mocked(teamSetEveryone).mockResolvedValue();
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    await userEvent.selectOptions(await screen.findByLabelText("Who can use Roadmap"), "read");
    await waitFor(() => expect(teamSetEveryone).toHaveBeenCalledWith("Roadmap", "read"));
  });

  it("invites someone with the Spaces ticked; a Space for everyone needs no tick", async () => {
    localStorage.setItem("wenlan-team-url", "https://team.example.com");
    vi.mocked(teamAddMember).mockResolvedValue({ member: person("c", "Cy", []), key: "wlt_k", join_code: "https://team.example.com#wlt_k" });
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: "Invite someone" }));
    await userEvent.type(screen.getByLabelText("Name"), "Cy");
    const product = screen.getByRole("checkbox", { name: "Product" });
    expect(product).toBeChecked();
    expect(product).toBeDisabled();
    await userEvent.click(screen.getByRole("checkbox", { name: "Roadmap" }));
    await userEvent.click(screen.getByRole("button", { name: "Invite" }));
    await waitFor(() => expect(teamAddMember).toHaveBeenCalledWith("Cy", [{ space: "Roadmap", access: "write" }], "https://team.example.com"));
    expect(await screen.findByDisplayValue("https://team.example.com#wlt_k")).toHaveAttribute("readonly");
  });
});

describe("TeamPage on a member's computer", () => {
  const joined = [
    share("Product", { host: "https://team.example.com", members: [{ id: "me", name: "Dee" }, { id: "a", name: "Amy" }] }),
    share("Sales", { host: "https://team.example.com", read_only: true, members: [{ id: "me", name: "Dee" }] }),
  ];

  it("shows who is on the team and their own Spaces, with no way to invite", async () => {
    vi.mocked(listShares).mockResolvedValue(joined);
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    expect(await screen.findByRole("heading", { name: "Team · team.example.com" })).toBeInTheDocument();
    expect(within(screen.getByText("Dee (you)").closest("li")!).getByText("2 Spaces")).toBeInTheDocument();
    expect(within(screen.getByText("Amy").closest("li")!).getByText("1 Space")).toBeInTheDocument();
    expect(within(screen.getByRole("button", { name: "Sales" }).closest("li")!).getByText("Read only")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Invite someone" })).not.toBeInTheDocument();
  });

  it("leaves the team: every Space of that server stops syncing here", async () => {
    vi.mocked(listShares).mockResolvedValue(joined);
    vi.mocked(confirmAction).mockResolvedValue(true);
    vi.mocked(stopSharingSpace).mockResolvedValue();
    wrap(<TeamPage onOpenSpace={() => undefined} onOpenPage={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: "Leave the team" }));
    await waitFor(() => expect(stopSharingSpace).toHaveBeenCalledTimes(2));
    expect(stopSharingSpace).toHaveBeenCalledWith("Product");
    expect(stopSharingSpace).toHaveBeenCalledWith("Sales");
  });
});
