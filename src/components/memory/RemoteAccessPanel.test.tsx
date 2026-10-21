import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup, act, within, configure } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { RemoteAccessPanel } from "./RemoteAccessPanel";
import {
  clearAwaitingConnection, clearPendingPairingCode, markPairingApproved, usePendingPairingCode,
} from "../../lib/pairingLink";

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
const mocks = vi.hoisted(() => Object.fromEntries([
  "toggleRemoteAccess", "reconnectRemoteAccess", "renewRemoteAccess", "getRemoteAccessStatus",
  "getRemoteAccessProfile", "configureRemoteAccess", "listSpaces", "listRemoteGrants",
  "revokeRemoteGrant", "testRemoteMcpConnection", "clipboardWrite",
].map((key) => [key, vi.fn()])));
vi.mock("../../lib/tauri", () => mocks);

const DAY = 24 * 60 * 60 * 1000;
const profile = { revision: "r1", space: "review", enabled: true, disconnect_pending: false, credential_expires_at: Date.now() + 60 * DAY };
const relayUrl = "https://relay.wenlan.app/mcp";
const connected = { status: "connected", tunnel_url: null, relay_url: relayUrl };
const twoSpaces = [{ id: "a", name: "review" }, { id: "b", name: "private" }];
const grant = (over: Record<string, unknown> = {}) => ({
  id: "g1", clientId: "synthetic-client-id", space: "review", status: "active", cleanupPending: false,
  createdAt: Date.now() - DAY, expiresAt: Date.now() + 29 * DAY,
  clientName: "Claude", redirectHost: "claude.ai", knownClient: true, lastUsedAt: Date.now() - 5 * 60_000, endReason: null,
  ...over,
});

function CodeProbe() {
  const code = usePendingPairingCode();
  return <span data-testid="pending-code">{code ?? "none"}</span>;
}
function panel(currentSpace?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return { ...render(<QueryClientProvider client={client}><RemoteAccessPanel currentSpace={currentSpace} /><CodeProbe /></QueryClientProvider>), client };
}
async function connectedPanel() {
  mocks.getRemoteAccessProfile.mockResolvedValue(profile);
  mocks.getRemoteAccessStatus.mockResolvedValue(connected);
  const view = panel();
  await screen.findByText("Connected");
  return view;
}
const button = (name: string | RegExp) => screen.getByRole("button", { name });
const TOGGLE = "Let web and phone AI apps use your library";
/** The switch is a button only once its value is known; before that it is a status. */
const findToggle = () => screen.findByRole("button", { name: TOGGLE });
/** Flips Web access on while Spaces exist, which asks the one question. */
async function openChooser() {
  fireEvent.click(await findToggle());
  return screen.findByRole("group", { name: "What can web apps search?" });
}

// jsdom accessible-name queries are slow on a busy machine; the first test of a
// file also pays the cold start. 4s still fails ahead of the 5s test timeout.
configure({ asyncUtilTimeout: 4_000 });

beforeEach(() => {
  vi.resetAllMocks();
  mocks.getRemoteAccessStatus.mockResolvedValue({ status: "off" });
  mocks.getRemoteAccessProfile.mockResolvedValue(null);
  mocks.listSpaces.mockResolvedValue([{ id: "s1", name: "review" }]);
  mocks.configureRemoteAccess.mockResolvedValue({ ...profile, enabled: false, revision: "configured" });
  mocks.toggleRemoteAccess.mockResolvedValue({ status: "starting" });
  mocks.reconnectRemoteAccess.mockResolvedValue({ status: "starting" });
  mocks.renewRemoteAccess.mockResolvedValue({ status: "starting" });
  mocks.listRemoteGrants.mockResolvedValue({ items: [], cursor: null });
  mocks.revokeRemoteGrant.mockResolvedValue({ revoked: true, cleanupPending: false });
  mocks.testRemoteMcpConnection.mockResolvedValue({ ok: true, latency_ms: 42, error: null });
  mocks.clipboardWrite.mockResolvedValue(undefined);
});
afterEach(async () => {
  cleanup(); vi.useRealTimers(); clearPendingPairingCode(); clearAwaitingConnection();
  await i18n.changeLanguage("en");
});

