// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../i18n";
import type { Space } from "../lib/tauri";
import { SpaceDossierHeader } from "../components/memory/space-detail/SpaceDossierHeader";
import { SPACE_DETAIL_TEST_COPY } from "../components/memory/space-detail/testTranslation";
import { resetTeamMeForTests, TEAM_KEY_STORAGE } from "./team";
import { TeamAccessProvider, teamAccessFor } from "./teamAccess";
import { TeamMemberApp } from "./TeamMemberApp";

const space: Space = {
  id: "space-1",
  name: "Field Notes",
  description: "Trail notes",
  suggested: false,
  starred: false,
  sort_order: 0,
  memory_count: 2,
  entity_count: 0,
  created_at: 1_000,
  updated_at: 2_000,
};

function header(access: "read" | "write") {
  const actions = { onBack: vi.fn(), onCreatePage: vi.fn(), onDelete: vi.fn(), onKeep: vi.fn(), onSaveIdentity: vi.fn() };
  return render(
    <TeamAccessProvider value={teamAccessFor({ member_id: "m", name: "Ari", spaces: [{ space: space.name, access }] })}>
      <SpaceDossierHeader actions={actions} copy={SPACE_DETAIL_TEST_COPY} mutationError={false} space={space} />
    </TeamAccessProvider>,
  );
}

describe("team member controls", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.history.pushState({}, "", "/app");
    window.localStorage.clear();
    resetTeamMeForTests();
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    window.localStorage.clear();
    window.history.pushState({}, "", "/");
  });

  it("a reader's Space has no new page, no actions and a fixed name", () => {
    const { container } = header("read");
    expect(container.querySelector(".space-dossier-new-page")).toBeNull();
    expect(container.querySelector(".space-dossier-overflow-trigger")).toBeNull();
    expect(container.querySelectorAll("[contenteditable=true], input:not([readonly]), textarea:not([readonly])")).toHaveLength(0);
  });

  it("a writer's Space has new page but still no Space actions", () => {
    const { container } = header("write");
    expect(container.querySelector(".space-dossier-new-page")).not.toBeNull();
    expect(container.querySelector(".space-dossier-overflow-trigger")).toBeNull();
  });

  it("outside team mode the same header is editable (control for the reader test)", () => {
    vi.stubGlobal("__WENLAN_WEB__", false);
    const actions = { onBack: vi.fn(), onCreatePage: vi.fn(), onDelete: vi.fn(), onKeep: vi.fn(), onSaveIdentity: vi.fn() };
    const { container } = render(<SpaceDossierHeader actions={actions} copy={SPACE_DETAIL_TEST_COPY} mutationError={false} space={space} />);
    expect(container.querySelector(".space-dossier-overflow-trigger")).not.toBeNull();
    expect(container.querySelectorAll("[contenteditable=true], input:not([readonly]), textarea:not([readonly])").length).toBeGreaterThan(0);
  });

  /** A server whose sign-in is off (or older): no session, no providers. */
  const signedOut = (providers: unknown = { providers: [] }) =>
    vi.fn(async (url: string) => (url === "/auth/providers" ? new Response(JSON.stringify(providers)) : new Response("", { status: 401 })));

  it("with no key and sign-in off, /app shows one sentence", async () => {
    vi.stubGlobal("fetch", signedOut());
    render(<TeamMemberApp><p>inside</p></TeamMemberApp>);
    await waitFor(() => expect(screen.getByTestId("team-web-message").textContent).toBe(i18n.t("teamWeb.openInvite")));
    expect(screen.queryByText("inside")).toBeNull();
    expect(screen.queryByTestId("team-web-signin")).toBeNull();
  });

  it("with no key and sign-in on, /app shows the sign-in page with a button for each provider", async () => {
    vi.stubGlobal("fetch", signedOut({ providers: ["google", "github"], team: "Acme" }));
    render(<TeamMemberApp><p>inside</p></TeamMemberApp>);
    expect(await screen.findByRole("heading", { name: "Sign in to Acme" })).toBeInTheDocument();
    expect(screen.getByText("Use the account you joined with.")).toBeInTheDocument();
    expect(screen.getByText("New here? Open the invite link you were sent.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sign in with Google" })).toHaveAttribute("href", "/auth/start/google?purpose=signin");
    expect(screen.getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute("href", "/auth/start/github?purpose=signin");
    expect(screen.queryByText("inside")).toBeNull();
    expect(screen.queryByTestId("team-web-message")).toBeNull();
  });

  it("offers only the providers that are on", async () => {
    vi.stubGlobal("fetch", signedOut({ providers: ["github"], team: "Acme" }));
    render(<TeamMemberApp><p>inside</p></TeamMemberApp>);
    await screen.findByTestId("team-web-signin");
    expect(screen.queryByRole("link", { name: "Sign in with Google" })).toBeNull();
    expect(screen.getByRole("link", { name: "Sign in with GitHub" })).toBeInTheDocument();
  });

  it("a signed-in browser session opens the app without a key", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ member_id: "m", name: "Dana", spaces: [], signed_in_with: "browser" }))));
    render(<TeamMemberApp><p>inside</p></TeamMemberApp>);
    await screen.findByText("inside");
  });

  it("a refused key shows the expired sentence", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "old");
    vi.stubGlobal("fetch", vi.fn(async () => new Response("", { status: 401 })));
    render(<TeamMemberApp><p>inside</p></TeamMemberApp>);
    await waitFor(() => expect(screen.getByTestId("team-web-message").textContent).toBe(i18n.t("teamWeb.expired")));
    expect(screen.queryByTestId("team-web-signin")).toBeNull();
  });

  it("a live key opens the app", async () => {
    window.localStorage.setItem(TEAM_KEY_STORAGE, "live");
    // The session is asked first and has nothing; then the key answers.
    const fetch = vi.fn(async (_url: string, init: RequestInit) => (
      (init.headers as Record<string, string> | undefined)?.Authorization === "Bearer live"
        ? new Response(JSON.stringify({ member_id: "m", name: "Ari", spaces: [] }))
        : new Response("", { status: 401 })
    ));
    vi.stubGlobal("fetch", fetch);
    render(<TeamMemberApp><p>inside</p></TeamMemberApp>);
    await screen.findByText("inside");
    expect(fetch).toHaveBeenCalledWith("/team/v1/me", expect.objectContaining({ headers: { Authorization: "Bearer live" } }));
    expect(window.localStorage.getItem(TEAM_KEY_STORAGE)).toBe("live");
  });
});
