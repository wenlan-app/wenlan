// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup, act, configure, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import PairingApprovalDialog from "./PairingApprovalDialog";
import {
  clearAwaitingConnection, clearPendingPairingCode, setPendingPairingCode, useAwaitingConnection,
  usePendingPairingCode,
} from "../../lib/pairingLink";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
const mocks = vi.hoisted(() => Object.fromEntries([
  "toggleRemoteAccess", "reconnectRemoteAccess", "renewRemoteAccess", "getRemoteAccessStatus",
  "getRemoteAccessProfile", "configureRemoteAccess", "listSpaces", "inspectRemotePairing",
  "lookupRemotePairing", "approveRemotePairing", "denyRemotePairing",
].map((key) => [key, vi.fn()])));
vi.mock("../../lib/tauri", () => mocks);

const SHORT = "ABCD-2345";
const SHORT_NORMAL = "ABCD2345";
const LONG = "a".repeat(64);
const profile = { revision: "r1", space: "review", enabled: true, disconnect_pending: false, credential_expires_at: Date.now() + 60 * 24 * 3600_000 };
const connected = { status: "connected", tunnel_url: null, relay_url: "https://relay.wenlan.app/mcp" };
const claude = {
  pairingId: "p".repeat(64), clientId: "synthetic-client-id", resource: "https://relay.wenlan.app/mcp",
  scopes: ["wenlan:query"], expiresAt: Date.now() + 10 * 60_000,
  clientName: "Claude", redirectHost: "claude.ai", knownClient: true,
};
const stranger = { ...claude, clientName: "Totally Claude", redirectHost: "evil.example", knownClient: false };

function Probe() {
  const awaiting = useAwaitingConnection();
  const code = usePendingPairingCode();
  return <><span data-testid="awaiting">{awaiting === null ? "no" : "yes"}</span><span data-testid="code">{code ?? "none"}</span></>;
}
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const invalidate = vi.spyOn(client, "invalidateQueries");
  return { ...render(<QueryClientProvider client={client}><PairingApprovalDialog currentSpace="review" /><Probe /></QueryClientProvider>), client, invalidate };
}
const buttons = () => screen.getAllByRole("button").map((button) => button.textContent);
// Lets the native reads and any lookup they would start finish, so "never asked" means never.
const settled = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 40)); });

// jsdom accessible-name queries are slow on a busy machine; the first test of a
// file also pays the cold start. 4s still fails ahead of the 5s test timeout.
configure({ asyncUtilTimeout: 4_000 });

beforeEach(() => {
  vi.resetAllMocks();
  mocks.getRemoteAccessStatus.mockResolvedValue(connected);
  mocks.getRemoteAccessProfile.mockResolvedValue(profile);
  mocks.listSpaces.mockResolvedValue([{ id: "s1", name: "review" }, { id: "s2", name: "private" }]);
  mocks.lookupRemotePairing.mockResolvedValue(claude);
  mocks.inspectRemotePairing.mockResolvedValue(claude);
  mocks.approveRemotePairing.mockResolvedValue(undefined);
  mocks.denyRemotePairing.mockResolvedValue(undefined);
  mocks.configureRemoteAccess.mockResolvedValue({ ...profile, enabled: false, revision: "r2" });
  mocks.toggleRemoteAccess.mockResolvedValue({ status: "starting" });
});
afterEach(async () => { cleanup(); vi.useRealTimers(); clearPendingPairingCode(); clearAwaitingConnection(); await i18n.changeLanguage("en"); });