describe("turning on", () => {
  it("is one switch, off, with the relay and stay-on facts in one short line", async () => {
    panel();
    const toggle = await findToggle();
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText("Claude, ChatGPT and others reach it through wenlan-relay. Keep this computer on.")).toBeInTheDocument();
    expect(screen.queryByText("Off")).not.toBeInTheDocument();
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Have a code?" })).toBeInTheDocument();
  });

  it("with Spaces, flipping it on asks one question and turns nothing on yet", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    panel();
    const question = await openChooser();
    expect(within(question).getByRole("button", { name: "Share whole library" })).toBeEnabled();
    expect(within(question).getByRole("button", { name: "Share one Space…" })).toHaveAttribute("aria-expanded", "false");
    expect(question).not.toHaveTextContent(/Whole library means|create a Space first/);
    // Nothing is preselected: no Space picker, no choice made for the person.
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: TOGGLE })).toHaveAttribute("aria-pressed", "true");
    expect(mocks.configureRemoteAccess).not.toHaveBeenCalled();
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
  });

  it("flipping it back off while the question shows just hides the question", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    panel();
    await openChooser();
    fireEvent.click(button(TOGGLE));
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: TOGGLE })).toHaveAttribute("aria-pressed", "false");
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
  });

  it("the whole library is one click once the question is open", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    panel();
    const question = await openChooser();
    fireEvent.click(within(question).getByRole("button", { name: "Share whole library" }));
    await waitFor(() => expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(true, "configured"));
    expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "wholeLibrary" }, undefined);
  });

  it("one Space starts from the one being viewed, and shares only the one picked", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    panel("private");
    await openChooser();
    fireEvent.click(button("Share one Space…"));
    const select = await screen.findByRole("combobox", { name: "Space to share" });
    expect(select).toHaveValue("private");
    fireEvent.change(select, { target: { value: "review" } });
    fireEvent.click(button("Share this Space"));
    await waitFor(() => expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "space", name: "review" }, undefined));
    expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(true, "configured");
  });

  it("one Space starts from the first Space when nothing else says which", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    panel();
    await openChooser();
    fireEvent.click(button("Share one Space…"));
    expect(await screen.findByRole("combobox", { name: "Space to share" })).toHaveValue("review");
  });

  it("never offers a Space named like the whole library as one Space", async () => {
    mocks.listSpaces.mockResolvedValue([{ id: "x", name: "*" }, { id: "s1", name: "review" }]);
    panel();
    await openChooser();
    fireEvent.click(button("Share one Space…"));
    const select = await screen.findByRole("combobox", { name: "Space to share" });
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual(["review"]);
  });

  it("asks the same question again after it was turned off", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false });
    panel();
    expect(await findToggle()).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
    const question = await openChooser();
    expect(within(question).getByRole("button", { name: "Share whole library" })).toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
  });

  it("with no Spaces there is nothing to ask: flipping it on shares the whole library", async () => {
    mocks.listSpaces.mockResolvedValue([]);
    panel();
    fireEvent.click(await findToggle());
    await waitFor(() => expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "wholeLibrary" }, undefined));
    expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(true, "configured");
    expect(screen.queryByRole("group", { name: "What can web apps search?" })).not.toBeInTheDocument();
  });

  it("a failed Space lookup is said when the switch is flipped on, and nothing is turned on", async () => {
    mocks.listSpaces.mockRejectedValue(new Error("Daemon offline"));
    panel();
    fireEvent.click(await findToggle());
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "What can web apps search?" })).not.toBeInTheDocument();
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
  });

  it("shows Loading in the switch's place while the saved state and Spaces are read", async () => {
    mocks.getRemoteAccessProfile.mockReturnValue(new Promise(() => {}));
    panel();
    expect(await screen.findByRole("status", { name: TOGGLE })).toHaveTextContent("Loading");
    expect(screen.queryByRole("button", { name: TOGGLE })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Have a code?" })).not.toBeInTheDocument();
  });

  it("shows Saving beside the switch while a change is in flight, and ignores clicks on it", async () => {
    mocks.listSpaces.mockResolvedValue([]);
    mocks.configureRemoteAccess.mockReturnValue(new Promise(() => {}));
    panel();
    fireEvent.click(await findToggle());
    const saving = await screen.findByRole("status", { name: "Saving" });
    expect(saving).toBeInTheDocument();
    const toggle = screen.getByRole("button", { name: TOGGLE });
    expect(toggle).toHaveAttribute("aria-disabled", "true");
    fireEvent.click(toggle);
    expect(mocks.configureRemoteAccess).toHaveBeenCalledTimes(1);
  });

  it("says Connecting while it starts", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "starting" });
    panel();
    expect(await screen.findByText("Connecting…")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Test connection" })).not.toBeInTheDocument();
  });

  it("says why turning on failed, in plain words, with the raw sentence behind Details", async () => {
    mocks.toggleRemoteAccess.mockRejectedValue(new Error("Remote connection rate limited"));
    mocks.listSpaces.mockResolvedValue([]);
    panel();
    fireEvent.click(await findToggle());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Too many tries. Wait a few minutes, then try again.");
    expect(screen.queryByText(/rate limited/)).not.toBeInTheDocument();
    fireEvent.click(button("Details"));
    expect(screen.getByText("Remote connection rate limited")).toBeInTheDocument();
  });

  it("unknown native settings do not appear as permission to turn on, but Stop access stays", async () => {
    mocks.getRemoteAccessProfile.mockRejectedValueOnce(new Error("Storage unavailable"));
    panel();
    expect(await screen.findByRole("alert")).toHaveTextContent("Something went wrong. Try again.");
    expect(screen.queryByRole("button", { name: "Turn on" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Turn on again" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Share whole library" })).not.toBeInTheDocument();
    // The switch claims neither position and cannot be flipped on.
    expect(screen.getByRole("status", { name: TOGGLE })).toHaveTextContent("Unavailable");
    expect(screen.queryByRole("button", { name: TOGGLE })).not.toBeInTheDocument();
    fireEvent.click(button("Stop access"));
    await waitFor(() => expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(false));
  });

  it("replaces the failure after the read recovers", async () => {
    mocks.getRemoteAccessProfile.mockRejectedValueOnce(new Error("Storage unavailable"));
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    const { client } = panel();
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    await client.invalidateQueries({ queryKey: ["remote-access-profile"] });
    expect(await screen.findByText("Connected")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop access" })).not.toBeInTheDocument();
  });

  it("a failed Space lookup does not stop an existing connection from being turned off", async () => {
    mocks.listSpaces.mockRejectedValue(new Error("Daemon offline"));
    await connectedPanel();
    fireEvent.click(button(TOGGLE));
    fireEvent.click(within(await screen.findByRole("group")).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(false));
  });
});

