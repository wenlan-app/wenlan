// SPDX-License-Identifier: AGPL-3.0-only
// A member's AI tools and desktop link from the account menu, and the
// `/app/device` page where an AI tool's short code is allowed.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../i18n";
import { memberTag } from "../components/memory/sharing/TeamMembers";
import type { TeamMember } from "../lib/tauri";
import { DevicePage } from "./DevicePage";
import { AFTER_STORAGE, resetTeamMeForTests, teamMe, type TeamMe } from "./team";
import { TeamAccountMenu } from "./TeamAccountMenu";

const COOKIE_ME = {
  member_id: "m1", name: "Dana", spaces: [{ space: "Product", access: "write" }], signed_in_with: "browser",
  team: { name: "Acme" }, accounts: [{ provider: "google", email: "dana@gmail.com" }],
};
const KEY_ME = { member_id: "m2", name: "Ben", spaces: [], signed_in_with: "key" };

type Route = (init?: RequestInit) => Response;

function server(me: object, routes: Record<string, Route> = {}) {
  const fetch = vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/team/v1/me") {
      const byKey = Boolean((init?.headers as Record<string, string> | undefined)?.Authorization);
      return byKey === ((me as { signed_in_with?: string }).signed_in_with === "key")
        ? new Response(JSON.stringify(me))
        : new Response("", { status: 401 });
    }
    const route = routes[`${init?.method ?? "GET"} ${url}`];
    if (route) return route(init);
    return new Response("{}", { status: 404 });
  });
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const TOOLS = [
  { id: "k1", name: "Claude Code", created_at: "2026-11-21T02:00:00Z" },
  { id: "k2", name: "Codex", created_at: "2026-11-12T02:00:00Z" },
];

describe("a member's own AI tools and desktop link", () => {
  const assign = vi.fn();
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.history.pushState({}, "", "/app");
    window.localStorage.clear();
    window.sessionStorage.clear();
    resetTeamMeForTests();
    assign.mockReset();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign, origin: "https://team.acme.com" } as Location);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.sessionStorage.clear();
    window.history.pushState({}, "", "/");
  });

  const mount = () => render(<TeamAccountMenu className="notes-primary-link" />);

  it("the menu counts the AI tools, and offers the desktop app only to a signed-in account", async () => {
    server(COOKIE_ME, { "GET /team/v1/account/keys": () => json(TOOLS) });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    const tools = screen.getByTestId("team-account-tools");
    await waitFor(() => expect(tools).toHaveTextContent("AI tools2"));
    expect(screen.getByTestId("team-account-desktop")).toHaveTextContent("Open in the desktop app");

    resetTeamMeForTests();
    window.localStorage.setItem("wenlan.team.key", "key-1");
    server(KEY_ME, { "GET /team/v1/account/keys": () => json([]) });
    const second = render(<TeamAccountMenu className="notes-primary-link" />);
    await userEvent.click(await within(second.container).findByTestId("team-account-row"));
    const menus = screen.getAllByTestId("team-account-menu");
    const keyMenu = menus[menus.length - 1];
    expect(within(keyMenu).getByTestId("team-account-tools")).toHaveTextContent("AI tools");
    expect(within(keyMenu).queryByTestId("team-account-desktop")).toBeNull();
  });

  it("lists the tools and removes one", async () => {
    const fetch = server(COOKIE_ME, {
      "GET /team/v1/account/keys": () => json(TOOLS),
      "DELETE /team/v1/account/keys/k1": () => json([TOOLS[1]]),
    });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    await userEvent.click(screen.getByTestId("team-account-tools"));
    const dialog = await screen.findByTestId("team-tools-dialog");
    await waitFor(() => expect(within(dialog).getAllByTestId("team-tool-row")).toHaveLength(2));
    expect(dialog).toHaveTextContent("Each can read and write your Acme pages until you remove it.");
    expect(dialog).toHaveTextContent("wenlan login https://team.acme.com");
    await userEvent.click(within(dialog).getByLabelText("Remove Claude Code"));
    await waitFor(() => expect(within(dialog).getAllByTestId("team-tool-row")).toHaveLength(1));
    expect(fetch).toHaveBeenCalledWith("/team/v1/account/keys/k1", expect.objectContaining({ method: "DELETE", credentials: "same-origin" }));
  });

  it("a desktop link first asks to confirm with Google, then comes back and is shown hidden", async () => {
    server(COOKIE_ME, {
      "POST /team/v1/account/desktop-link": () => json({ error: "reauth_required" }, 403),
      "POST /auth/reauth/google": () => json({ url: "https://accounts.google.com/o" }),
    });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    await userEvent.click(screen.getByTestId("team-account-desktop"));
    await userEvent.click(await screen.findByTestId("team-desktop-confirm"));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("https://accounts.google.com/o"));
    expect(JSON.parse(window.sessionStorage.getItem(AFTER_STORAGE) ?? "{}")).toEqual({ kind: "desktop" });

    // Back on `/app`: the dialog opens by itself and makes the link.
    resetTeamMeForTests();
    server(COOKIE_ME, {
      "POST /team/v1/account/desktop-link": () => json({ link: "https://team.acme.com/join#wlt_" + "a".repeat(64) }),
    });
    const back = render(<TeamAccountMenu className="notes-primary-link" />);
    const link = await within(back.baseElement).findByTestId("team-desktop-link");
    expect(link).toHaveTextContent("https://team.acme.com/join#wlt_••••••••••••");
    expect(link).not.toHaveTextContent("aaaa");
    expect(window.sessionStorage.getItem(AFTER_STORAGE)).toBeNull();
  });

  it("the owner sees how many AI tools a member has beside how they sign in", () => {
    const t = i18n.getFixedT("en");
    const member = (sign_in: TeamMember["sign_in"]) => ({ id: "m", name: "Chen", spaces: [], added_at: "", role: "member", sign_in }) as unknown as TeamMember;
    expect(memberTag(t, member({ providers: ["google"], tools: 2 }))).toBe("Google · 2 AI tools");
    expect(memberTag(t, member({ providers: ["github"], tools: 1 }))).toBe("GitHub · 1 AI tool");
    expect(memberTag(t, member({ providers: ["google"] }))).toBe("Google");
  });
});

