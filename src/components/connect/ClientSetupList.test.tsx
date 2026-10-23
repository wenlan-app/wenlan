// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import { resources } from "../../i18n/resources";

const mocks = vi.hoisted(() => ({
  detectMcpClients: vi.fn(),
  writeMcpConfig: vi.fn(),
  installClientPlugin: vi.fn(),
  installClientSkills: vi.fn(),
  removeRawMcpEntry: vi.fn(),
}));
vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return { ...actual, ...mocks };
});

import ClientSetupList from "./ClientSetupList";
import { NO, YES, unreadable } from "../../test/readings";

const CLIENTS = [
  { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
  { name: "Codex CLI", client_type: "codex_cli", config_path: "~/.codex/config.toml", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
  { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
  { name: "Claude Desktop", client_type: "claude_desktop", config_path: "~/Library/.../config.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
  { name: "Gemini CLI", client_type: "gemini_cli", config_path: "~/.gemini/settings.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
];

function renderList(
  qc: QueryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } }),
  connectedFamilies?: Set<string>,
  seenFamilies?: Set<string>,
  hideWhenComplete = false,
) {
  return render(
    <QueryClientProvider client={qc}>
      <ClientSetupList
        connectedFamilies={connectedFamilies}
        seenFamilies={seenFamilies}
        hideWhenComplete={hideWhenComplete}
      />
    </QueryClientProvider>,
  );
}

/** Finds the row shell (ClientRow's `rounded-xl` wrapper div) for a given
 *  client name, so assertions can be scoped `within` a single card instead
 *  of matching anywhere in the document. */
function rowFor(name: string) {
  return screen.getByText(name).closest("div.rounded-xl") as HTMLElement;
}

async function clickSetUp(name: string) {
  await userEvent.click(within(rowFor(name)).getByRole("button", { name: /^Connect / }));
}

describe("ClientSetupList — one Set up button, two different jobs behind it", () => {
  afterEach(() => Object.values(mocks).forEach((m) => m.mockReset()));
  beforeEach(() => {
    mocks.detectMcpClients.mockResolvedValue(CLIENTS);
    mocks.writeMcpConfig.mockResolvedValue([]);
    mocks.installClientPlugin.mockResolvedValue(undefined);
  });

  it("every detected client gets the same one-click Set up — no slash commands, no copy-a-prompt", async () => {
    renderList();
    const setUps = await screen.findAllByRole("button", { name: /^Connect (?!all)/ });
    expect(setUps).toHaveLength(CLIENTS.length);

    expect(screen.queryByRole("button", { name: "Copy setup prompt" })).not.toBeInTheDocument();
    expect(screen.queryByText("Show terminal commands")).not.toBeInTheDocument();
    expect(screen.queryByText("Advanced")).not.toBeInTheDocument();
    expect(screen.queryByText(/plugin marketplace add/)).not.toBeInTheDocument();
  });

  it("shows loading and retryable scan errors in the compact Settings section", async () => {
    mocks.detectMcpClients.mockReturnValueOnce(new Promise(() => {}));
    const loading = renderList(undefined, undefined, undefined, true);
    expect(loading.container.querySelector("section")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Loading available tools");
    loading.unmount();

    mocks.detectMcpClients.mockRejectedValueOnce(new Error("catalog unavailable"));
    const failed = renderList(undefined, undefined, undefined, true);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load available tools");
    expect(failed.container.querySelector("section")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Claude Code")).toBeInTheDocument();
  });

  it("reports a measured empty catalog without claiming a tool is connected", async () => {
    mocks.detectMcpClients.mockResolvedValue([]);
    renderList();

    expect(await screen.findByText("No tools are available on this device.")).toBeInTheDocument();
    expect(screen.queryByText("Every detected tool is already connected")).not.toBeInTheDocument();
  });

  it("hides the completed compact section after a nonempty catalog is measured", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    const { container } = renderList(undefined, undefined, undefined, true);

    await waitFor(() => expect(container.querySelector("section")).not.toBeInTheDocument());
    expect(screen.queryByText("Add a tool")).not.toBeInTheDocument();
  });

  // The invariant, from the Settings side. Claude Code's and Codex's Wenlan
  // plugins declare their own `mcpServers`, so writing an MCP config on top of
  // installing the plugin would register Wenlan twice.
  it("claude_code installs the plugin and never writes an MCP config", async () => {
    renderList();
    await screen.findByText("Claude Code");
    await clickSetUp("Claude Code");

    expect(mocks.installClientPlugin).toHaveBeenCalledWith("claude_code");
    expect(mocks.writeMcpConfig).not.toHaveBeenCalled();
  });

  it("codex_cli installs the plugin and never writes an MCP config", async () => {
    renderList();
    await screen.findByText("Codex CLI");
    await clickSetUp("Codex CLI");

    expect(mocks.installClientPlugin).toHaveBeenCalledWith("codex_cli");
    expect(mocks.writeMcpConfig).not.toHaveBeenCalled();
  });

  it("a non-plugin client still takes the config-write path", async () => {
    renderList();
    await screen.findByText("Cursor");
    await clickSetUp("Cursor");

    expect(mocks.writeMcpConfig).toHaveBeenCalledWith("cursor");
    expect(mocks.installClientPlugin).not.toHaveBeenCalled();
  });

  it("shipped copy never references .mcpb or .codex-plugin — the DOM and every locale", async () => {
    const { container } = renderList();
    await screen.findByText("Claude Code");
    expect(container.textContent).not.toContain(".mcpb");
    expect(container.textContent).not.toContain(".codex-plugin");

    // en is only one of three shipped locales — scan every connectMatrix
    // string in every locale, not just what happened to render in en.
    for (const [locale, bundle] of Object.entries(resources)) {
      const connectMatrix = (bundle.translation as Record<string, unknown>).connectMatrix as Record<
        string,
        string
      >;
      for (const [key, value] of Object.entries(connectMatrix)) {
        expect(value, `${locale}.connectMatrix.${key}`).not.toContain(".mcpb");
        expect(value, `${locale}.connectMatrix.${key}`).not.toContain(".codex-plugin");
      }
    }
  });

  it("undetected clients get no row; they are named once on the Also works with line", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: NO, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Codex CLI", client_type: "codex_cli", config_path: "~/.codex/config.toml", detected: NO, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    renderList();
    expect(await screen.findByText("Also works with Claude Code, Codex CLI (not installed)")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect Claude Code" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect Codex CLI" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect Cursor" })).toBeInTheDocument();
  });

  it("with nothing installed to add, the line still says what works", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Kiro", client_type: "kiro", config_path: "~/.kiro/settings/mcp.json", detected: NO, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO, has_skills: NO },
    ]);
    renderList();
    expect(await screen.findByText("Also works with Kiro (not installed)")).toBeInTheDocument();
  });

  it("a failed Set up shows the error in the danger-text token, not a raw Tailwind color", async () => {
    mocks.writeMcpConfig.mockRejectedValue(new Error("permission denied"));
    renderList();
    await screen.findByText("Cursor");
    await clickSetUp("Cursor");

    const errorEl = await screen.findByRole("alert");
    expect(errorEl).toHaveTextContent(/permission denied/);
    expect(errorEl).toHaveStyle({ color: "var(--mem-status-danger-text)" });
    expect(errorEl.className).not.toContain("text-red-500");
  });

  it("a failed plugin install surfaces its reason too — the CLI-not-found case", async () => {
    mocks.installClientPlugin.mockRejectedValue(new Error("Codex CLI not found"));
    renderList();
    await screen.findByText("Codex CLI");
    await clickSetUp("Codex CLI");

    expect(await screen.findByRole("alert")).toHaveTextContent(/Codex CLI not found/);
  });

  // Configured clients are already shown in the Connected group above —
  // repeating them here (with nothing left to do) was the duplication the
  // user vetoed. Mixed detected list proves the filter, not just the empty case.
  it("hides already-configured clients — only clients with something left to do render", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    renderList();

    await screen.findByText("Cursor");
    expect(screen.queryByText("Claude Code")).not.toBeInTheDocument();
  });

  it("shows an all-connected note when every configured client is already seen in the roster", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    renderList(new QueryClient({ defaultOptions: { queries: { retry: false } } }), new Set(["claude-code"]));

    expect(await screen.findByText("Every detected tool is already connected")).toBeInTheDocument();
    expect(screen.queryByText(/to finish/)).not.toBeInTheDocument();
  });

  // The restart instruction lives on the tool's own row in the roster above
  // ("Restart X to finish."). Repeating it here made the same sentence appear
  // twice on the page, so this note claims only what was measured.
  it("says configured-but-unseen tools are added, not connected, and does not repeat the restart line", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    renderList();

    expect(await screen.findByText("Every detected tool is added.")).toBeInTheDocument();
    expect(screen.queryByText("Every detected tool is already connected")).not.toBeInTheDocument();
    expect(screen.queryByText(/Restart/)).not.toBeInTheDocument();
    expect(screen.queryByText(/to finish/)).not.toBeInTheDocument();
  });

  it("still says added when only one of the hidden clients has been seen in the roster", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Claude Code", client_type: "claude_code", config_path: "~/.claude.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    renderList(new QueryClient({ defaultOptions: { queries: { retry: false } } }), new Set(["cursor"]));

    expect(await screen.findByText("Every detected tool is added.")).toBeInTheDocument();
    expect(screen.queryByText("Every detected tool is already connected")).not.toBeInTheDocument();
    expect(screen.queryByText(/Restart/)).not.toBeInTheDocument();
  });

  it("never names an undetected client in the note, even when configured", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Gemini CLI", client_type: "gemini_cli", config_path: "~/.gemini/settings.json", detected: NO, already_configured: YES, has_raw_entry: YES, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    renderList();

    expect(await screen.findByText("Every detected tool is added.")).toBeInTheDocument();
    expect(screen.queryByText("Gemini CLI")).not.toBeInTheDocument();
    expect(screen.queryByText(/Cursor/)).not.toBeInTheDocument();
    expect(screen.queryByText("Every detected tool is already connected")).not.toBeInTheDocument();
  });

  // Coherence pass: the roster above is the single source of truth for what
  // is connected. A client whose tool family already has an identity there is
  // hidden here even when its OWN config file reads unconfigured (the live
  // Cursor identities did not carry `already_configured`). Family, not the
  // per-file flag, decides.
  it("hides a detected-but-unconfigured client whose family is already connected, still shows others", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { name: "Cursor", client_type: "cursor", config_path: "~/.cursor/mcp.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
      { name: "Gemini CLI", client_type: "gemini_cli", config_path: "~/.gemini/settings.json", detected: YES, already_configured: NO, has_raw_entry: NO, has_raw_duplicate: NO, has_plugin: NO },
    ]);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <ClientSetupList connectedFamilies={new Set(["cursor"])} />
      </QueryClientProvider>,
    );

    // Gemini's family is not connected → it still offers a Set up button.
    await screen.findByText("Gemini CLI");
    expect(screen.queryByText("Cursor")).not.toBeInTheDocument();
  });
  // ── Round 5, defect 4: a look that failed is not a tool that is absent ──

  // `detect_mcp_clients` used to answer this with a bare `false`, so a config
  // the OS would not hand over and a config that genuinely has no Wenlan entry
  // arrived here as the same value. This row is what that produced.
  it("a detection that could not be made says so - it never says Not installed", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      {
        name: "Cursor",
        client_type: "cursor",
        config_path: "~/.cursor/mcp.json",
        detected: unreadable("Access is denied. (os error 5)"),
        already_configured: NO,
        has_raw_entry: NO,
        has_raw_duplicate: NO,
        has_plugin: NO,
      },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).queryByText("Not installed")).not.toBeInTheDocument();
    expect(within(row).getByText("Could not check")).toBeInTheDocument();
    // The reason reaches the user, not just the fact that something failed.
    expect(within(row).getByText(/Access is denied/)).toBeInTheDocument();
    // And the action survives: withholding Set up would be the same false
    // negative, expressed as a missing button instead of a wrong label.
    expect(within(row).getByRole("button", { name: /^Connect / })).toBeInTheDocument();
  });

  // The hide filter was `!client.already_configured`, which is FALSY-negated to
  // true for a read that failed - so an unreadable config was hidden from the
  // one list whose job is showing what still needs doing, under the heading
  // "Every detected tool is already connected".
  it("a configured-state that could not be read is still listed, marked unknown", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      {
        name: "Claude Code",
        client_type: "claude_code",
        config_path: "~/.claude.json",
        detected: YES,
        already_configured: unreadable("Access is denied. (os error 5)"),
        has_raw_entry: NO,
        has_raw_duplicate: NO,
        has_plugin: NO,
      },
    ]);
    renderList();

    await screen.findByText("Claude Code");
    expect(screen.queryByText("Every detected tool is already connected")).not.toBeInTheDocument();
    const row = rowFor("Claude Code");
    // Regex: the chip appends the reason after the label, and the reason is
    // the half a boolean could never carry.
    expect(within(row).getByText(/Setup state unknown/)).toBeInTheDocument();
    expect(within(row).getByText(/Access is denied/)).toBeInTheDocument();
    // Not the "Added" chip: nothing was read that says so.
    expect(within(row).queryByText("Added")).not.toBeInTheDocument();
  });

  // ── Round 6, D6a ────────────────────────────────────────────────────

  /** A Claude Desktop whose chat-side plugin scan FAILED. `claude_desktop` is
   *  the client this matters for: it has a plugin surface AND its "Set up"
   *  writes a raw `mcpServers` entry, so an unread plugin state plus a write
   *  is the plugin+raw double registration Diagnostics has a warning box for.
   */
  const desktopWithUnreadPlugin = [
    {
      name: "Claude Desktop",
      client_type: "claude_desktop",
      config_path: "~/Library/.../claude_desktop_config.json",
      detected: YES,
      // The config file itself read fine and holds no raw entry…
      has_raw_entry: NO,
      has_raw_duplicate: NO,
      // …but the plugin half could not be read, so the OR is unknown.
      has_plugin: unreadable("Access is denied. (os error 5)"),
      already_configured: unreadable("Access is denied. (os error 5)"),
    },
  ];

  it("a write that could duplicate a registration is offered as a decision, not as the default action", async () => {
    mocks.detectMcpClients.mockResolvedValue(desktopWithUnreadPlugin);
    renderList();

    await screen.findByText("Claude Desktop");
    const row = rowFor("Claude Desktop");

    // SEES: not the same button a measured `no` gets.
    expect(within(row).queryByRole("button", { name: /^Connect / })).not.toBeInTheDocument();
    const button = within(row).getByRole("button", { name: "Set up anyway" });

    // SEES: what could not be read, and what writing anyway would do.
    expect(
      within(row).getByText(/could not check whether this tool is already connected/i),
    ).toBeInTheDocument();
    expect(within(row).getByText(/second registration/i)).toBeInTheDocument();
    // The OS's own words, not just "something went wrong". `getAllByText`:
    // the unknown-state chip carries the same reason, and both should.
    expect(within(row).getAllByText(/Access is denied/).length).toBeGreaterThan(0);

    // ANNOUNCES: the button carries that line, so the risk is read out with
    // the action rather than sitting several nodes away from it.
    const describedBy = button.getAttribute("aria-describedby");
    expect(describedBy).toBe("client-row-desc-claude_desktop");
    expect(document.getElementById(describedBy!)).toHaveTextContent(/second registration/i);

    // CLICKS: still possible. Withholding the action would be the same false
    // negative wearing a different coat — the user may well know the plugin
    // is not installed.
    await userEvent.click(button);
    expect(mocks.writeMcpConfig).toHaveBeenCalledWith("claude_desktop");
  });

  it("a MEASURED plugin absence keeps the ordinary unqualified Set up", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...desktopWithUnreadPlugin[0], has_plugin: NO, already_configured: NO },
    ]);
    renderList();

    await screen.findByText("Claude Desktop");
    const row = rowFor("Claude Desktop");
    expect(within(row).getByRole("button", { name: /^Connect / })).toBeInTheDocument();
    expect(within(row).queryByText(/second registration/i)).not.toBeInTheDocument();
  });

  it("a plugin-install client is never qualified — its route writes no raw entry", async () => {
    // `claude_code` goes through installClientPlugin, which installs the
    // plugin rather than adding a second registration beside it, so an unread
    // plugin state cannot produce a duplicate through this button.
    mocks.detectMcpClients.mockResolvedValue([
      {
        name: "Claude Code",
        client_type: "claude_code",
        config_path: "~/.claude.json",
        detected: YES,
        has_raw_entry: NO,
        has_raw_duplicate: NO,
        has_plugin: unreadable("Access is denied. (os error 5)"),
        already_configured: unreadable("Access is denied. (os error 5)"),
      },
    ]);
    renderList();

    await screen.findByText("Claude Code");
    const row = rowFor("Claude Code");
    expect(within(row).getByRole("button", { name: /^Connect / })).toBeInTheDocument();
    expect(within(row).queryByText(/second registration/i)).not.toBeInTheDocument();
  });

  // ── Round 6, D3's boundary defect, at the pixel ──────────────────────

  it("a write that skipped an input it never determined says so, and still counts as done", async () => {
    mocks.writeMcpConfig.mockResolvedValue([
      {
        input: "WENLAN_MCP_DEV_BIN",
        blocked: "WENLAN_MCP_DEV_BIN",
        error: "environment variable was not valid Unicode",
      },
    ]);
    renderList();

    await screen.findByText("Cursor");
    await clickSetUp("Cursor");

    const row = rowFor("Cursor");
    expect(within(row).getByText(/WENLAN_MCP_DEV_BIN could not be determined/))
      .toBeInTheDocument();
    expect(within(row).getByText(/not valid Unicode/)).toBeInTheDocument();
    // Not an error: the write SUCCEEDED. It is the completeness of the search
    // behind it that is being reported.
    expect(within(row).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("a write with every input determined reports nothing extra", async () => {
    mocks.writeMcpConfig.mockResolvedValue([]);
    renderList();

    await screen.findByText("Cursor");
    await clickSetUp("Cursor");

    const row = rowFor("Cursor");
    expect(within(row).queryByText(/could not be determined/)).not.toBeInTheDocument();
  });

  // ── C2: Added versus Connected, in place ────────────────────────────

  it("a tool added from this list keeps its row and says to restart it", async () => {
    renderList();
    await screen.findByText("Cursor");
    await clickSetUp("Cursor");

    const row = rowFor("Cursor");
    expect(await within(row).findByText("Added")).toBeInTheDocument();
    expect(within(row).getByRole("status")).toHaveTextContent("Added. Restart Cursor to finish.");
    // The switch now reads on, and nothing claims a connection yet.
    expect(within(row).getByRole("button", { name: "Connect Cursor" })).toHaveAttribute("aria-pressed", "true");
    expect(within(row).queryByText("Connected")).not.toBeInTheDocument();
    expect(mocks.writeMcpConfig).toHaveBeenCalledTimes(1);
  });

  it("turning the switch back off removes the entry it just wrote", async () => {
    mocks.removeRawMcpEntry.mockResolvedValue(undefined);
    renderList();
    await screen.findByText("Cursor");
    await clickSetUp("Cursor");
    const toggle = await within(rowFor("Cursor")).findByRole("button", { name: "Connect Cursor" });
    await waitFor(() => expect(toggle).toHaveAttribute("aria-pressed", "true"));
    await userEvent.click(toggle);

    expect(mocks.removeRawMcpEntry).toHaveBeenCalledWith("cursor");
    await waitFor(() =>
      expect(within(rowFor("Cursor")).getByRole("button", { name: "Connect Cursor" })).toHaveAttribute("aria-pressed", "false"),
    );
  });

  it("a plugin tool has no switch to turn back off once added", async () => {
    renderList();
    await screen.findByText("Codex CLI");
    await clickSetUp("Codex CLI");
    const row = rowFor("Codex CLI");
    expect(await within(row).findByText("Added. Restart Codex CLI to finish.")).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /^Connect / })).not.toBeInTheDocument();
  });

  it("Connect all sets up every installed tool one after another", async () => {
    renderList();
    expect(await screen.findByText("Found 5 AI tools to connect")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Connect all" }));

    await waitFor(() => expect(mocks.installClientPlugin).toHaveBeenCalledTimes(2));
    expect(mocks.installClientPlugin).toHaveBeenCalledWith("claude_code");
    expect(mocks.installClientPlugin).toHaveBeenCalledWith("codex_cli");
    expect(mocks.writeMcpConfig.mock.calls.map(([type]) => type).sort()).toEqual(["claude_desktop", "cursor", "gemini_cli"]);
    // Every one is added now, so the offer goes away.
    await waitFor(() => expect(screen.queryByRole("button", { name: "Connect all" })).not.toBeInTheDocument());
  });

  it("one tool left to add needs no Connect all", async () => {
    mocks.detectMcpClients.mockResolvedValue([CLIENTS[2]]);
    renderList();
    await screen.findByRole("button", { name: "Connect Cursor" });
    expect(screen.queryByRole("button", { name: "Connect all" })).not.toBeInTheDocument();
  });

  it("a skills tool says the skills come along, and its switch installs them after the entry", async () => {
    mocks.detectMcpClients.mockResolvedValue([{ ...CLIENTS[2], has_skills: NO }]);
    mocks.installClientSkills.mockResolvedValue("/home/u/.cursor/skills");
    renderList();
    expect(await screen.findByText("Connects and adds skills")).toBeInTheDocument();
    await clickSetUp("Cursor");
    await waitFor(() => expect(mocks.installClientSkills).toHaveBeenCalledWith("cursor"));
    expect(mocks.writeMcpConfig).toHaveBeenCalledWith("cursor");
  });

  it("a tool connected before it had skills keeps a row with Add skills, even when the roster shows it", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...CLIENTS[2], already_configured: YES, has_raw_entry: YES, has_skills: NO },
    ]);
    mocks.installClientSkills.mockResolvedValue("/home/u/.cursor/skills");
    renderList(undefined, new Set(["cursor"]), new Set(["cursor"]));
    expect(await screen.findByText("No skills yet")).toBeInTheDocument();
    await userEvent.click(within(rowFor("Cursor")).getByRole("button", { name: "Add skills" }));
    await waitFor(() => expect(mocks.installClientSkills).toHaveBeenCalledWith("cursor"));
    expect(mocks.writeMcpConfig).not.toHaveBeenCalled();
  });

  it("the same row flips to Connected once the roster has seen the tool", async () => {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const tree = (seen: Set<string>) => (
      <QueryClientProvider client={qc}>
        <ClientSetupList connectedFamilies={new Set()} seenFamilies={seen} />
      </QueryClientProvider>
    );
    const { rerender } = render(tree(new Set()));
    await screen.findByText("Cursor");
    await clickSetUp("Cursor");
    expect(await within(rowFor("Cursor")).findByText("Added")).toBeInTheDocument();

    rerender(tree(new Set(["cursor"])));

    const row = rowFor("Cursor");
    expect(await within(row).findByText("Connected")).toBeInTheDocument();
    expect(within(row).queryByText("Added")).not.toBeInTheDocument();
    expect(within(row).getByRole("status")).toHaveTextContent("Connected. Cursor has reached Wenlan.");
  });

  it("a failed Set up names the tool, classifies the failure, and keeps the raw text underneath", async () => {
    mocks.writeMcpConfig.mockRejectedValue(new Error("permission denied: ~/.cursor/mcp.json"));
    renderList();
    await screen.findByText("Cursor");
    await clickSetUp("Cursor");

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't add Wenlan to Cursor. Wenlan was not allowed to do this.");
    expect(screen.getByTestId("client-row-error-detail-cursor")).toHaveTextContent(
      "permission denied: ~/.cursor/mcp.json",
    );
    // The row stays actionable, so the user can try again.
    expect(within(rowFor("Cursor")).getByRole("button", { name: /^Connect / })).toBeInTheDocument();
  });

  it.each([
    ["zh-Hans", "无法将文澜添加到 Cursor。", "文澜没有权限执行这项操作。", "添加"],
    ["zh-Hant", "無法將文瀾新增到 Cursor。", "文瀾沒有權限執行這項操作。", "新增"],
  ])("a failed Set up is localized in %s, with the raw text kept verbatim", async (lng, sentence, heading) => {
    await i18n.changeLanguage(lng);
    try {
      mocks.writeMcpConfig.mockRejectedValue(new Error("permission denied: ~/.cursor/mcp.json"));
      renderList();
      await screen.findByText("Cursor");
      const row = rowFor("Cursor");
      await userEvent.click(within(row).getAllByRole("button")[0]);

      const alert = await screen.findByRole("alert");
      expect(alert).toHaveTextContent(sentence);
      expect(alert).toHaveTextContent(heading);
      expect(alert).not.toHaveTextContent("Couldn't");
      expect(screen.getByTestId("client-row-error-detail-cursor")).toHaveTextContent(
        "permission denied: ~/.cursor/mcp.json",
      );
    } finally {
      await i18n.changeLanguage("en");
    }
  });
});