describe("a connection that is on", () => {
  it("shows status, the Space, and one short instruction that adds Wenlan for you", async () => {
    await connectedPanel();
    expect(screen.getByText("Sharing: review")).toBeInTheDocument();
    expect(screen.getByText("Add Wenlan to your AI app, then click Allow when Wenlan asks.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add to Claude" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add to ChatGPT" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Other apps" })).toBeInTheDocument();
    // The link is not in the card; it lives in the Other apps guide.
    expect(screen.queryByText(relayUrl)).not.toBeInTheDocument();
    expect(screen.queryByText(/Another app/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy URL" })).not.toBeInTheDocument();
    expect(screen.queryByText(/use Add a tool above/)).not.toBeInTheDocument();
    expect(await screen.findByText("No apps connected yet")).toBeInTheDocument();
  });

  it("separates its sections by space, never by horizontal rules", async () => {
    await connectedPanel();
    await screen.findByText("No apps connected yet");
    // The card's own divider under the toggle row stays; everything in the body below it has none.
    const body = screen.getByText("Connected").closest(".px-5.py-4") as HTMLElement;
    expect(body).not.toBeNull();
    expect(body.querySelector("ol")).toBeNull();
    expect(body.querySelector("[class*='divide-y']")).toBeNull();
    expect(body.querySelector(".border-t")).toBeNull();
  });

  it("says the whole library in words, never the stored value", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*" });
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    expect(await screen.findByText("Sharing: Whole library")).toBeInTheDocument();
    expect(screen.queryByText(/\*/)).not.toBeInTheDocument();
  });

  it("Add to Claude opens Claude's add-connector form with Wenlan's link filled in", async () => {
    await connectedPanel();
    fireEvent.click(button("Add to Claude"));
    await waitFor(() => expect(openExternal).toHaveBeenCalledTimes(1));
    const opened = new URL(vi.mocked(openExternal).mock.calls[0][0]);
    expect(opened.origin + opened.pathname).toBe("https://claude.ai/customize/connectors");
    expect(Object.fromEntries(opened.searchParams)).toEqual({
      modal: "add-custom-connector", connectorName: "Wenlan", connectorUrl: relayUrl,
    });
    expect(mocks.clipboardWrite).not.toHaveBeenCalled();
  });

  it("ChatGPT copies the link first, opens ChatGPT, and says where to paste it", async () => {
    await connectedPanel();
    expect(screen.queryByText(/Developer mode/)).not.toBeInTheDocument();
    fireEvent.click(button("Add to ChatGPT"));
    await waitFor(() => expect(openExternal).toHaveBeenCalledWith("https://chatgpt.com/plugins"));
    expect(mocks.clipboardWrite).toHaveBeenCalledWith(relayUrl);
    expect(mocks.clipboardWrite.mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(openExternal).mock.invocationCallOrder[0]);
    expect(await screen.findByText(/Link copied\. In ChatGPT, click Add, then Add custom MCP server\./)).toBeInTheDocument();
  });

  it("Other apps opens a three step guide with the link; the card itself never shows the link", async () => {
    await connectedPanel();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(button("Other apps"));
    const dialog = await screen.findByRole("dialog", { name: "Add Wenlan to another AI app" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    const steps = within(dialog).getAllByRole("listitem");
    expect(steps).toHaveLength(3);
    expect(steps[0]).toHaveTextContent("In the app's settings, open Connectors and add a custom connector.");
    expect(steps[1]).toHaveTextContent("Paste this link.");
    expect(steps[1]).toHaveTextContent(relayUrl);
    expect(steps[2]).toHaveTextContent("When Wenlan asks, click Allow.");
    expect(within(dialog).getByText("Works with apps that support custom MCP connectors, such as Muse.")).toBeInTheDocument();
    expect(dialog.textContent).not.toMatch(/[\u2013\u2014]/);
    expect(dialog.querySelector("hr, .border-t, [class*='divide-']")).toBeNull();
    // The dialog holds focus; the card behind it keeps no second copy of the link.
    expect(dialog).toHaveFocus();
    expect(screen.getAllByText(relayUrl)).toHaveLength(1);
  });

  it("opening the guide copies the relay URL, like Add to ChatGPT, and Copy copies it again", async () => {
    await connectedPanel();
    fireEvent.click(button("Other apps"));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(mocks.clipboardWrite).toHaveBeenCalledWith(relayUrl));
    expect(mocks.clipboardWrite).toHaveBeenCalledTimes(1);
    expect(await within(dialog).findByRole("button", { name: "Copied" })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Copied" }));
    await waitFor(() => expect(mocks.clipboardWrite).toHaveBeenCalledTimes(2));
    expect(mocks.clipboardWrite).toHaveBeenLastCalledWith(relayUrl);
    expect(openExternal).not.toHaveBeenCalled();
  });

  it("a failed copy is not reported as copied", async () => {
    mocks.clipboardWrite.mockRejectedValue(new Error("denied"));
    await connectedPanel();
    fireEvent.click(button("Other apps"));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(mocks.clipboardWrite).toHaveBeenCalled());
    expect(within(dialog).getByRole("button", { name: "Copy URL" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Copied" })).not.toBeInTheDocument();
  });

  it("Escape closes the guide and returns focus to Other apps", async () => {
    await connectedPanel();
    const trigger = button("Other apps");
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("dialog");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(trigger).toHaveFocus();
  });

  it("Done closes the guide and returns focus to Other apps", async () => {
    await connectedPanel();
    const trigger = button("Other apps");
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(trigger).toHaveFocus();
  });

  it("the guide keeps Tab inside it", async () => {
    await connectedPanel();
    fireEvent.click(button("Other apps"));
    const dialog = await screen.findByRole("dialog");
    // Opening copied the link already, so the first control reads Copied.
    const copyAgain = await within(dialog).findByRole("button", { name: "Copied" });
    const done = within(dialog).getByRole("button", { name: "Done" });
    done.focus();
    fireEvent.keyDown(done, { key: "Tab" });
    expect(copyAgain).toHaveFocus();
    fireEvent.keyDown(dialog, { key: "Tab", shiftKey: true });
    expect(done).toHaveFocus();
  });

  it("has nothing to copy, and no steps, when the relay URL is missing", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue({ ...connected, relay_url: null });
    panel();
    await screen.findByText("Connected");
    expect(screen.queryByRole("button", { name: "Other apps" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add to Claude" })).not.toBeInTheDocument();
    expect(screen.queryByText("Add Wenlan to your AI app.")).not.toBeInTheDocument();
  });

  it("reconnect restarts at the saved revision without turning off or reconfiguring", async () => {
    await connectedPanel();
    fireEvent.click(button("Reconnect"));
    await waitFor(() => expect(mocks.reconnectRemoteAccess).toHaveBeenCalledWith("r1"));
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
    expect(mocks.configureRemoteAccess).not.toHaveBeenCalled();
    expect(mocks.revokeRemoteGrant).not.toHaveBeenCalled();
  });

  it("a failed reconnect never falls back to turning anything off or on", async () => {
    await connectedPanel();
    mocks.reconnectRemoteAccess.mockRejectedValue(new Error("Transport cleanup unconfirmed"));
    fireEvent.click(button("Reconnect"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Something went wrong. Try again.");
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
  });

  it("Test connection says what it proved, and nothing more", async () => {
    await connectedPanel();
    fireEvent.click(button("Test connection"));
    expect(await screen.findByText("Connected. Wenlan's web service sees this computer (42 ms).")).toBeInTheDocument();
    expect(screen.queryByText(/backend and relay verified/i)).not.toBeInTheDocument();
  });

  it("Test connection failure is plain, with the reason behind Details", async () => {
    mocks.testRemoteMcpConnection.mockResolvedValue({ ok: false, latency_ms: null, error: "relay does not see this device (HTTP 502)" });
    await connectedPanel();
    fireEvent.click(button("Test connection"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Connection check failed");
    expect(screen.queryByText(/502/)).not.toBeInTheDocument();
    fireEvent.click(button("Details"));
    expect(screen.getByText("relay does not see this device (HTTP 502)")).toBeInTheDocument();
  });

  it("shows a status error in plain words and offers Stop access", async () => {
    const warning = "Local transport stop requested; remote access settings are not confirmed; server revoke failed: Remote connection unavailable; retry later; disconnect must be retried before app restart";
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "error", error: warning });
    panel();
    expect(await screen.findByRole("alert")).toHaveTextContent("Can't reach Wenlan's web service. Check your internet connection.");
    expect(screen.queryByText("Connected")).not.toBeInTheDocument();
    expect(screen.queryByText(/revoke failed/)).not.toBeInTheDocument();
    fireEvent.click(button("Stop access"));
    await waitFor(() => expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false]]));
  });

  it("allows another Stop when local processes are unconfirmed, and offers no Turn on beside it", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false });
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "error", error: "Local remote-access processes have not been confirmed stopped. New connections are blocked; retry Stop access." });
    panel();
    await screen.findByRole("alert");
    expect(screen.queryByRole("button", { name: /^Turn on/ })).not.toBeInTheDocument();
    fireEvent.click(button("Stop access"));
    await waitFor(() => expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false]]));
  });

  it("a pending disconnect blocks turning on and reconnecting, and offers Retry disconnect", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false, disconnect_pending: true });
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    fireEvent.click(await screen.findByRole("button", { name: "Retry disconnect" }));
    expect(screen.queryByRole("button", { name: "Reconnect" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Turn on/ })).not.toBeInTheDocument();
    await waitFor(() => expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(false));
    expect(mocks.reconnectRemoteAccess).not.toHaveBeenCalled();
  });
});

