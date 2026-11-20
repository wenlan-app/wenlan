// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setConfirmPresenter } from "../lib/confirmAction";
import { i18n } from "../i18n";
import { resetTeamMeForTests, TEAM_KEY_STORAGE } from "./team";
import { TeamAccountMenu } from "./TeamAccountMenu";

const COOKIE_ME = {
  member_id: "m1", name: "Dana", spaces: [], signed_in_with: "browser",
  accounts: [{ provider: "google", email: "dana@gmail.com" }],
};
const KEY_ME = { member_id: "m2", name: "Ben", spaces: [], signed_in_with: "key" };

type Answer = { me: object | null; providers?: string[] };

/** A team server: who the cookie or key is, which providers are on. */
function server({ me, providers = ["google", "github"] }: Answer) {
  const fetch = vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/team/v1/me") {
      const bearer = (init?.headers as Record<string, string> | undefined)?.Authorization;
      const byKey = Boolean(bearer);
      const isKeyMe = (me as { signed_in_with?: string } | null)?.signed_in_with === "key";
      return me && byKey === isKeyMe ? new Response(JSON.stringify(me)) : new Response("", { status: 401 });
    }
    if (url === "/auth/providers") return new Response(JSON.stringify({ providers, team: "Acme" }));
    if (url.startsWith("/auth/link/")) return new Response(JSON.stringify({ url: "https://provider.example.com/go" }));
    return new Response(null, { status: 204 });
  });
  vi.stubGlobal("fetch", fetch);
  return fetch;
}

const mount = () => render(<TeamAccountMenu className="notes-primary-link" glyphClassName="notes-navigation-glyph" />);
const writes = (fetch: ReturnType<typeof vi.fn>) =>
  (fetch.mock.calls as unknown as [string, RequestInit | undefined][]).filter(([, init]) => init?.method === "POST").map(([url]) => url);