describe("the page an AI tool's short code is typed on", () => {
  const assign = vi.fn();
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.history.pushState({}, "", "/app/device");
    window.sessionStorage.clear();
    resetTeamMeForTests();
    assign.mockReset();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.sessionStorage.clear();
    window.history.pushState({}, "", "/");
  });

  const me = async (): Promise<TeamMe> => (await teamMe())!;

  it("takes the code, shows who asks and what it can use, and allows it", async () => {
    const fetch = server(COOKIE_ME, {
      "POST /auth/device/lookup": () => json({ tool: "Claude Code", spaces: [{ space: "Product", access: "write" }, { space: "Ops", access: "read" }] }),
      "POST /auth/device/approve": () => new Response(null, { status: 204 }),
    });
    render(<DevicePage me={await me()} />);
    const input = screen.getByTestId("team-device-input");
    expect(screen.getByTestId("team-device-continue")).toBeDisabled();
    // Typed loosely: lower case, spaces, an ambiguous letter left out.
    await userEvent.type(input, "kxrt 7pqm");
    expect(input).toHaveValue("KXRT-7PQM");
    await userEvent.click(screen.getByTestId("team-device-continue"));
    expect(await screen.findByText("Let Claude Code use Acme?")).toBeInTheDocument();
    expect(screen.getByTestId("team-device-spaces")).toHaveTextContent("Product");
    expect(screen.getByTestId("team-device-spaces")).toHaveTextContent("Ops (read only)");
    expect(screen.getByText("You'll confirm with Google first.")).toBeInTheDocument();
    await userEvent.click(screen.getByTestId("team-device-allow"));
    expect(await screen.findByTestId("team-device-done")).toHaveTextContent("Claude Code is connected");
    const bodies = (fetch.mock.calls as unknown as [string, RequestInit | undefined][])
      .filter(([url]) => url.startsWith("/auth/device/"))
      .map(([, init]) => JSON.parse(String(init?.body)));
    expect(bodies).toEqual([{ user_code: "KXRT7PQM" }, { user_code: "KXRT7PQM" }]);
  });

  it("a wrong code says so; allowing without a fresh sign-in goes to Google and keeps the code", async () => {
    let known = false;
    server(COOKIE_ME, {
      "POST /auth/device/lookup": () => (known ? json({ tool: "Codex", spaces: [] }) : json({ error: "unknown_code" }, 404)),
      "POST /auth/device/approve": () => json({ error: "reauth_required" }, 403),
      "POST /auth/reauth/google": () => json({ url: "https://accounts.google.com/o" }),
    });
    render(<DevicePage me={await me()} />);
    await userEvent.type(screen.getByTestId("team-device-input"), "AAAABBBB");
    await userEvent.click(screen.getByTestId("team-device-continue"));
    expect(await screen.findByRole("alert")).toHaveTextContent("That code doesn't match");
    known = true;
    await userEvent.click(screen.getByTestId("team-device-continue"));
    await userEvent.click(await screen.findByTestId("team-device-allow"));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("https://accounts.google.com/o"));
    expect(JSON.parse(window.sessionStorage.getItem(AFTER_STORAGE) ?? "{}")).toEqual({ kind: "device", code: "AAAABBBB" });
  });

  it("back with the code, it goes straight to the question", async () => {
    server(COOKIE_ME, { "POST /auth/device/lookup": () => json({ tool: "Codex", spaces: [] }) });
    render(<DevicePage me={await me()} initialCode="AAAABBBB" />);
    expect(await screen.findByText("Let Codex use Acme?")).toBeInTheDocument();
  });
});