describe("asking before it ends connections", () => {
  it("turn off asks first, Escape cancels, then it turns off", async () => {
    await connectedPanel();
    expect(button(TOGGLE)).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: "Turn off" })).not.toBeInTheDocument();
    fireEvent.click(button(TOGGLE));
    const ask = await screen.findByRole("group");
    expect(ask).toHaveTextContent("Turn off Web access? Connected web apps lose access right away.");
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
    fireEvent.keyDown(ask, { key: "Escape" });
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
    fireEvent.click(button(TOGGLE));
    fireEvent.click(within(await screen.findByRole("group")).getByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(mocks.toggleRemoteAccess).toHaveBeenCalledWith(false));
  });

  it("changing to another Space turns off, saves it, and turns on again, after a confirm", async () => {
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    await connectedPanel();
    fireEvent.click(button("Change"));
    const question = await screen.findByRole("group", { name: "What can web apps search?" });
    fireEvent.click(within(question).getByRole("button", { name: "Share one Space…" }));
    // The Space shared now is not offered again.
    const select = within(question).getByRole("combobox", { name: "Space to share" });
    expect(within(select).getAllByRole("option").map((option) => option.textContent)).toEqual(["private"]);
    fireEvent.click(within(question).getByRole("button", { name: "Share this Space" }));
    const ask = await screen.findByRole("group");
    expect(ask).toHaveTextContent("Share private instead? Web apps connected now will need to connect again.");
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false, revision: "r5" });
    fireEvent.click(within(ask).getByRole("button", { name: "Change" }));
    await waitFor(() => expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false], [true, "configured"]]));
    expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "space", name: "private" }, "r5");
  });

  it("one shared Space can widen to the whole library, after a confirm that says so", async () => {
    await connectedPanel();
    fireEvent.click(button("Change"));
    const question = await screen.findByRole("group", { name: "What can web apps search?" });
    expect(within(question).getByRole("button", { name: "Share one Space…" })).toBeDisabled();
    fireEvent.click(within(question).getByRole("button", { name: "Share whole library" }));
    const ask = await screen.findByRole("group");
    expect(ask).toHaveTextContent("Share the whole library instead? Web apps connected now will need to connect again.");
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false, revision: "r6" });
    fireEvent.click(within(ask).getByRole("button", { name: "Change" }));
    await waitFor(() => expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false], [true, "configured"]]));
    expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "wholeLibrary" }, "r6");
  });

  it("the whole library can narrow to one Space, and is not offered again", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*" });
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    await screen.findByText("Connected");
    await waitFor(() => expect(button("Change")).toBeEnabled());
    fireEvent.click(button("Change"));
    const question = await screen.findByRole("group", { name: "What can web apps search?" });
    expect(within(question).getByRole("button", { name: "Share whole library" })).toBeDisabled();
    fireEvent.click(within(question).getByRole("button", { name: "Share one Space…" }));
    fireEvent.click(within(question).getByRole("button", { name: "Share this Space" }));
    expect(await screen.findByRole("group")).toHaveTextContent("Share review instead?");
  });

  it("has nothing to change to when the whole library is shared and there are no Spaces", async () => {
    mocks.listSpaces.mockResolvedValue([]);
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*" });
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    await screen.findByText("Connected");
    expect(button("Change")).toBeDisabled();
  });

  it("removing an app asks, then revokes at the exact revision and grant", async () => {
    mocks.listRemoteGrants.mockResolvedValue({ items: [grant({ id: "g9" })], cursor: null });
    await connectedPanel();
    fireEvent.click(await screen.findByRole("button", { name: "Remove access" }));
    const ask = await screen.findByRole("group");
    expect(ask).toHaveTextContent("Remove access for Claude? It will need to connect again.");
    expect(mocks.revokeRemoteGrant).not.toHaveBeenCalled();
    fireEvent.click(within(ask).getByRole("button", { name: "Remove access" }));
    await waitFor(() => expect(mocks.revokeRemoteGrant).toHaveBeenCalledWith("r1", "g9"));
    expect(mocks.revokeRemoteGrant).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Ended: access was removed")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove access" })).not.toBeInTheDocument();
  });

  it("shows revocation separately from token cleanup, and offers a retry", async () => {
    mocks.listRemoteGrants.mockResolvedValue({ items: [grant()], cursor: null });
    mocks.revokeRemoteGrant.mockResolvedValue({ revoked: true, cleanupPending: true });
    await connectedPanel();
    fireEvent.click(await screen.findByRole("button", { name: "Remove access" }));
    fireEvent.click(within(await screen.findByRole("group")).getByRole("button", { name: "Remove access" }));
    expect(await screen.findByText("Access revoked. Stored token cleanup is pending.")).toBeInTheDocument();
    mocks.revokeRemoteGrant.mockResolvedValue({ revoked: true, cleanupPending: false });
    fireEvent.click(button("Retry"));
    await waitFor(() => expect(mocks.revokeRemoteGrant).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(screen.queryByText("Access revoked. Stored token cleanup is pending.")).not.toBeInTheDocument());
  });
});