// ── C2: the daemon's install and entry checks (C1's optional fields) ──────

const BASE = {
  name: "Cursor",
  client_type: "cursor",
  config_path: "~/.cursor/mcp.json",
  detected: YES,
  already_configured: YES,
  has_raw_entry: YES,
  has_raw_duplicate: NO,
  has_plugin: NO,
};

describe("ClientSetupList — repair and install state", () => {
  afterEach(() => Object.values(mocks).forEach((m) => m.mockReset()));
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    mocks.writeMcpConfig.mockResolvedValue([]);
    mocks.installClientPlugin.mockResolvedValue(undefined);
  });

  const REASONS = [
    ["command_missing", "Wenlan's entry in Cursor has no command to start it."],
    ["command_not_found", "Wenlan's entry in Cursor points to a program that is no longer there."],
    ["command_not_runnable", "Wenlan's entry in Cursor points to a program that can't be run."],
    ["args_invalid", "Wenlan's entry in Cursor has start-up arguments that don't look right."],
  ] as const;

  it.each(REASONS)("needs_repair (%s) shows a localized line, a Repair button, and never the Rust detail", async (reason, line) => {
    mocks.detectMcpClients.mockResolvedValue([
      {
        ...BASE,
        entry_health: { kind: "needs_repair", reason, detail: "RUST-ONLY-ENGLISH-DETAIL /usr/bin/x" },
      },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).getByText(line)).toBeInTheDocument();
    expect(within(row).getByText("Needs repair")).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: "Repair" })).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /^Connect / })).not.toBeInTheDocument();
    // Not "Added": restarting a tool whose entry cannot start fixes nothing.
    expect(within(row).queryByText("Added")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("RUST-ONLY-ENGLISH-DETAIL");
  });

  it("Repair rewrites the entry through write_mcp_config and then asks for a restart", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...BASE, entry_health: { kind: "needs_repair", reason: "command_not_found", detail: "x" } },
    ]);
    renderList();
    await screen.findByText("Cursor");

    await userEvent.click(within(rowFor("Cursor")).getByRole("button", { name: "Repair" }));

    expect(mocks.writeMcpConfig).toHaveBeenCalledWith("cursor");
    expect(mocks.installClientPlugin).not.toHaveBeenCalled();
    const row = rowFor("Cursor");
    expect(await within(row).findByRole("status")).toHaveTextContent("Repaired. Restart Cursor to finish.");
    expect(within(row).queryByRole("button", { name: "Repair" })).not.toBeInTheDocument();
  });

  it("Repair on a plugin client rewrites the existing raw entry and never installs a second plugin", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      {
        ...BASE,
        name: "Claude Code",
        client_type: "claude_code",
        config_path: "~/.claude.json",
        entry_health: { kind: "needs_repair", reason: "command_not_runnable", detail: "x" },
      },
    ]);
    renderList();
    await screen.findByText("Claude Code");

    await userEvent.click(within(rowFor("Claude Code")).getByRole("button", { name: "Repair" }));

    expect(mocks.writeMcpConfig).toHaveBeenCalledWith("claude_code");
    expect(mocks.installClientPlugin).not.toHaveBeenCalled();
  });

  it("a broken entry stays listed even when its family is already connected", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...BASE, entry_health: { kind: "needs_repair", reason: "command_missing", detail: "x" } },
    ]);
    renderList(undefined, new Set(["cursor"]), new Set(["cursor"]));

    await screen.findByText("Cursor");
    expect(screen.queryByText("Every detected tool is already connected")).not.toBeInTheDocument();
    expect(within(rowFor("Cursor")).getByRole("button", { name: "Repair" })).toBeInTheDocument();
  });

  it("a failed Repair says it could not repair, not that it could not add", async () => {
    mocks.writeMcpConfig.mockRejectedValue(new Error("permission denied"));
    mocks.detectMcpClients.mockResolvedValue([
      { ...BASE, entry_health: { kind: "needs_repair", reason: "command_missing", detail: "x" } },
    ]);
    renderList();
    await screen.findByText("Cursor");

    await userEvent.click(within(rowFor("Cursor")).getByRole("button", { name: "Repair" }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Couldn't repair Wenlan in Cursor.");
    expect(alert).not.toHaveTextContent("Couldn't add");
    expect(within(rowFor("Cursor")).getByRole("button", { name: "Repair" })).toBeInTheDocument();
  });

  it("an entry that could not be read says so and offers no Repair", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      {
        ...BASE,
        entry_health: { kind: "unreadable", error: "RUST-ONLY-ENGLISH-ERROR (os error 5)" },
      },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).getByTestId("client-row-entry-unreadable-cursor")).toHaveTextContent(
      "Can't read this tool's settings, so Wenlan can't tell whether its entry works.",
    );
    expect(within(row).queryByRole("button", { name: "Repair" })).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("RUST-ONLY-ENGLISH-ERROR");
  });

  // The chip and the localized line said the same thing, the chip in raw
  // English ("SETUP STATE UNKNOWN · PERMISSION DENIED"). With the line on the
  // row, the chip keeps only its localized label.
  it("an unreadable entry keeps the raw OS detail out of the setup-state chip", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      {
        ...BASE,
        already_configured: unreadable("Permission denied (os error 13)"),
        entry_health: { kind: "unreadable", error: "Permission denied (os error 13)" },
      },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).getByText("Setup state unknown")).toBeInTheDocument();
    expect(within(row).getByTestId("client-row-entry-unreadable-cursor")).toBeInTheDocument();
    expect(document.body.textContent).not.toContain("Permission denied");
    expect(document.body.textContent).not.toContain("os error 13");
  });

  it("without the localized line, the setup-state chip still shows its raw detail", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...BASE, already_configured: unreadable("Permission denied (os error 13)") },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).getByText(/Setup state unknown · Permission denied \(os error 13\)/)).toBeInTheDocument();
    expect(within(row).queryByTestId("client-row-entry-unreadable-cursor")).not.toBeInTheDocument();
  });

  it("config_only does not claim the tool is installed, and Set up is still offered", async () => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...BASE, already_configured: NO, has_raw_entry: NO, install_state: { kind: "config_only" } },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).getByText("Settings found, app not found")).toBeInTheDocument();
    expect(within(row).queryByText(/installed/i)).not.toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /^Connect / })).toBeInTheDocument();
  });

  it.each([
    ["no_entry", { kind: "no_entry" }],
    ["healthy", { kind: "healthy" }],
  ] as const)("entry_health %s adds nothing to the row", async (_name, health) => {
    mocks.detectMcpClients.mockResolvedValue([
      { ...BASE, already_configured: NO, has_raw_entry: NO, entry_health: health, install_state: { kind: "installed" } },
    ]);
    renderList();

    await screen.findByText("Cursor");
    const row = rowFor("Cursor");
    expect(within(row).getByRole("button", { name: /^Connect / })).toBeInTheDocument();
    expect(within(row).queryByText("Needs repair")).not.toBeInTheDocument();
    expect(within(row).queryByText("Settings found, app not found")).not.toBeInTheDocument();
    expect(within(row).queryByTestId("client-row-entry-unreadable-cursor")).not.toBeInTheDocument();
  });

  it("without the fields (an older daemon or a mock) a configured client behaves exactly as before", async () => {
    mocks.detectMcpClients.mockResolvedValue([BASE]);
    renderList();

    expect(await screen.findByText("Every detected tool is added.")).toBeInTheDocument();
    expect(screen.queryByText("Needs repair")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Repair" })).not.toBeInTheDocument();
  });

  it.each([
    ["zh-Hans", "需要修复", "修复", "Cursor 中的文澜配置指向的程序已经不存在。"],
    ["zh-Hant", "需要修復", "修復", "Cursor 中的文瀾設定指向的程式已經不存在。"],
  ])("the repair row is localized in %s", async (lng, chip, button, line) => {
    await i18n.changeLanguage(lng);
    try {
      mocks.detectMcpClients.mockResolvedValue([
        { ...BASE, entry_health: { kind: "needs_repair", reason: "command_not_found", detail: "x" } },
      ]);
      renderList();
      await screen.findByText("Cursor");
      const row = rowFor("Cursor");
      expect(within(row).getByText(chip)).toBeInTheDocument();
      expect(within(row).getByRole("button", { name: button })).toBeInTheDocument();
      expect(within(row).getByText(line)).toBeInTheDocument();
    } finally {
      await i18n.changeLanguage("en");
    }
  });
});
