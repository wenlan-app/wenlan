// SPDX-License-Identifier: AGPL-3.0-only
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { teamGroups } from "./TeamPage";
import { IssuedCode } from "../sharing/TeamMembers";
import { ScopeProvider, useScopeState } from "./scope";
import { ScopeSwitcher } from "./ScopeSwitcher";
import { JoinedTeamLibraryGroups } from "./joinedLibrary";
import {
  listShares,
  listSpaces,
  stopSharingSpace,
  teamAddMember,
  teamApproveMember,
  teamInvite,
  teamMemberCompromised,
  teamNewInviteLink,
  teamNewMemberKey,
  teamRename,
  teamStatus,
  teamJoinedLibrary,
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
    teamInvite: vi.fn(),
    teamNewInviteLink: vi.fn(),
    teamNewMemberKey: vi.fn(),
    teamApproveMember: vi.fn(),
    teamMemberCompromised: vi.fn(),
    teamUpdateMember: vi.fn(),
    teamSetEveryone: vi.fn(),
    teamRename: vi.fn(),
    teamHostSpace: vi.fn(),
    stopSharingSpace: vi.fn(),
    teamJoinedLibrary: vi.fn(),
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
  vi.mocked(teamJoinedLibrary).mockResolvedValue({ teams: [] });
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
      <JoinedTeamLibraryGroups />
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

describe("IssuedCode browser setup hint", () => {
  it.each([
    ["en", "No Wenlan app? Open the link in a browser to connect an AI tool."],
    ["zh-Hans", "没有文澜应用？在浏览器中打开链接，即可改为连接 AI 工具。"],
    ["zh-Hant", "沒有文瀾應用程式？在瀏覽器中打開連結，即可改為連接 AI 工具。"],
  ])("renders in %s without printing the invite link", async (language, hint) => {
    await i18n.changeLanguage(language);
    const code = "https://team.example.com/join#wlt_secret";
    wrap(<IssuedCode name="Cy" code={code} onDone={() => undefined} />);
    expect(screen.getByText(hint)).toBeInTheDocument();
    expect(screen.queryByDisplayValue(code)).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("team.example.com");
    expect(document.body.textContent).not.toContain("wlt_secret");
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

  it("removes the departed team's live library without refetching it", async () => {
    vi.mocked(confirmAction).mockResolvedValue(true);
    vi.mocked(stopSharingSpace).mockResolvedValue();
    vi.mocked(teamJoinedLibrary).mockResolvedValue({ teams: [{ team: { id: "t1", name: "Acme" }, library: { state: "ready", team_pages: [], spaces: [{ space: { id: "s1", name: "Roadmap", restricted: false }, pages: [] }] } }] });
    wrap(<Shell />);
    expect(await screen.findByTestId("team-joined-library")).toBeInTheDocument();
    await userEvent.click(within(await openMenu("Acme")).getByRole("menuitem", { name: "Leave Acme…" }));
    await waitFor(() => expect(screen.queryByTestId("team-joined-library")).not.toBeInTheDocument());
    expect(teamJoinedLibrary).toHaveBeenCalledTimes(1);
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

describe("how each person signs in, in the people box of the owner", () => {
  const signedIn: TeamStatus = {
    listening: "100.70.1.2:7879",
    url: "https://team.example.com",
    sign_in: true,
    spaces: ["Product"],
    members: [
      { ...person("b", "Ben", []) },
      { ...person("c", "Chen Wei", []), sign_in: { providers: ["google"] } },
      { ...person("d", "Dana", []), sign_in: { providers: ["github"] } },
      { ...person("e", "Eli", []), sign_in: { waiting_email: "e***@studio.io" } },
      { ...person("f", "Farah", []), sign_in: { invited: true } },
    ],
  };
  beforeEach(() => {
    vi.mocked(listShares).mockResolvedValue([share("Product", { hosted: true })]);
    vi.mocked(teamStatus).mockResolvedValue(signedIn);
    localStorage.setItem("wenlan-scope", "hosted");
    localStorage.setItem("wenlan-team-url", "https://team.example.com");
  });
  const peopleBox = async () => {
    wrap(<Shell />);
    await userEvent.click(within(await openMenu("Team")).getByRole("menuitem", { name: "People in Team…" }));
    const box = await screen.findByRole("dialog");
    await within(box).findByText("Eli");
    return box;
  };
  const row = (box: HTMLElement, name: string) => within(box).getByText(name).closest("li") as HTMLElement;

  it("tags each person: Google, GitHub, invite link, invited and not joined, or waiting with a masked email", async () => {
    const box = await peopleBox();
    const tags = Object.fromEntries(["Ben", "Chen Wei", "Dana", "Eli", "Farah"].map((name) => [name, within(row(box, name)).getByTestId("team-member-tag").textContent]));
    expect(tags).toEqual({
      Ben: "Invite link · read only",
      "Chen Wei": "Google",
      Dana: "GitHub",
      Eli: "Waiting for you · e•••@studio.io",
      Farah: "Invited · not joined yet",
    });
    // Never a whole address on screen.
    expect(box.textContent).not.toContain("eli@");
    // The owner's own row has no tag.
    expect(within(row(box, "Dee (you) · Owner")).queryByTestId("team-member-tag")).toBeNull();
  });

  it("only the person waiting has a Let in button, and it approves them", async () => {
    vi.mocked(teamApproveMember).mockResolvedValue(person("e", "Eli", []));
    const box = await peopleBox();
    expect(within(box).getAllByRole("button", { name: /^Let in/ })).toHaveLength(1);
    await userEvent.click(within(row(box, "Eli")).getByRole("button", { name: "Let in Eli" }));
    await waitFor(() => expect(teamApproveMember).toHaveBeenCalledWith("e"));
  });

  it("the menu keeps New invite link and Remove from team, and adds Account taken over with its sub line", async () => {
    const box = await peopleBox();
    await userEvent.click(within(row(box, "Ben")).getByRole("button", { name: "More for Ben" }));
    const items = screen.getAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual([
      "New invite link",
      "Account taken over…Ends every sign-in and AI tool key, unlinks Google and GitHub",
      "Remove from team",
    ]);
    expect(screen.getByRole("menuitem", { name: /Remove from team/ })).toHaveAttribute("data-danger", "true");
    // After the separator, the last item.
    expect(screen.getByRole("separator")).toBeInTheDocument();
  });

  it("the menu is placed in the window, so the box's own edge cannot clip it", async () => {
    const box = await peopleBox();
    await userEvent.click(within(row(box, "Ben")).getByRole("button", { name: "More for Ben" }));
    const list = screen.getByRole("menu");
    // Fixed to the window by the stylesheet, from where the button is.
    expect(readFileSync(resolve("src/components/memory/sharing/sharing.css"), "utf8")).toMatch(/\.team-menu-list \{ position:fixed;/);
    expect(list.style.top !== "" || list.style.bottom !== "").toBe(true);
    expect(list.style.right).not.toBe("");
  });

  it("Account taken over asks first, and does nothing when cancelled", async () => {
    vi.mocked(confirmAction).mockResolvedValue(false);
    const box = await peopleBox();
    await userEvent.click(within(row(box, "Dana")).getByRole("button", { name: "More for Dana" }));
    await userEvent.click(screen.getByRole("menuitem", { name: /Account taken over/ }));
    await waitFor(() => expect(confirmAction).toHaveBeenCalled());
    const [message, labels] = vi.mocked(confirmAction).mock.calls[0];
    expect(message).toContain("Dana");
    expect(message).toContain("Google and GitHub are unlinked");
    expect(labels.confirm).toBe("End all access");
    expect(teamMemberCompromised).not.toHaveBeenCalled();
  });

  it("Account taken over, once confirmed, resets and shows the new link once", async () => {
    vi.mocked(confirmAction).mockResolvedValue(true);
    vi.mocked(teamMemberCompromised).mockResolvedValue({ member: person("d", "Dana", []), key: "wlt_new", join_code: "https://team.example.com/join#wlt_new" });
    const box = await peopleBox();
    await userEvent.click(within(row(box, "Dana")).getByRole("button", { name: "More for Dana" }));
    await userEvent.click(screen.getByRole("menuitem", { name: /Account taken over/ }));
    await waitFor(() => expect(teamMemberCompromised).toHaveBeenCalledWith("d", "https://team.example.com"));
    expect(await screen.findByText("Invite link for Dana")).toBeInTheDocument();
  });

  it("New invite link for someone who signs in is an invite, not a key", async () => {
    vi.mocked(teamNewInviteLink).mockResolvedValue({ member: person("f", "Farah", []), link: "https://team.example.com/join#inv_x" });
    const box = await peopleBox();
    await userEvent.click(within(row(box, "Farah")).getByRole("button", { name: "More for Farah" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "New invite link" }));
    await waitFor(() => expect(teamNewInviteLink).toHaveBeenCalledWith("f"));
    expect(teamNewMemberKey).not.toHaveBeenCalled();
    // The copy does not call an invite a key.
    expect(await screen.findByText(/signs in with Google or GitHub. There is no key to keep./)).toBeInTheDocument();
    expect(screen.queryByText(/No Wenlan app\?/)).toBeNull();
  });

  it("New invite link for someone with only a key is a new key, as before", async () => {
    vi.mocked(teamNewMemberKey).mockResolvedValue({ member: person("b", "Ben", []), key: "wlt_k", join_code: "https://team.example.com/join#wlt_k" });
    const box = await peopleBox();
    await userEvent.click(within(row(box, "Ben")).getByRole("button", { name: "More for Ben" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "New invite link" }));
    await waitFor(() => expect(teamNewMemberKey).toHaveBeenCalledWith("b", "https://team.example.com"));
    expect(teamNewInviteLink).not.toHaveBeenCalled();
  });
});

describe("the invite form of the owner", () => {
  const status = (signIn: boolean | undefined): TeamStatus => ({
    listening: "100.70.1.2:7879", url: "https://team.example.com", sign_in: signIn, spaces: ["Product"], members: [person("a", "Amy", [])],
  });
  const openInvite = async () => {
    wrap(<Shell />);
    await userEvent.click(within(await openMenu("Team")).getByRole("menuitem", { name: "Invite someone…" }));
  };
  beforeEach(() => {
    vi.mocked(listShares).mockResolvedValue([share("Product", { hosted: true })]);
    localStorage.setItem("wenlan-scope", "hosted");
    localStorage.setItem("wenlan-team-url", "https://team.example.com");
  });

  it("with sign-in on: name, optional email with its help line, and the choice with Google or GitHub first", async () => {
    vi.mocked(teamStatus).mockResolvedValue(status(true));
    await openInvite();
    const box = await screen.findByRole("dialog");
    expect(within(box).getByRole("heading", { name: "Invite someone to Team" })).toBeInTheDocument();
    expect(within(box).getByLabelText("Name")).toBeInTheDocument();
    expect(within(box).getByLabelText("Email (optional)")).toBeInTheDocument();
    expect(within(box).getByText("Only an account with this email can use the link. Leave it empty and anyone with the link can join.")).toBeInTheDocument();
    expect(within(box).getByText("They join with")).toBeInTheDocument();
    const account = within(box).getByRole("radio", { name: /^Google or GitHub/ });
    const link = within(box).getByRole("radio", { name: /^The link only/ });
    expect(account).toBeChecked();
    expect(link).not.toBeChecked();
    expect(within(box).getByText("They sign in with their own account. No key to keep.")).toBeInTheDocument();
    expect(within(box).getByText("For someone without Google or GitHub. The link is their key. They can read, and write once they add Google or GitHub.")).toBeInTheDocument();
    // The form is the box's whole view.
    expect(within(box).queryByText("Amy")).toBeNull();
  });

  it("Google or GitHub makes an invite with the email, and its card does not call the link a key", async () => {
    vi.mocked(teamStatus).mockResolvedValue(status(true));
    vi.mocked(teamInvite).mockResolvedValue({ member: person("f", "Farah", []), link: "https://team.example.com/join#inv_x" });
    await openInvite();
    const box = await screen.findByRole("dialog");
    await userEvent.type(within(box).getByLabelText("Name"), "Farah");
    await userEvent.type(within(box).getByLabelText("Email (optional)"), "farah@gmail.com");
    await userEvent.click(within(box).getByRole("button", { name: "Create invite link" }));
    await waitFor(() => expect(teamInvite).toHaveBeenCalledWith("Farah", "farah@gmail.com"));
    expect(teamAddMember).not.toHaveBeenCalled();
    expect(await screen.findByText("Invite link for Farah")).toBeInTheDocument();
    expect(screen.getByText(/signs in with Google or GitHub. There is no key to keep./)).toBeInTheDocument();
    expect(screen.queryByText(/gets every Space/)).toBeNull();
  });

  it("an empty email is not sent", async () => {
    vi.mocked(teamStatus).mockResolvedValue(status(true));
    vi.mocked(teamInvite).mockResolvedValue({ member: person("f", "Farah", []), link: "https://team.example.com/join#inv_x" });
    await openInvite();
    const box = await screen.findByRole("dialog");
    await userEvent.type(within(box).getByLabelText("Name"), "Farah");
    await userEvent.click(within(box).getByRole("button", { name: "Create invite link" }));
    await waitFor(() => expect(teamInvite).toHaveBeenCalledWith("Farah", undefined));
  });

  it("The link only stays today's key link", async () => {
    vi.mocked(teamStatus).mockResolvedValue(status(true));
    vi.mocked(teamAddMember).mockResolvedValue({ member: person("c", "Cy", []), key: "wlt_k", join_code: "https://team.example.com/join#wlt_k" });
    await openInvite();
    const box = await screen.findByRole("dialog");
    await userEvent.type(within(box).getByLabelText("Name"), "Cy");
    await userEvent.click(within(box).getByRole("radio", { name: /^The link only/ }));
    await userEvent.click(within(box).getByRole("button", { name: "Create invite link" }));
    await waitFor(() => expect(teamAddMember).toHaveBeenCalledWith("Cy", [], "https://team.example.com"));
    expect(teamInvite).not.toHaveBeenCalled();
    expect(await screen.findByText(/gets every Space open to everyone/)).toBeInTheDocument();
  });

  it.each([[false], [undefined]])("with sign-in off (%s) there is no email field and no choice, as today", async (signIn) => {
    vi.mocked(teamStatus).mockResolvedValue(status(signIn));
    await openInvite();
    const box = await screen.findByRole("dialog");
    expect(within(box).getByLabelText("Name")).toBeInTheDocument();
    expect(within(box).queryByLabelText("Email (optional)")).toBeNull();
    expect(within(box).queryByText("They join with")).toBeNull();
    expect(within(box).queryByRole("radio")).toBeNull();
    expect(within(box).getByRole("button", { name: "Create invite link" })).toBeInTheDocument();
    expect(within(box).getByText("Amy")).toBeInTheDocument();
  });

  it("Cancel goes back to the people", async () => {
    vi.mocked(teamStatus).mockResolvedValue(status(true));
    await openInvite();
    const box = await screen.findByRole("dialog");
    await userEvent.click(within(box).getByRole("button", { name: "Cancel" }));
    expect(await within(box).findByText("Amy")).toBeInTheDocument();
  });
});