describe("connected apps", () => {
  it.each([
    ["still connecting", { status: "starting" }],
    ["cannot reach the service", { status: "error", error: "Remote connection unavailable; retry later" }],
  ])("shows no empty list header while %s, and never asks the relay", async (_label, status) => {
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue(status);
    panel();
    await screen.findByRole("button", { name: "Reconnect" });
    expect(screen.queryByText("Connected apps")).not.toBeInTheDocument();
    expect(mocks.listRemoteGrants).not.toHaveBeenCalled();
  });

  it("reads in plain words: who, last used, when it ends, and why one ended", async () => {
    const endsAt = Date.now() + 20 * DAY;
    mocks.listRemoteGrants.mockResolvedValue({ items: [
      grant({ expiresAt: endsAt }),
      grant({ id: "g2", clientId: "client-two", clientName: "Mystery", redirectHost: "other.example", knownClient: false, lastUsedAt: null }),
      grant({ id: "g3", clientId: "client-three", clientName: null, redirectHost: null, knownClient: false }),
      grant({ id: "g4", clientId: "client-four", redirectHost: "chatgpt.com", status: "inactive", endReason: "reset" }),
      grant({ id: "g5", clientId: "client-five", status: "inactive", endReason: "expired" }),
      grant({ id: "g6", clientId: "client-six", status: "inactive", endReason: "replaced" }),
      grant({ id: "g7", clientId: "client-seven", status: "inactive", endReason: "other" }),
    ], cursor: null });
    await connectedPanel();
    const date = new Date(endsAt).toLocaleDateString("en");
    expect(await screen.findByText(`Last used 5m ago · Ends ${date}`)).toBeInTheDocument();
    expect(screen.getByText("other.example")).toBeInTheDocument();
    expect(screen.getByText(/Not used yet/)).toBeInTheDocument();
    expect(screen.getByText("Unrecognized app")).toBeInTheDocument();
    expect(screen.queryByText("Mystery")).not.toBeInTheDocument();
    expect(screen.getByText("ChatGPT")).toBeInTheDocument();
    expect(screen.getByText("Ended: Web access was renewed or what it shares changed")).toBeInTheDocument();
    expect(screen.getByText("Ended: it expired")).toBeInTheDocument();
    expect(screen.getByText("Ended: a newer connection replaced it")).toBeInTheDocument();
    expect(screen.getByText("Ended")).toBeInTheDocument();
    // The client id is not in the list, only behind Details.
    expect(screen.queryByText("synthetic-client-id")).not.toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Details" })[0]);
    expect(screen.getByText("synthetic-client-id")).toBeInTheDocument();
  });

  it("tags each active app with what it may do, and reads a missing access as search only", async () => {
    mocks.listRemoteGrants.mockResolvedValue({ items: [
      grant({ id: "g1", access: "write" }),
      grant({ id: "g2", clientId: "client-two", redirectHost: "chatgpt.com", access: "query" }),
      // A relay that predates the choice sends no access at all.
      grant({ id: "g3", clientId: "client-three", redirectHost: "agent.meta.ai" }),
      grant({ id: "g4", clientId: "client-four", redirectHost: "notes.example.org", knownClient: false, status: "inactive", endReason: "revoked", access: "write" }),
    ], cursor: null });
    await connectedPanel();
    expect(await screen.findAllByText("Search only")).toHaveLength(2);
    expect(screen.getAllByText("Can write")).toHaveLength(1);
    const claude = screen.getByText("Claude").closest("li")!;
    expect(within(claude).getByText("Can write")).toBeInTheDocument();
    const chatgpt = screen.getByText("ChatGPT").closest("li")!;
    expect(within(chatgpt).getByText("Search only")).toBeInTheDocument();
  });

  it("tags the apps in Traditional Chinese", async () => {
    await i18n.changeLanguage("zh-Hant");
    mocks.listRemoteGrants.mockResolvedValue({ items: [
      grant({ access: "write" }),
      grant({ id: "g2", clientId: "client-two", redirectHost: "chatgpt.com", access: "query" }),
    ], cursor: null });
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    expect(await screen.findByText("可以寫入")).toBeInTheDocument();
    expect(screen.getByText("只能搜尋")).toBeInTheDocument();
  });

  it("names a whole-library connection in words behind Details", async () => {
    mocks.listRemoteGrants.mockResolvedValue({ items: [grant({ space: "*" })], cursor: null });
    await connectedPanel();
    fireEvent.click(await screen.findByRole("button", { name: "Details" }));
    expect(screen.getByText("Sharing: Whole library")).toBeInTheDocument();
  });

  it("works against a relay that does not send the newer fields", async () => {
    mocks.listRemoteGrants.mockResolvedValue({ items: [
      { id: "old", clientId: "legacy-client", space: "review", status: "active", cleanupPending: false, createdAt: Date.now(), expiresAt: Date.now() + 29 * DAY },
      { id: "old2", clientId: "legacy-two", space: "review", status: "inactive", cleanupPending: false, createdAt: Date.now(), expiresAt: Date.now() },
    ], cursor: null });
    await connectedPanel();
    expect(await screen.findAllByText("Unrecognized app")).toHaveLength(2);
    expect(screen.queryByText(/Not used yet/)).not.toBeInTheDocument();
    expect(screen.getByText("Ended")).toBeInTheDocument();
    expect(screen.queryByText("legacy-client")).not.toBeInTheDocument();
  });

  it("follows the app language for dates and wording", async () => {
    const endsAt = Date.now() + 29 * DAY;
    mocks.listRemoteGrants.mockResolvedValue({ items: [grant({ expiresAt: endsAt })], cursor: null });
    await i18n.changeLanguage("zh-Hant");
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    const date = new Date(endsAt).toLocaleDateString("zh-Hant");
    expect(await screen.findByText(`上次使用：5 分鐘前 · ${date} 結束`)).toBeInTheDocument();
  });

  it("pages through a long list", async () => {
    mocks.listRemoteGrants.mockResolvedValueOnce({ items: [grant()], cursor: "next" });
    await connectedPanel();
    mocks.listRemoteGrants.mockResolvedValueOnce({ items: [grant({ id: "g2", redirectHost: "chatgpt.com" })], cursor: null });
    fireEvent.click(await screen.findByRole("button", { name: "Next page" }));
    expect(await screen.findByText("ChatGPT")).toBeInTheDocument();
    expect(mocks.listRemoteGrants).toHaveBeenLastCalledWith("r1", "next");
    fireEvent.click(button("First page"));
    await waitFor(() => expect(mocks.listRemoteGrants).toHaveBeenLastCalledWith("r1", null));
  });
});