describe("the account menu at the bottom of the web sidebar", () => {
  const assign = vi.fn();
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.history.pushState({}, "", "/app");
    window.localStorage.clear();
    resetTeamMeForTests();
    assign.mockReset();
    vi.spyOn(window, "location", "get").mockReturnValue({ ...window.location, assign } as Location);
  });
  afterEach(() => {
    setConfirmPresenter();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    window.localStorage.clear();
    window.history.pushState({}, "", "/");
  });

  it("shows the avatar letter and the name, and nothing until it knows who it is", async () => {
    server({ me: null });
    const { container } = mount();
    await waitFor(() => expect(globalThis.fetch).toHaveBeenCalled());
    expect(container.querySelector("button")).toBeNull();

    resetTeamMeForTests();
    server({ me: COOKIE_ME });
    const second = mount();
    const row = await within(second.container).findByTestId("team-account-row");
    expect(row).toHaveTextContent("D");
    expect(row).toHaveTextContent("Dana");
    expect(row).toHaveAccessibleName("Account menu, Dana");
  });

  it("with a browser session: name, the provider and email, Sign out, and a quiet everywhere line", async () => {
    const fetch = server({ me: COOKIE_ME });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    const menu = screen.getByTestId("team-account-menu");
    expect(within(menu).getByText("Dana", { selector: "b" })).toBeInTheDocument();
    expect(within(menu).getByTestId("team-account-email")).toHaveTextContent("dana@gmail.com");
    expect(within(menu).queryByText("Sign in from any device")).toBeNull();
    expect(within(menu).queryByText("Joined with an invite link")).toBeNull();
    expect(within(menu).getByText(/Signed in somewhere you don't recognize\?/)).toBeInTheDocument();
    // The menu is in the viewport, not inside the sidebar that may clip it.
    expect(menu.parentElement).toBe(document.body);

    await userEvent.click(within(menu).getByRole("menuitem", { name: "Sign out" }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("/app"));
    expect(writes(fetch)).toEqual(["/auth/logout"]);
  });

  it("Sign out everywhere asks first, says AI tool keys are unaffected, and does nothing on cancel", async () => {
    const fetch = server({ me: COOKIE_ME });
    const ask = vi.fn(async () => false);
    setConfirmPresenter(ask);
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    await userEvent.click(screen.getByTestId("team-account-everywhere"));
    await waitFor(() => expect(ask).toHaveBeenCalledTimes(1));
    const [message, labels] = ask.mock.calls[0] as unknown as [string, { confirm: string }];
    expect(message).toContain("AI tool keys keep working");
    expect(labels.confirm).toBe("Sign out everywhere");
    expect(writes(fetch)).toEqual([]);
    expect(assign).not.toHaveBeenCalled();
  });

  it("Sign out everywhere ends every session once confirmed", async () => {
    const fetch = server({ me: COOKIE_ME });
    setConfirmPresenter(async () => true);
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    await userEvent.click(screen.getByTestId("team-account-everywhere"));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("/app"));
    expect(writes(fetch)).toEqual(["/auth/logout-all"]);
  });

  it("with a key: Joined with an invite link, a card to add Google or GitHub, and a sign out that needs the link to come back", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-1");
    server({ me: KEY_ME });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    const menu = screen.getByTestId("team-account-menu");
    expect(within(menu).getByText("Joined with an invite link")).toBeInTheDocument();
    const card = await within(menu).findByTestId("team-account-link-card");
    expect(within(card).getByText("Sign in from any device")).toBeInTheDocument();
    expect(within(card).getByText(/Right now only this browser knows you/)).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Continue with Google" })).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Continue with GitHub" })).toBeInTheDocument();
    expect(within(menu).queryByText("Sign out everywhere")).toBeNull();
    const forget = within(menu).getByTestId("team-web-forget");
    expect(forget).toHaveTextContent("Sign out of this browser");
    expect(forget).toHaveTextContent("To come back you'll need your invite link");
  });

  it("with a key, Continue starts the Link with the key and follows the provider address", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-1");
    const fetch = server({ me: KEY_ME });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    await userEvent.click(await screen.findByRole("button", { name: "Continue with GitHub" }));
    await waitFor(() => expect(assign).toHaveBeenCalledWith("https://provider.example.com/go"));
    expect(fetch).toHaveBeenCalledWith("/auth/link/github", expect.objectContaining({ method: "POST", headers: { Authorization: "Bearer key-1" } }));
    // The key is dropped only after the new session has worked, not here.
    expect(window.localStorage.getItem(TEAM_KEY_STORAGE)).toBe("key-1");
  });

  it("shows a quiet failure when linking cannot start", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-1");
    const fetch = server({ me: KEY_ME });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    const button = await screen.findByRole("button", { name: "Continue with Google" });
    fetch.mockImplementation(async (url: string) => (url.startsWith("/auth/link/") ? new Response("", { status: 403 }) : new Response("{}")));
    await userEvent.click(button);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not start sign-in");
    expect(assign).not.toHaveBeenCalled();
  });

  it("with a key and sign-in off on the server, there is no card", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-1");
    server({ me: KEY_ME, providers: [] });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    const menu = screen.getByTestId("team-account-menu");
    await waitFor(() => expect(globalThis.fetch).toHaveBeenCalledWith("/auth/providers", expect.anything()));
    expect(within(menu).queryByTestId("team-account-link-card")).toBeNull();
    expect(within(menu).getByTestId("team-web-forget")).toBeInTheDocument();
  });

  it("offers only the provider that is on", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "key-1");
    server({ me: KEY_ME, providers: ["github"] });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    await screen.findByRole("button", { name: "Continue with GitHub" });
    expect(screen.queryByRole("button", { name: "Continue with Google" })).toBeNull();
  });

  it("is the same menu in Chinese", async () => {
    await i18n.changeLanguage("zh-Hant");
    server({ me: COOKIE_ME });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    expect(screen.getByRole("menuitem", { name: "登出" })).toBeInTheDocument();
    expect(screen.getByText("全部登出")).toBeInTheDocument();
  });

  it("closes on Escape", async () => {
    server({ me: COOKIE_ME });
    mount();
    await userEvent.click(await screen.findByTestId("team-account-row"));
    expect(screen.getByTestId("team-account-menu")).toBeInTheDocument();
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByTestId("team-account-menu")).toBeNull();
  });
});