describe("PairingApprovalDialog", () => {
  it("renders nothing until a code is waiting, and asks the native side nothing", () => {
    mount();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(mocks.getRemoteAccessProfile).not.toHaveBeenCalled();
  });

  it("looks a short code up by its code, not by pairing id, and names a known app", async () => {
    setPendingPairingCode(SHORT.toLowerCase().replace("-", " - "));
    mount();
    expect(await screen.findByRole("heading", { name: "Allow Claude to use your library?" })).toBeInTheDocument();
    expect(mocks.lookupRemotePairing).toHaveBeenCalledWith("r1", SHORT_NORMAL);
    expect(mocks.inspectRemotePairing).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toHaveAccessibleName("Allow Claude to use your library?");
    expect(screen.getByText("It can search and read memories in review. Your other Spaces stay private.")).toBeInTheDocument();
    expect(screen.queryByText(/wenlan doesn't recognize/i)).not.toBeInTheDocument();
    // Never the raw client id or a status code in the main dialog.
    expect(screen.queryByText(/synthetic-client-id/)).not.toBeInTheDocument();
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
  });

  it("says plainly when an app would see the whole library", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*" });
    setPendingPairingCode(SHORT);
    mount();
    await screen.findByRole("heading", { name: "Allow Claude to use your library?" });
    expect(screen.getByText("It can search and read your whole library: every Space, plus everything not in a Space.")).toBeInTheDocument();
    expect(screen.queryByText(/\*|other Spaces stay private/)).not.toBeInTheDocument();
  });

  it("inspects a long code from a link by its pairing id", async () => {
    setPendingPairingCode(LONG);
    mount();
    await screen.findByRole("heading", { name: /Allow Claude/ });
    expect(mocks.inspectRemotePairing).toHaveBeenCalledWith("r1", LONG);
    expect(mocks.lookupRemotePairing).not.toHaveBeenCalled();
  });

  it("names an unknown app by where it asks to be sent, never by the name it gave itself", async () => {
    mocks.lookupRemotePairing.mockResolvedValue(stranger);
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("heading", { name: "Allow evil.example to use your library?" })).toBeInTheDocument();
    expect(screen.queryByText(/Totally Claude/)).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /Allow Claude/ })).not.toBeInTheDocument();
    expect(screen.getByRole("note")).toHaveTextContent("Wenlan doesn't recognize this app. Continue only if you started this yourself.");
    expect(screen.getByText("Sends access to evil.example")).toBeInTheDocument();
    // Deny comes first, and an unknown app gets the emphasis on saying no.
    expect(buttons().slice(-2)).toEqual(["Deny", "Allow"]);
  });

  it("names Muse by its own host, without the unknown-app warning", async () => {
    mocks.lookupRemotePairing.mockResolvedValue({ ...claude, clientName: "Muse", redirectHost: "agent.meta.ai", knownClient: true });
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("heading", { name: "Allow Muse to use your library?" })).toBeInTheDocument();
    expect(screen.queryByText(/wenlan doesn't recognize/i)).not.toBeInTheDocument();
  });

  it("does not trust a known flag when the host is not the app's own", async () => {
    mocks.lookupRemotePairing.mockResolvedValue({ ...claude, redirectHost: "claude.ai.evil.example", knownClient: true });
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("heading", { name: "Allow claude.ai.evil.example to use your library?" })).toBeInTheDocument();
    expect(screen.getByRole("note")).toBeInTheDocument();
  });

  it("falls back to 'this app' when an old relay sends no host", async () => {
    mocks.lookupRemotePairing.mockResolvedValue({ pairingId: claude.pairingId, clientId: "c", resource: claude.resource, scopes: claude.scopes, expiresAt: claude.expiresAt });
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("heading", { name: "Allow this app to use your library?" })).toBeInTheDocument();
  });

  it("allows with the exact inspected request, then watches for the connection", async () => {
    setPendingPairingCode(SHORT);
    const { invalidate } = mount();
    fireEvent.click(await screen.findByRole("button", { name: "Allow" }));
    await waitFor(() => expect(mocks.approveRemotePairing).toHaveBeenCalledWith("r1", claude, "query"));
    expect(await screen.findByRole("heading", { name: "Allowed" })).toBeInTheDocument();
    expect(screen.getByText("Claude is connecting. You can go back to it now.")).toBeInTheDocument();
    expect(screen.getByTestId("awaiting")).toHaveTextContent("yes");
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["remote-access-grants"] });
    fireEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByTestId("code")).toHaveTextContent("none");
  });

  it("asks what the app can do, with search only chosen by default", async () => {
    setPendingPairingCode(SHORT);
    mount();
    await screen.findByRole("heading", { name: "Allow Claude to use your library?" });
    const group = screen.getByRole("radiogroup", { name: "What can it do?" });
    const search = within(group).getByRole("radio", { name: "Search only" });
    const write = within(group).getByRole("radio", { name: "Search and write" });
    expect(search).toBeChecked();
    expect(write).not.toBeChecked();
    expect(search).toHaveAccessibleDescription("Search and read. It cannot change anything.");
    expect(write).toHaveAccessibleDescription("It can also add, edit and delete. The AI app asks you before deleting.");
    expect(screen.getByRole("dialog").textContent).not.toMatch(/[—–]| - /);
  });

  it("sends search only when the person allows without touching the choice", async () => {
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Allow" }));
    await waitFor(() => expect(mocks.approveRemotePairing).toHaveBeenCalledWith("r1", claude, "query"));
  });

  it("sends write only after the person chooses it", async () => {
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("radio", { name: "Search and write" }));
    expect(screen.getByRole("radio", { name: "Search and write" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Search only" })).not.toBeChecked();
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(mocks.approveRemotePairing).toHaveBeenCalledWith("r1", claude, "write"));
    expect(await screen.findByRole("heading", { name: "Allowed" })).toBeInTheDocument();
  });

  it("can go back from write to search only before allowing", async () => {
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("radio", { name: "Search and write" }));
    fireEvent.click(screen.getByRole("radio", { name: "Search only" }));
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(mocks.approveRemotePairing).toHaveBeenCalledWith("r1", claude, "query"));
  });

  it("puts both choices in one native radio group that can take focus", async () => {
    setPendingPairingCode(SHORT);
    mount();
    const search = await screen.findByRole("radio", { name: "Search only" });
    const write = screen.getByRole("radio", { name: "Search and write" });
    expect(search).toHaveAttribute("name", write.getAttribute("name"));
    expect(search).not.toBeDisabled();
    // Only the chosen radio is a tab stop; the arrow keys move within the group.
    search.focus();
    expect(document.activeElement).toBe(search);
  });

  it("never allows on its own", async () => {
    setPendingPairingCode(SHORT);
    mount();
    await screen.findByRole("button", { name: "Allow" });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
    expect(mocks.denyRemotePairing).not.toHaveBeenCalled();
  });

  it("denies at the exact pairing and closes", async () => {
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Deny" }));
    await waitFor(() => expect(mocks.denyRemotePairing).toHaveBeenCalledWith("r1", claude.pairingId));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
    expect(screen.getByTestId("awaiting")).toHaveTextContent("no");
  });

  it("treats a deny the older relay does not know as already gone", async () => {
    mocks.denyRemotePairing.mockRejectedValue(new Error("Remote connection rejected (HTTP 404)"));
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Deny" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("keeps the request open when a deny fails for another reason", async () => {
    mocks.denyRemotePairing.mockRejectedValue(new Error("Remote connection unavailable; retry later"));
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Deny" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Can't reach Wenlan's web service. Check your internet connection.");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it.each([["lookup 404", "lookupRemotePairing", "Remote connection rejected (HTTP 404)"], ["lookup 409", "lookupRemotePairing", "Remote connection rejected (HTTP 409)"]])(
    "shows the expired state for a %s", async (_name, fn, message) => {
      mocks[fn].mockRejectedValue(new Error(message));
      setPendingPairingCode(SHORT);
      mount();
      expect(await screen.findByRole("heading", { name: "This request expired. Start connecting again from your AI app." })).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
      expect(screen.queryByText(/404|409|HTTP/)).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Close" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    });

  it("shows the expired state when approval finds the request gone", async () => {
    mocks.approveRemotePairing.mockRejectedValue(new Error("Remote connection rejected (HTTP 409)"));
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Allow" }));
    expect(await screen.findByRole("heading", { name: /request expired/ })).toBeInTheDocument();
    expect(screen.getByTestId("awaiting")).toHaveTextContent("no");
  });

  it("expires on its own clock while it waits for an answer", async () => {
    mocks.lookupRemotePairing.mockResolvedValue({ ...claude, expiresAt: Date.now() + 120 });
    setPendingPairingCode(SHORT);
    mount();
    await screen.findByRole("button", { name: "Allow" });
    expect(await screen.findByRole("heading", { name: /request expired/ })).toBeInTheDocument();
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
  });

  it("does not offer a request that is already past its time", async () => {
    mocks.lookupRemotePairing.mockResolvedValue({ ...claude, expiresAt: 1 });
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("heading", { name: /request expired/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
  });

  it("explains rate limiting and offline in plain words, with the raw sentence only behind Details", async () => {
    mocks.lookupRemotePairing.mockRejectedValueOnce(new Error("Remote connection rate limited"));
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent("Too many tries. Wait a few minutes, then try again.");
    expect(screen.queryByText(/rate limited/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Details" }));
    expect(screen.getByText("Remote connection rate limited")).toBeInTheDocument();
    // Try again asks once more.
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("button", { name: "Allow" })).toBeInTheDocument();
    expect(mocks.lookupRemotePairing).toHaveBeenCalledTimes(2);
  });

  it("maps a network failure to the offline sentence", async () => {
    mocks.approveRemotePairing.mockRejectedValue(new Error("Remote connection unavailable; retry later"));
    setPendingPairingCode(SHORT);
    mount();
    fireEvent.click(await screen.findByRole("button", { name: "Allow" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Can't reach Wenlan's web service. Check your internet connection.");
  });

  it("reviews again if the saved settings changed under an open request, and never approves the old one", async () => {
    setPendingPairingCode(SHORT);
    const { client } = mount();
    await screen.findByRole("button", { name: "Allow" });
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, revision: "r9" });
    await act(async () => { await client.invalidateQueries({ queryKey: ["remote-access-profile"] }); });
    await waitFor(() => expect(mocks.lookupRemotePairing).toHaveBeenLastCalledWith("r9", SHORT_NORMAL));
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
  });

  it("will not allow a request whose time has just run out, even before the screen notices", async () => {
    const expiresAt = Date.now() + 60_000;
    mocks.lookupRemotePairing.mockResolvedValue({ ...claude, expiresAt });
    setPendingPairingCode(SHORT);
    mount();
    const allow = await screen.findByRole("button", { name: "Allow" });
    const clock = vi.spyOn(Date, "now").mockReturnValue(expiresAt + 1);
    try {
      fireEvent.click(allow);
      expect(await screen.findByRole("heading", { name: /request expired/ })).toBeInTheDocument();
    } finally {
      clock.mockRestore();
    }
    expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
  });

  it("puts the emphasis on Allow for an app it knows, and on Deny for one it does not", async () => {
    setPendingPairingCode(SHORT);
    mount();
    const knownAllow = (await screen.findByRole("button", { name: "Allow" })).className;
    const knownDeny = screen.getByRole("button", { name: "Deny" }).className;
    expect(knownAllow).not.toBe(knownDeny);
    cleanup();
    clearPendingPairingCode();
    mocks.lookupRemotePairing.mockResolvedValue(stranger);
    setPendingPairingCode("WXYZ-2345");
    mount();
    await screen.findByRole("heading", { name: /evil\.example/ });
    await waitFor(() => expect(screen.getByRole("button", { name: "Deny" }).className).toBe(knownAllow));
    expect(screen.getByRole("button", { name: "Allow" }).className).toBe(knownDeny);
  });

  it("keeps the request it is looking up or showing when another link arrives", async () => {
    let found: (request: typeof claude) => void = () => {};
    mocks.lookupRemotePairing.mockImplementationOnce(() => new Promise((resolve) => { found = resolve; }));
    setPendingPairingCode(SHORT);
    mount();
    await waitFor(() => expect(mocks.lookupRemotePairing).toHaveBeenCalledTimes(1));
    act(() => setPendingPairingCode("WXYZ-2345"));
    await act(async () => { found(claude); });
    await screen.findByRole("button", { name: "Allow" });
    act(() => setPendingPairingCode("WXYZ-2345"));
    await settled();
    expect(mocks.lookupRemotePairing).toHaveBeenCalledTimes(1);
    expect(mocks.lookupRemotePairing).toHaveBeenCalledWith("r1", SHORT_NORMAL);
    fireEvent.click(screen.getByRole("button", { name: "Allow" }));
    await waitFor(() => expect(mocks.approveRemotePairing).toHaveBeenCalledWith("r1", claude, "query"));
  });

  it("takes a newer link once the open request has an outcome", async () => {
    mocks.lookupRemotePairing.mockResolvedValueOnce({ ...claude, expiresAt: 1 });
    setPendingPairingCode(SHORT);
    mount();
    await screen.findByRole("heading", { name: /request expired/ });
    act(() => setPendingPairingCode("WXYZ-2345"));
    await waitFor(() => expect(mocks.lookupRemotePairing).toHaveBeenLastCalledWith("r1", "WXYZ2345"));
    expect(await screen.findByRole("button", { name: "Allow" })).toBeInTheDocument();
  });

  it("treats a code that is neither short nor long as an expired request, without asking anyone", async () => {
    setPendingPairingCode("not a code");
    mount();
    expect(await screen.findByRole("heading", { name: /request expired/ })).toBeInTheDocument();
    await settled();
    expect(mocks.lookupRemotePairing).not.toHaveBeenCalled();
    expect(mocks.inspectRemotePairing).not.toHaveBeenCalled();
  });

  describe("when Web access is off", () => {
    function goesOnOnceTurnedOn() {
      let on = false;
      mocks.getRemoteAccessProfile.mockImplementation(async () => (on ? profile : { ...profile, enabled: false, revision: "r0" }));
      mocks.getRemoteAccessStatus.mockImplementation(async () => (on ? connected : { status: "off" }));
      mocks.toggleRemoteAccess.mockImplementation(async () => { on = true; return { status: "starting" }; });
      mocks.configureRemoteAccess.mockImplementation(async () => ({ ...profile, enabled: false, revision: "r2" }));
    }

    it("asks what to share with nothing preselected, then goes on to ask who is connecting", async () => {
      goesOnOnceTurnedOn();
      setPendingPairingCode(SHORT);
      mount();
      expect(await screen.findByRole("heading", { name: "Turn on Web access?" })).toBeInTheDocument();
      const question = screen.getByRole("group", { name: "What can web apps search?" });
      expect(within(question).queryByRole("combobox")).not.toBeInTheDocument();
      expect(mocks.lookupRemotePairing).not.toHaveBeenCalled();
      fireEvent.click(within(question).getByRole("button", { name: "Share one Space…" }));
      fireEvent.change(within(question).getByRole("combobox", { name: "Space to share" }), { target: { value: "private" } });
      fireEvent.click(within(question).getByRole("button", { name: "Share this Space" }));
      await waitFor(() => expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(true, "r2"));
      expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "space", name: "private" }, "r0");
      expect(await screen.findByRole("heading", { name: "Allow Claude to use your library?" })).toBeInTheDocument();
      expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
    });

    it("turning on is not allowing: the whole library still stops at the question", async () => {
      goesOnOnceTurnedOn();
      setPendingPairingCode(SHORT);
      mount();
      fireEvent.click(await screen.findByRole("button", { name: "Share whole library" }));
      await waitFor(() => expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "wholeLibrary" }, "r0"));
      await screen.findByRole("button", { name: "Allow" });
      expect(mocks.approveRemotePairing).not.toHaveBeenCalled();
    });

    it("cancel closes without turning anything on", async () => {
      goesOnOnceTurnedOn();
      setPendingPairingCode(SHORT);
      mount();
      fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      await settled();
      expect(mocks.configureRemoteAccess).not.toHaveBeenCalled();
      expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
    });

    it("lets a newer link replace the request before anyone is asked about", async () => {
      goesOnOnceTurnedOn();
      setPendingPairingCode(SHORT);
      mount();
      await screen.findByRole("heading", { name: "Turn on Web access?" });
      act(() => setPendingPairingCode("WXYZ-2345"));
      expect(screen.getByTestId("code")).toHaveTextContent("WXYZ-2345");
    });

    it("shows why turning on failed", async () => {
      goesOnOnceTurnedOn();
      mocks.toggleRemoteAccess.mockRejectedValue(new Error("Remote connection unavailable; retry later"));
      setPendingPairingCode(SHORT);
      mount();
      fireEvent.click(await screen.findByRole("button", { name: "Share whole library" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("Can't reach Wenlan's web service.");
      expect(screen.getByRole("button", { name: "Share whole library" })).toBeEnabled();
    });

    it("with no Spaces, can still share the whole library", async () => {
      goesOnOnceTurnedOn();
      mocks.listSpaces.mockResolvedValue([]);
      setPendingPairingCode(SHORT);
      mount();
      expect(await screen.findByRole("button", { name: "Share whole library" })).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Share one Space…" })).toBeDisabled();
      await waitFor(() => expect(screen.getByRole("button", { name: "Share whole library" })).toBeEnabled());
    });
  });

  it.each([
    ["Web access was turned off", { enabled: false }],
    ["Web access still shows as connected", { enabled: true }],
  ])("will not look anything up while a remote disconnect is still pending (%s)", async (_label, over) => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, ...over, disconnect_pending: true });
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByText(/still removing this computer from the relay/)).toBeInTheDocument();
    await settled();
    expect(screen.queryByRole("button", { name: "Allow" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Share whole library" })).not.toBeInTheDocument();
    expect(mocks.lookupRemotePairing).not.toHaveBeenCalled();
  });

  it("will not offer to turn on when the saved settings cannot be read", async () => {
    mocks.getRemoteAccessProfile.mockRejectedValue(new Error("Storage unavailable"));
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Share whole library" })).not.toBeInTheDocument();
    expect(mocks.lookupRemotePairing).not.toHaveBeenCalled();
  });

  it("waits while Web access is still connecting", async () => {
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "starting" });
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByText(/Connecting/)).toBeInTheDocument();
    await settled();
    expect(mocks.lookupRemotePairing).not.toHaveBeenCalled();
  });

  describe("keyboard", () => {
    it("takes focus, closes on Escape without denying, and gives focus back", async () => {
      const opener = document.createElement("button");
      document.body.append(opener);
      opener.focus();
      setPendingPairingCode(SHORT);
      mount();
      const dialog = await screen.findByRole("dialog");
      await waitFor(() => expect(dialog).toHaveFocus());
      await screen.findByRole("button", { name: "Allow" });
      fireEvent.keyDown(dialog, { key: "Escape" });
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
      expect(mocks.denyRemotePairing).not.toHaveBeenCalled();
      expect(opener).toHaveFocus();
      opener.remove();
    });

    it("keeps Tab inside the dialog", async () => {
      setPendingPairingCode(SHORT);
      mount();
      const allow = await screen.findByRole("button", { name: "Allow" });
      const deny = screen.getByRole("button", { name: "Deny" });
      // The chosen radio is the first stop, ahead of Deny and Allow.
      const chosen = screen.getByRole("radio", { name: "Search only" });
      allow.focus();
      fireEvent.keyDown(allow, { key: "Tab" });
      expect(chosen).toHaveFocus();
      fireEvent.keyDown(chosen, { key: "Tab", shiftKey: true });
      expect(allow).toHaveFocus();
      expect(deny).not.toHaveFocus();
    });

    it("does not close from a click on the backdrop", async () => {
      setPendingPairingCode(SHORT);
      mount();
      const dialog = await screen.findByRole("dialog");
      fireEvent.click(dialog.parentElement!);
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    });
  });

  it.each([
    ["zh-Hant", "允許 Claude 使用你的資料庫嗎？", "允許", "拒絕", "它可以做什麼？", "只能搜尋", "搜尋和寫入"],
    ["zh-Hans", "允许 Claude 使用你的资料库吗？", "允许", "拒绝", "它可以做什么？", "只能搜索", "搜索和写入"],
  ])("speaks %s, with no English left in it", async (locale, title, allow, deny, question, queryLabel, writeLabel) => {
    await i18n.changeLanguage(locale);
    setPendingPairingCode(SHORT);
    mount();
    expect(await screen.findByRole("heading", { name: title })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: allow })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: deny })).toBeInTheDocument();
    expect(screen.getByRole("radiogroup", { name: question })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: queryLabel })).toBeChecked();
    expect(screen.getByRole("radio", { name: writeLabel })).not.toBeChecked();
    expect(screen.getByRole("dialog").textContent).not.toMatch(/Allow|Deny|search|library|private|Sends|write/i);
    expect(screen.getByRole("dialog").textContent).not.toMatch(/[—–]| - /);
  });
});