describe("when the key is running out or has run out", () => {
  it("offers Renew within 14 days, after a confirm", async () => {
    const endsAt = Date.now() + 10 * DAY;
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, credential_expires_at: endsAt });
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    expect(await screen.findByText(`Web access ends on ${new Date(endsAt).toLocaleDateString("en")}.`)).toBeInTheDocument();
    fireEvent.click(button("Renew"));
    const ask = await screen.findByRole("group");
    expect(ask).toHaveTextContent("Keep Web access on for 90 more days? Connected web apps will need to connect again.");
    expect(mocks.renewRemoteAccess).not.toHaveBeenCalled();
    fireEvent.click(within(ask).getByRole("button", { name: "Renew" }));
    await waitFor(() => expect(mocks.renewRemoteAccess).toHaveBeenCalledWith("r1"));
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
  });

  it("says nothing about renewing when the key has plenty of time", async () => {
    await connectedPanel();
    expect(screen.queryByRole("button", { name: "Renew" })).not.toBeInTheDocument();
  });

  it("shows an ended banner with 'Turn on again', and nothing that implies it still works", async () => {
    const endedAt = Date.now() - 2 * DAY;
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, credential_expires_at: endedAt });
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "error", error: "Remote connection requires device authorization" });
    mocks.listSpaces.mockResolvedValue([{ id: "s1", name: "review" }]);
    panel();
    expect(await screen.findByText(`Web access ended on ${new Date(endedAt).toLocaleDateString("en")}.`)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText("Connected")).not.toBeInTheDocument();
    expect(screen.queryByText("Add Wenlan to your AI app.")).not.toBeInTheDocument();
    expect(screen.queryByText("Connected apps")).not.toBeInTheDocument();
    expect(mocks.listRemoteGrants).not.toHaveBeenCalled();
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false, revision: "r7" });
    fireEvent.click(button("Turn on again"));
    await waitFor(() => expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false], [true, "configured"]]));
    expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "space", name: "review" }, "r7");
  });

  it("'Turn on again' keeps the whole library as the whole library", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*", credential_expires_at: Date.now() - DAY });
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "off" });
    panel();
    await screen.findByText(/Web access ended on/);
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*", enabled: false, revision: "r8" });
    await waitFor(() => expect(button("Turn on again")).toBeEnabled());
    fireEvent.click(button("Turn on again"));
    await waitFor(() => expect(mocks.configureRemoteAccess).toHaveBeenCalledWith({ kind: "wholeLibrary" }, "r8"));
  });

  it("'Turn on again' never swaps a Space that is gone for another one: it stops and asks", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, credential_expires_at: Date.now() - DAY });
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "off" });
    mocks.listSpaces.mockResolvedValue([{ id: "s2", name: "private" }]);
    panel();
    await screen.findByText(/Web access ended on/);
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, enabled: false, revision: "r9" });
    await waitFor(() => expect(button("Turn on again")).toBeEnabled());
    fireEvent.click(button("Turn on again"));
    expect(await screen.findByRole("group", { name: "What can web apps search?" })).toBeInTheDocument();
    expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false]]);
    expect(mocks.configureRemoteAccess).not.toHaveBeenCalled();
  });

  it("an ended key can simply be turned off", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, credential_expires_at: Date.now() - DAY });
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "off" });
    panel();
    fireEvent.click(await screen.findByRole("button", { name: "Turn off" }));
    await waitFor(() => expect(mocks.toggleRemoteAccess.mock.calls).toEqual([[false]]));
  });

  it("an ended key's switch also asks before turning off", async () => {
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, credential_expires_at: Date.now() - DAY });
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "off" });
    panel();
    fireEvent.click(await findToggle());
    expect(await screen.findByRole("group")).toHaveTextContent("Turn off Web access?");
    expect(mocks.toggleRemoteAccess).not.toHaveBeenCalled();
  });
});

describe("Have a code?", () => {
  it("is collapsed until asked", async () => {
    panel();
    const toggle = await screen.findByRole("button", { name: "Have a code?" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("textbox", { name: "Pairing code" })).not.toBeInTheDocument();
    fireEvent.click(toggle);
    expect(screen.getByRole("textbox", { name: "Pairing code" })).toBeInTheDocument();
  });

  it.each([
    ["abcd-2345", "ABCD2345"],
    ["  wxyz 2345 ", "WXYZ2345"],
    ["b".repeat(64), "b".repeat(64)],
  ])("hands %j to the approval dialog as %j", async (typed, parked) => {
    panel();
    fireEvent.click(await screen.findByRole("button", { name: "Have a code?" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Pairing code" }), { target: { value: typed } });
    fireEvent.click(button("Review request"));
    expect(screen.getByTestId("pending-code")).toHaveTextContent(parked);
    expect(screen.getByRole("textbox", { name: "Pairing code" })).toHaveValue("");
  });

  it("refuses a code that cannot be one, and says what a code looks like", async () => {
    panel();
    fireEvent.click(await screen.findByRole("button", { name: "Have a code?" }));
    const input = screen.getByRole("textbox", { name: "Pairing code" });
    expect(button("Review request")).toBeDisabled();
    // 0, O, 1, I, L and U are not in the code alphabet.
    fireEvent.change(input, { target: { value: "ABCD-0O1I" } });
    expect(button("Review request")).toBeDisabled();
    expect(screen.getByText("Enter the 8-character code from the web page, or the longer code from a link.")).toBeInTheDocument();
    fireEvent.submit(input.closest("form")!);
    expect(screen.getByTestId("pending-code")).toHaveTextContent("none");
  });

  it("is not offered while the saved settings cannot be read", async () => {
    mocks.getRemoteAccessProfile.mockRejectedValue(new Error("Storage unavailable"));
    panel();
    await screen.findByRole("alert");
    expect(screen.queryByRole("button", { name: "Have a code?" })).not.toBeInTheDocument();
  });
});

describe("in every language", () => {
  it.each([
    ["zh-Hant", "網頁存取", "分享整個資料庫", "新增到 Claude", "正在分享：整個資料庫"],
    ["zh-Hans", "网页访问", "共享整个资料库", "添加到 Claude", "正在共享：整个资料库"],
  ])("renders %s without English fallback", async (locale, title, whole, addToClaude, sharingWhole) => {
    const toggleTitle = locale === "zh-Hant" ? "讓網頁和手機上的 AI 使用你的資料庫" : "让网页和手机上的 AI 使用你的资料库";
    await i18n.changeLanguage(locale);
    panel();
    expect(await screen.findByRole("heading", { name: title })).toBeInTheDocument();
    mocks.listSpaces.mockResolvedValue(twoSpaces);
    fireEvent.click(await screen.findByRole("button", { name: toggleTitle }));
    expect(await screen.findByRole("button", { name: whole })).toBeInTheDocument();
    expect(screen.queryByText(/Share|Whole library|Web access|What can|Space to share|create a Space|Let web|Keep this/)).not.toBeInTheDocument();
    cleanup();
    mocks.getRemoteAccessProfile.mockResolvedValue({ ...profile, space: "*" });
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    await screen.findByRole("button", { name: i18n.t("remoteAccess.testConnection") });
    expect(screen.getByRole("button", { name: addToClaude })).toBeInTheDocument();
    expect(screen.getByText(sharingWhole)).toBeInTheDocument();
    expect(screen.queryByText(/Connected|Add Wenlan|Add to Claude|Other apps|Sharing|Test connection|Turn off|Change/)).not.toBeInTheDocument();
  });
});

describe("the relay disclosure", () => {
  it.each(["en", "zh-Hant", "zh-Hans"])("is on screen exactly once, off or on, in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    panel();
    await screen.findByRole("button", { name: i18n.t("remoteAccess.toggleTitle") });
    expect(screen.getAllByText(/wenlan-relay/)).toHaveLength(1);
    cleanup();
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    panel();
    await screen.findByRole("button", { name: i18n.t("remoteAccess.testConnection") });
    expect(screen.getAllByText(/wenlan-relay/)).toHaveLength(1);
  });
});

describe("how often it asks", () => {
  async function settle(rounds = 8) {
    for (let i = 0; i < rounds; i += 1) {
      await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    }
  }
  async function fakeTimerPanel() {
    vi.useFakeTimers();
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    const view = panel();
    await settle();
    expect(screen.getByText("Connected")).toBeInTheDocument();
    return view;
  }
  const calls = () => mocks.listRemoteGrants.mock.calls.length;

  it("refreshes once a minute, not every five seconds", async () => {
    await fakeTimerPanel();
    const initial = calls();
    expect(initial).toBe(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(30_000); });
    expect(calls()).toBe(initial);
    await act(async () => { await vi.advanceTimersByTimeAsync(31_000); });
    await settle();
    expect(calls()).toBe(initial + 1);
  });

  it("looks every five seconds while an app that was just allowed is still connecting, then slows down", async () => {
    await fakeTimerPanel();
    act(() => markPairingApproved());
    await settle();
    const before = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    await settle();
    expect(calls()).toBeGreaterThan(before);
    // The new app shows up: the fast look stops.
    mocks.listRemoteGrants.mockResolvedValue({ items: [grant({ createdAt: Date.now() + 1000 })], cursor: null });
    await act(async () => { await vi.advanceTimersByTimeAsync(5000); });
    await settle();
    expect(screen.getByText("Claude")).toBeInTheDocument();
    const found = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
    expect(calls()).toBe(found);
  });

  it("gives up looking fast after two minutes if the app never connects", async () => {
    await fakeTimerPanel();
    act(() => markPairingApproved());
    await act(async () => { await vi.advanceTimersByTimeAsync(2 * 60_000 + 6000); });
    await settle();
    const later = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(20_000); });
    expect(calls()).toBe(later);
  });

  it("refreshes when the person comes back to the window", async () => {
    await fakeTimerPanel();
    const before = calls();
    await act(async () => { window.dispatchEvent(new Event("focus")); });
    await settle();
    expect(calls()).toBe(before + 1);
  });

  it("does not ask while the window is hidden, and asks once when it is shown again", async () => {
    const show = (visible: boolean) => {
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => (visible ? "visible" : "hidden") });
    };
    await fakeTimerPanel();
    try {
      show(false);
      await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
      await settle();
      const before = calls();
      await act(async () => { await vi.advanceTimersByTimeAsync(3 * 60_000); });
      expect(calls()).toBe(before);
      // A focus event that arrives while hidden is not a return to the window.
      await act(async () => { window.dispatchEvent(new Event("focus")); });
      await settle();
      expect(calls()).toBe(before);
      show(true);
      await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
      await settle();
      expect(calls()).toBe(before + 1);
    } finally {
      delete (document as { visibilityState?: unknown }).visibilityState;
    }
  });

  it("stops after an error until a manual refresh succeeds, then resumes", async () => {
    vi.useFakeTimers();
    mocks.getRemoteAccessProfile.mockResolvedValue(profile);
    mocks.getRemoteAccessStatus.mockResolvedValue(connected);
    mocks.listRemoteGrants.mockRejectedValueOnce(new Error("Remote connection unavailable; retry later"));
    panel();
    await settle();
    expect(screen.getByText("Can't reach Wenlan's web service. Check your internet connection.")).toBeInTheDocument();
    const afterError = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(3 * 60_000); });
    expect(calls()).toBe(afterError);
    fireEvent.click(button("Refresh"));
    await settle();
    expect(calls()).toBeGreaterThan(afterError);
    expect(screen.getByText("No apps connected yet")).toBeInTheDocument();
    const recovered = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(61_000); });
    await settle();
    expect(calls()).toBeGreaterThan(recovered);
  });

  it("stops entirely when Web access disconnects, and on unmount", async () => {
    const { listen } = await import("@tauri-apps/api/event");
    const view = await fakeTimerPanel();
    const handler = (listen as unknown as { mock: { calls: Array<[string, (event: { payload: unknown }) => void]> } }).mock.calls[0][1];
    mocks.getRemoteAccessStatus.mockResolvedValue({ status: "off" });
    await act(async () => { handler({ payload: { status: "off" } }); });
    await settle();
    const afterDisconnect = calls();
    await act(async () => { await vi.advanceTimersByTimeAsync(5 * 60_000); });
    expect(calls()).toBe(afterDisconnect);
    view.unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(5 * 60_000); });
    expect(calls()).toBe(afterDisconnect);
  });
});
