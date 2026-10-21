// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor, fireEvent, act, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import DiagnosticsSection from "./DiagnosticsSection";
import {
  getActivity,
  getWireState,
  removeLegacyMcpEntry,
  removeRawMcpEntry,
  setSetupCompleted,
  startDaemonSidecar,
  type ActivityAssetKind,
  type ActivityAssetStatus,
  type ActivityResponse,
  type ActivityStep,
  type ActivityStepName,
  type WireState,
} from "../../../../lib/tauri";
import { i18n } from "../../../../i18n";
import { NO, YES, unreadable } from "../../../../test/readings";

vi.mock("../../../../lib/tauri", () => ({
  getActivity: vi.fn(),
  getWireState: vi.fn(),
  removeLegacyMcpEntry: vi.fn().mockResolvedValue(undefined),
  removeRawMcpEntry: vi.fn().mockResolvedValue(undefined),
  setSetupCompleted: vi.fn().mockResolvedValue(undefined),
  startDaemonSidecar: vi.fn(),
}));

function renderDiagnosticsOverview(props: {
  onSetupAgent?: () => void;
  onOpenConnections?: () => void;
  onOpenIntelligence?: () => void;
  onOpenActivity?: () => void;
} = {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  }
  return render(<DiagnosticsSection {...props} />, { wrapper: Wrapper });
}

function renderDiagnostics(onSetupAgent?: () => void) {
  const rendered = renderDiagnosticsOverview({ onSetupAgent });
  fireEvent.click(screen.getByTestId("diagnostics-details-toggle"));
  return rendered;
}

const wireFixture: WireState = {
  daemon: {
    base_url: "http://127.0.0.1:7878",
    reachable: true,
    version: "0.12.3",
    error: null,
    sidecar_spawned_on_unknown_owner: false,
  },
  mcp_binary: {
    command: "wenlan-mcp",
    args: ["--stdio"],
    undetermined: [],
    candidates: [
      {
        path: "/Users/x/.wenlan/bin/wenlan-mcp",
        state: { kind: "file" },
        source: "installed",
      },
      {
        path: "/Users/x/Repos/wenlan/target/release/wenlan-mcp",
        state: { kind: "absent" },
        source: "cargo",
      },
    ],
  },
  clients: [
    {
      client_type: "claude_code",
      name: "Claude Code",
      detected: YES,
      config_path: "/Users/x/.claude.json",
      has_raw_entry: NO,
      has_raw_duplicate: NO,
      has_plugin: YES,
      route: "plugin",
    },
    {
      client_type: "claude_desktop",
      name: "Claude Desktop",
      detected: YES,
      config_path: "/Users/x/Library/Application Support/Claude/claude_desktop_config.json",
      has_raw_entry: YES,
      has_raw_duplicate: NO,
      has_plugin: YES,
      route: "plugin",
    },
  ],
};

function step(name: ActivityStepName, fields: Partial<ActivityStep> = {}): ActivityStep {
  return { name, state: "idle", done: 0, total: 0, failed: 0, job: null, ...fields };
}

function asset(kind: ActivityAssetKind, fields: Partial<ActivityAssetStatus> = {}): ActivityAssetStatus {
  return { kind, state: "idle", done: 0, total: 0, blocked: 0, steps: [], ...fields };
}

/**
 * Memories and Entities count different things on purpose: 143 memories were
 * scanned for entities, 21 entities were found, and 93 memories wait on the
 * everyday model. The Entities headline `done`/`total` follow Detect (memories),
 * which is exactly the number the card must not print beside "Entities".
 */
function activity(fields: Partial<ActivityResponse> = {}): ActivityResponse {
  return {
    state: "organizing",
    last_activity_at: null,
    assets: [
      asset("memories", {
        state: "running",
        done: 140,
        total: 143,
        steps: [
          step("store", { done: 143, total: 143 }),
          step("summarize", { state: "running", done: 140, total: 143, failed: 3, job: "everyday" }),
          step("link", { done: 143, total: 143, job: "everyday" }),
        ],
      }),
      asset("entities", {
        state: "blocked",
        done: 50,
        total: 143,
        blocked: 93,
        steps: [
          step("detect", { state: "blocked", done: 50, total: 143, job: "everyday" }),
          step("confirm", { done: 9, total: 21 }),
        ],
      }),
      asset("pages", { done: 12, total: 12, steps: [step("write", { done: 12, total: 12, job: "synthesis" })] }),
    ],
    everyday: { job: "everyday", lane: "on_device", model: "qwen3-8b", mode: "pinned", available: true },
    synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
    refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
    ...fields,
  };
}

describe("DiagnosticsSection", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getWireState).mockResolvedValue(wireFixture);
    vi.mocked(getActivity).mockResolvedValue(activity());
  });

  describe("compact overview", () => {
    it("shows three concise checks without paths, versions, routine counts, or detail tables", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        clients: wireFixture.clients.map((client) => ({ ...client, has_raw_entry: NO })),
      });
      renderDiagnosticsOverview();

      const overview = await screen.findByTestId("diagnostics-overview");
      expect(await within(overview).findByTestId("diagnostics-overview-service")).toHaveTextContent("Responding");
      expect(within(overview).getByTestId("diagnostics-overview-integration")).toHaveTextContent("AI tool detected");
      expect(within(overview).getByTestId("diagnostics-overview-background")).toHaveTextContent("Steeping");
      expect(within(overview).queryByText(/\/Users\/x/)).not.toBeInTheDocument();
      expect(within(overview).queryByText("0 memories")).not.toBeInTheDocument();
      expect(within(overview).queryByTestId("diagnostics-asset-memories")).not.toBeInTheDocument();
      expect(within(overview).queryByText("Version 0.12.3")).not.toBeInTheDocument();
      expect(within(overview).queryByText("Action")).not.toBeInTheDocument();
    });

    it("keeps binary-search and client-read uncertainty visible even when a candidate was found", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        mcp_binary: {
          ...wireFixture.mcp_binary,
          undetermined: [{ input: "the home directory", blocked: "installed", error: "permission denied" }],
        },
        clients: [
          {
            ...wireFixture.clients[0],
            detected: unreadable("Access is denied."),
          },
        ],
      });
      vi.mocked(getActivity).mockResolvedValue(activity({ state: "unknown" }));
      renderDiagnosticsOverview();

      const integration = await screen.findByTestId("diagnostics-overview-integration");
      await screen.findByText("Some possible binary locations could not be checked.");
      expect(integration).toHaveTextContent("Some possible binary locations could not be checked.");
      expect(integration).toHaveTextContent("Some client configuration checks could not be read.");
      expect(integration).toHaveTextContent("Unknown");
      expect(await screen.findByTestId("diagnostics-overview-background")).toHaveTextContent("Some background-work details could not be determined.");
    });

    it("places connections, activity, and intelligence navigation only when callbacks are supplied", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        clients: [
          { ...wireFixture.clients[0], has_raw_entry: YES },
        ],
      });
      vi.mocked(getActivity).mockResolvedValue(activity({
        state: "blocked",
        everyday: { ...activity().everyday, mode: "pinned_unavailable", available: false },
      }));
      const onOpenConnections = vi.fn();
      const onOpenIntelligence = vi.fn();
      const onOpenActivity = vi.fn();
      renderDiagnosticsOverview({ onOpenConnections, onOpenIntelligence, onOpenActivity });

      fireEvent.click(await screen.findByRole("button", { name: "Connections" }));
      fireEvent.click(screen.getByRole("button", { name: "View Activity" }));
      fireEvent.click(screen.getByRole("button", { name: "Intelligence settings" }));
      expect(onOpenConnections).toHaveBeenCalledTimes(1);
      expect(onOpenActivity).toHaveBeenCalledTimes(1);
      expect(onOpenIntelligence).toHaveBeenCalledTimes(1);

      const noCallbacks = renderDiagnosticsOverview();
      expect(within(noCallbacks.container).queryByRole("button", { name: "Connections" })).not.toBeInTheDocument();
      expect(within(noCallbacks.container).queryByRole("button", { name: "View Activity" })).not.toBeInTheDocument();
      expect(within(noCallbacks.container).queryByRole("button", { name: "Intelligence settings" })).not.toBeInTheDocument();
    });
  });

  describe("background work summary", () => {
    it("summarizes failures and blocked work, then links to Activity", async () => {
      const onOpenActivity = vi.fn();
      renderDiagnosticsOverview({ onOpenActivity });
      fireEvent.click(screen.getByTestId("diagnostics-details-toggle"));

      const summary = await screen.findByTestId("diagnostics-activity-summary");
      expect(summary).toHaveTextContent("Steeping");
      expect(summary).toHaveTextContent("Nothing has run yet");
      expect(summary).toHaveTextContent("Some background work failed.");
      expect(summary).toHaveTextContent("Some background work is blocked.");
      expect(summary).not.toHaveTextContent("Summarize");
      expect(summary).not.toHaveTextContent("143 memories");
      fireEvent.click(within(summary).getByRole("button", { name: "View Activity" }));
      expect(onOpenActivity).toHaveBeenCalledTimes(1);
    });

    it("reports On demand as a mode, not as blocked or off", async () => {
      vi.mocked(getActivity).mockResolvedValue(activity({
        state: "off",
        assets: [
          asset("memories", { done: 143, total: 143, steps: [step("store", { done: 143, total: 143 })] }),
          asset("pages", {
            state: "blocked",
            done: 9,
            total: 12,
            blocked: 3,
            steps: [step("write", { state: "blocked", done: 9, total: 12, job: "synthesis" })],
          }),
        ],
        everyday: { ...activity().everyday, mode: "pinned_unavailable", available: false },
      }));
      renderDiagnostics();

      const summary = await screen.findByTestId("diagnostics-activity-summary");
      expect(summary).toHaveTextContent("On demand");
      expect(summary).toHaveTextContent("3 pages still need steeping");
      expect(summary).not.toHaveTextContent("Off");
      expect(summary).not.toHaveTextContent("Some background work is blocked.");
      expect(summary).not.toHaveTextContent("The pinned everyday model is unavailable.");
      expect(within(summary).getByText("On demand").closest("span[class*='status-danger']")).toBeNull();
    });

    it("preserves unknown and unavailable-model warnings in the compact summary", async () => {
      vi.mocked(getActivity).mockResolvedValue(activity({
        state: "unknown",
        assets: [asset("future_asset" as ActivityAssetKind, { state: "future_state" as ActivityAssetStatus["state"] })],
        everyday: { ...activity().everyday, mode: "pinned_unavailable", available: false },
      }));
      renderDiagnostics();

      const summary = await screen.findByTestId("diagnostics-activity-summary");
      expect(summary).toHaveTextContent("Some background-work details could not be determined.");
      expect(summary).toHaveTextContent("The pinned everyday model is unavailable.");
      expect(summary).toHaveTextContent("Unknown");
    });

    it("keeps an unset model muted and a chosen model that cannot run red", async () => {
      vi.mocked(getActivity).mockResolvedValue(
        activity({
          everyday: { job: "everyday", lane: "basic", model: null, mode: "unconfigured", available: false },
          synthesis: { job: "synthesis", lane: "external", model: "gpt-x", mode: "pinned_unavailable", available: false },
        }),
      );

      renderDiagnostics();

      const everyday = await screen.findByTestId("diagnostics-route-everyday");
      const unset = within(everyday).getByText("Not available").closest("[aria-live]");
      expect(unset?.className).toContain("text-[var(--mem-text-tertiary)]");
      expect(unset?.className).not.toContain("danger");
      const synthesis = screen.getByTestId("diagnostics-route-synthesis");
      const broken = within(synthesis).getByText("Not available").closest("[aria-live]");
      expect(broken?.className).toContain("text-[var(--mem-status-danger-text)]");
    });

    it("renders readable suggestion labels and preserves unknown identifiers", async () => {
      vi.mocked(getActivity).mockResolvedValue(
        activity({
          refinement: {
            ready_for_review: 2,
            not_ready: 5,
            groups: [
              { action: "detect_contradiction", status: "awaiting_review", count: 2 },
              { action: "entity_merge", status: "pending", count: 5 },
              { action: "future_action", status: "future_status", count: 1 },
            ],
          },
        }),
      );

      renderDiagnostics();

      const suggestions = await screen.findByTestId("diagnostics-suggestions");
      expect(screen.getByText("Suggestions")).toBeInTheDocument();
      expect(within(suggestions).getByTestId("diagnostics-suggestions-ready")).toHaveTextContent("Ready for review2");
      expect(within(suggestions).getByTestId("diagnostics-suggestions-not-ready")).toHaveTextContent(
        "Not yet ready for review5",
      );
      expect(within(suggestions).getByRole("columnheader", { name: "Action" })).toBeInTheDocument();
      expect(within(suggestions).getByRole("columnheader", { name: "Status" })).toBeInTheDocument();
      expect(within(suggestions).getByRole("columnheader", { name: "Count" })).toBeInTheDocument();
      const groups = within(suggestions).getAllByTestId("diagnostics-suggestion-group");
      expect(groups.map((row) => row.textContent)).toEqual([
        "Contradictionawaiting review2",
        "Entity mergePending5",
        "Other action: future_actionOther status: future_status1",
      ]);
      expect(within(suggestions).queryByText("No open suggestions.")).not.toBeInTheDocument();
    });

    it("says there are no open suggestions when none are open", async () => {
      renderDiagnostics();

      const suggestions = await screen.findByTestId("diagnostics-suggestions");
      expect(within(suggestions).getByText("No open suggestions.")).toBeInTheDocument();
      expect(within(suggestions).queryByRole("table")).not.toBeInTheDocument();
      expect(within(suggestions).getByTestId("diagnostics-suggestions-ready")).toHaveTextContent("Ready for review0");
    });

    it("says it needs a newer daemon, with no Retry, when the activity route is missing", async () => {
      // A Tauri command rejects with the bare string the Rust client formats.
      vi.mocked(getActivity).mockRejectedValue("HTTP GET /api/activity returned 404 Not Found");

      renderDiagnostics();

      expect(await screen.findByText("Diagnostics require a newer daemon")).toBeInTheDocument();
      expect(screen.queryByText("Diagnostics unavailable")).not.toBeInTheDocument();
      expect(screen.queryByText("Retry")).not.toBeInTheDocument();
    });

    it("offers a Retry that reads activity again on any other error", async () => {
      vi.mocked(getActivity).mockRejectedValue("HTTP GET /api/activity returned 500 Internal Server Error");

      renderDiagnostics();

      expect(await screen.findByText("Diagnostics unavailable")).toBeInTheDocument();
      expect(screen.queryByText("Diagnostics require a newer daemon")).not.toBeInTheDocument();
      const callsBefore = vi.mocked(getActivity).mock.calls.length;
      fireEvent.click(screen.getByText("Retry"));

      await waitFor(() => expect(vi.mocked(getActivity).mock.calls.length).toBeGreaterThan(callsBefore));
    });

    it("Refresh reads activity again", async () => {
      renderDiagnostics();

      await screen.findByTestId("diagnostics-activity-summary");
      const callsBefore = vi.mocked(getActivity).mock.calls.length;
      fireEvent.click(screen.getByText("Refresh checks"));

      await waitFor(() => expect(vi.mocked(getActivity).mock.calls.length).toBeGreaterThan(callsBefore));
    });

    it("qualifies retained activity as the last available result after a failed refresh", async () => {
      vi.mocked(getActivity)
        .mockResolvedValueOnce(activity())
        .mockRejectedValueOnce(new Error("IPC failed"));

      renderDiagnostics();
      await screen.findByTestId("diagnostics-state");

      fireEvent.click(screen.getByText("Refresh checks"));

      expect(
        await screen.findByText("The latest check failed (IPC failed); showing the last available result."),
      ).toBeInTheDocument();
      expect(screen.getByTestId("diagnostics-state")).toHaveTextContent("Steeping");
    });

    it("does not expose the manual steep maintenance action", async () => {
      renderDiagnostics();

      await waitFor(() => expect(getActivity).toHaveBeenCalled());
      expect(screen.queryByText("Run maintenance")).not.toBeInTheDocument();
      expect(screen.queryByText("Steep")).not.toBeInTheDocument();
    });

    it("still renders when the wiring query rejects", async () => {
      vi.mocked(getWireState).mockRejectedValue(new Error("IPC failure"));

      renderDiagnostics();

      // Await the summary because the section heading exists before its query resolves.
      expect(await screen.findByTestId("diagnostics-activity-summary")).toBeInTheDocument();
      expect(await screen.findAllByText("Wiring information unavailable")).toHaveLength(3);
    });

    it("Start Wenlan respawns the daemon, then re-probes the wiring", async () => {
      // Daemon down ⇒ WiringError. Retry alone can't heal it; Start must invoke
      // the respawn command AND re-probe (refetch) so a recovered daemon shows.
      vi.mocked(getWireState).mockRejectedValue(new Error("connection refused"));
      vi.mocked(startDaemonSidecar).mockResolvedValue({ status: "started" });

      renderDiagnostics();
      await screen.findAllByText("Wiring information unavailable");
      const callsBefore = vi.mocked(getWireState).mock.calls.length;

      fireEvent.click(screen.getByText("Start Wenlan"));

      await waitFor(() => expect(startDaemonSidecar).toHaveBeenCalledTimes(1));
      // A refetch of the wire query is the whole point — a bare invoke that
      // never re-probes would leave the red frozen.
      await waitFor(() =>
        expect(vi.mocked(getWireState).mock.calls.length).toBeGreaterThan(callsBefore),
      );
    });

    it("surfaces a start failure inline, without re-probing", async () => {
      vi.mocked(getWireState).mockRejectedValue(new Error("connection refused"));
      vi.mocked(startDaemonSidecar).mockResolvedValue({
        status: "failed",
        message: "sidecar quarantined",
      });

      renderDiagnostics();
      await screen.findAllByText("Wiring information unavailable");
      const callsBefore = vi.mocked(getWireState).mock.calls.length;

      fireEvent.click(screen.getByText("Start Wenlan"));

      expect(
        await screen.findByText("Couldn't start Wenlan: sidecar quarantined"),
      ).toBeInTheDocument();
      // A failed start must not claim to have re-probed.
      expect(vi.mocked(getWireState).mock.calls.length).toBe(callsBefore);
    });
  });

  describe("wiring card", () => {
    it("renders the daemon, MCP binary, and clients groups", async () => {
      renderDiagnostics();

      expect(await screen.findByText("Reachable")).toBeInTheDocument();
      expect(screen.getByText("Reachable")).toBeInTheDocument();
      expect(screen.getByText("http://127.0.0.1:7878")).toBeInTheDocument();
      expect(screen.getByText("Version 0.12.3")).toBeInTheDocument();

      expect(screen.getByText("MCP server binary")).toBeInTheDocument();
      expect(screen.getByText("wenlan-mcp --stdio")).toBeInTheDocument();
      expect(screen.getByText("/Users/x/.wenlan/bin/wenlan-mcp")).toBeInTheDocument();
      expect(screen.getByText("/Users/x/Repos/wenlan/target/release/wenlan-mcp")).toBeInTheDocument();

      expect(screen.getByText("AI-tool integration")).toBeInTheDocument();
      expect(screen.getByText("Claude Code")).toBeInTheDocument();
      expect(screen.getByText("Claude Desktop")).toBeInTheDocument();
    });

    it("still renders when the activity query rejects", async () => {
      vi.mocked(getActivity).mockRejectedValue(new Error("boom"));

      renderDiagnostics();

      expect(await screen.findByText("Reachable")).toBeInTheDocument();
      expect(screen.getByText("Reachable")).toBeInTheDocument();
      expect(await screen.findByText("Diagnostics unavailable")).toBeInTheDocument();
    });

    it("shows its own loading state independent of the background work card", async () => {
      let resolveWire!: (value: WireState) => void;
      vi.mocked(getWireState).mockReturnValue(
        new Promise((resolve) => {
          resolveWire = resolve;
        }),
      );

      renderDiagnostics();

      expect(await screen.findByText("Loading wiring…")).toBeInTheDocument();
      // Background work card resolved already; wiring card is still loading independently.
      expect(await screen.findByText("Background work")).toBeInTheDocument();
      expect(await screen.findByTestId("diagnostics-activity-summary")).toBeInTheDocument();

      resolveWire(wireFixture);
      expect(await screen.findByText("Reachable")).toBeInTheDocument();
    });

    it("renders the empty clients state", async () => {
      vi.mocked(getWireState).mockResolvedValue({ ...wireFixture, clients: [] });

      renderDiagnostics();

      expect(await screen.findByText("Reachable")).toBeInTheDocument();
      expect(screen.getByText("No MCP clients found.")).toBeInTheDocument();
    });

  });

  describe("wiring loading state", () => {
    it("shows the loading state while the wire state is pending, then the resolved rows", async () => {
      let resolveWire!: (value: WireState) => void;
      vi.mocked(getWireState).mockReturnValue(
        new Promise((resolve) => {
          resolveWire = resolve;
        }),
      );

      renderDiagnostics();

      // While in flight, only the loading state is on screen. An
      // instant-resolve mock would skip straight past this, so the promise
      // is held open on purpose.
      await screen.findByText("Loading wiring…");
      expect(screen.queryByText("Reachable")).not.toBeInTheDocument();

      await act(async () => {
        resolveWire(wireFixture);
      });

      // Resolved: the real rows render and the loading state is gone.
      expect(await screen.findByText("Reachable")).toBeInTheDocument();
      expect(screen.queryByText("Loading wiring…")).not.toBeInTheDocument();
    });
  });

  // ── Mutation-proof: the three properties this card exists to guarantee ──

  describe("mutation-proof properties", () => {
    it("PROPERTY 1: a missing MCP binary candidate renders as missing, not found", async () => {
      renderDiagnostics();

      await screen.findByText("Reachable");
      expect(screen.getAllByText("Found")).toHaveLength(1);
      expect(screen.getAllByText("Missing")).toHaveLength(1);
    });

    it("PROPERTY 2: double registration (plugin + raw entry) is flagged for the affected client only", async () => {
      renderDiagnostics();

      await screen.findByText("Reachable");
      expect(
        screen.getByText(
          "Wenlan is registered twice for Claude Desktop. Remove the manual MCP entry. Wenlan is already connected automatically.",
        ),
      ).toBeInTheDocument();
      expect(screen.queryByText(/registered twice for Claude Code/)).not.toBeInTheDocument();
    });

    it("PROPERTY 3: an unreachable daemon shows its error, not a reachable state", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        daemon: {
          base_url: "http://127.0.0.1:7878",
          reachable: false,
          version: null,
          error: "connection refused",
          sidecar_spawned_on_unknown_owner: false,
        },
      });

      renderDiagnostics();

      await screen.findByText("Unreachable");
      expect(screen.getByText("Unreachable")).toBeInTheDocument();
      expect(screen.queryByText("Reachable")).not.toBeInTheDocument();
      expect(screen.getByText("connection refused")).toBeInTheDocument();
    });

    it("PROPERTY 4: the raw route tag is never rendered — Wenlan is never called a plugin on screen", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        clients: [
          { ...wireFixture.clients[0], route: "plugin" },
          {
            ...wireFixture.clients[1],
            client_type: "cursor",
            name: "Cursor",
            config_path: "/Users/x/.cursor/mcp.json",
            has_plugin: NO,
            has_raw_entry: NO,
            has_raw_duplicate: NO,
            route: "config",
          },
        ],
      });

      renderDiagnostics();

      await screen.findByText("Reachable");
      expect(screen.getByText("Connects automatically")).toBeInTheDocument();
      expect(screen.getByText("Sets up an MCP entry")).toBeInTheDocument();

      // `route` arrives from Rust as the bare tag "plugin". The i18n banned-word
      // guard only scans resources.ts, so it cannot catch a value rendered raw
      // from the backend — this is the only thing standing between that tag and
      // the screen.
      const wiringCard = screen.getByText("MCP server binary").closest("section");
      expect(wiringCard?.textContent).not.toMatch(/plugin/i);
    });

    it("PROPERTY 5: a raw+raw duplicate is flagged on a no-plugin client, never on a plugin client", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        clients: [
          // Cursor: no plugin, both `wenlan` and legacy `origin` raw entries.
          {
            client_type: "cursor",
            name: "Cursor",
            detected: YES,
            config_path: "/Users/x/.cursor/mcp.json",
            has_raw_entry: YES,
            has_raw_duplicate: YES,
            has_plugin: NO,
            route: "config",
          },
          // Claude Code: the plugin AND a raw duplicate. The plugin+raw box
          // owns this case (its fix removes both raw entries, plugin remains),
          // so the raw+raw box — which keeps `wenlan` — must NOT also fire.
          {
            client_type: "claude_code",
            name: "Claude Code",
            detected: YES,
            config_path: "/Users/x/.claude.json",
            has_raw_entry: YES,
            has_raw_duplicate: YES,
            has_plugin: YES,
            route: "plugin",
          },
        ],
      });

      renderDiagnostics();

      await screen.findByText("Reachable");
      expect(
        screen.getByText(
          "Cursor's config lists Wenlan twice: as wenlan and under its old name origin. Cursor starts two copies until the old entry is removed.",
        ),
      ).toBeInTheDocument();
      // The `!has_plugin` gate: the plugin client never shows the raw+raw box.
      expect(
        screen.queryByText(/Claude Code's config lists Wenlan twice/),
      ).not.toBeInTheDocument();
    });
  });

  // ── Every red carries a fix or a Retry ─────────────────────────────────

  describe("actionable wiring reds", () => {
    it("the double-registration fix removes the raw entry for that client and refreshes wiring", async () => {
      renderDiagnostics();

      await screen.findByText("Reachable");
      const callsBefore = vi.mocked(getWireState).mock.calls.length;

      // Claude Desktop is the double-registered client in the fixture
      // (has_plugin && has_raw_entry); Claude Code is not.
      fireEvent.click(screen.getByText("Remove duplicate entry"));

      await waitFor(() => expect(removeRawMcpEntry).toHaveBeenCalledWith("claude_desktop"));
      // onSuccess invalidates ["wireState"], which refetches the wire query.
      await waitFor(() =>
        expect(vi.mocked(getWireState).mock.calls.length).toBeGreaterThan(callsBefore),
      );
    });

    it("the raw+raw fix removes only the legacy origin entry (keeps wenlan) and refreshes wiring", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        clients: [
          {
            client_type: "cursor",
            name: "Cursor",
            detected: YES,
            config_path: "/Users/x/.cursor/mcp.json",
            has_raw_entry: YES,
            has_raw_duplicate: YES,
            has_plugin: NO,
            route: "config",
          },
        ],
      });

      renderDiagnostics();

      await screen.findByText("Reachable");
      const callsBefore = vi.mocked(getWireState).mock.calls.length;

      fireEvent.click(screen.getByText("Remove the old entry"));

      await waitFor(() => expect(removeLegacyMcpEntry).toHaveBeenCalledWith("cursor"));
      // Headline (b): the raw+raw fix must be removeLegacyMcpEntry — NOT
      // removeRawMcpEntry, which would delete the live `wenlan` entry too and
      // sever Cursor's only connection.
      expect(removeRawMcpEntry).not.toHaveBeenCalled();
      await waitFor(() =>
        expect(vi.mocked(getWireState).mock.calls.length).toBeGreaterThan(callsBefore),
      );
    });

    it("offers a Retry that refetches the wire state when the daemon is unreachable", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        daemon: {
          base_url: "http://127.0.0.1:7878",
          reachable: false,
          version: null,
          error: "connection refused",
          sidecar_spawned_on_unknown_owner: false,
        },
      });

      renderDiagnostics();

      await screen.findByText("Unreachable");
      const callsBefore = vi.mocked(getWireState).mock.calls.length;
      fireEvent.click(screen.getByText("Retry"));

      await waitFor(() =>
        expect(vi.mocked(getWireState).mock.calls.length).toBeGreaterThan(callsBefore),
      );
    });

    it("does not offer tool setup while an MCP binary candidate still exists", async () => {
      renderDiagnostics(vi.fn());

      // Resolve the wiring rows first, then assert absence (an absence assertion
      // made before the rows render would pass vacuously).
      await screen.findByText("Reachable");
      await screen.findByText("MCP server binary");
      expect(screen.queryByRole("button", { name: "Open tool setup" })).not.toBeInTheDocument();
    });

    it("with every MCP binary candidate missing, opens tool setup once without resetting onboarding", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        mcp_binary: {
          ...wireFixture.mcp_binary,
          candidates: [
            {
              path: "/Users/x/.wenlan/bin/wenlan-mcp",
              state: { kind: "absent" },
              source: "installed",
            },
            {
              path: "/Users/x/.cargo/bin/wenlan-mcp",
              state: { kind: "absent" },
              source: "cargo",
            },
          ],
        },
      });

      const onSetupAgent = vi.fn();
      renderDiagnostics(onSetupAgent);

      fireEvent.click(await screen.findByRole("button", { name: "Open tool setup" }));

      expect(onSetupAgent).toHaveBeenCalledTimes(1);
      expect(setSetupCompleted).not.toHaveBeenCalled();
      expect(screen.queryByRole("button", { name: "Confirm" })).not.toBeInTheDocument();
    });

    it("with missing MCP binary candidates and no setup callback, offers no unavailable action", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        mcp_binary: {
          ...wireFixture.mcp_binary,
          candidates: wireFixture.mcp_binary.candidates.map((candidate) => ({
            ...candidate,
            state: { kind: "absent" },
          })),
        },
      });

      renderDiagnostics();

      await screen.findByText("MCP server binary");
      expect(screen.queryByRole("button", { name: "Open tool setup" })).not.toBeInTheDocument();
      expect(setSetupCompleted).not.toHaveBeenCalled();
    });

    // Round 4, defect F, on the UI. `candidate.exists` was a boolean that read
    // `false` for "absent" AND for "the OS refused to look", and this panel
    // rendered both as "Missing" — then offered "Run setup again" as the fix.
    // Reinstalling is not the fix for a permission problem, and calling an
    // unread path missing is the shipped conflation, rendered.
    it("shows an unreadable candidate as unreadable, and does not offer missing-binary tool setup", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        mcp_binary: {
          command: null,
          args: [],
          unresolved: {
            message: "Could not determine the wenlan-mcp binary: nothing was written.",
            unreadable: [
              { path: "/Users/x/.wenlan/bin/wenlan-mcp", error: "Access is denied. (os error 5)" },
            ],
          },
          undetermined: [],
          candidates: [
            {
              path: "/Users/x/.wenlan/bin/wenlan-mcp",
              state: { kind: "unreadable", error: "Access is denied. (os error 5)" },
              source: "installed",
            },
            {
              path: "/Users/x/.cargo/bin/wenlan-mcp",
              state: { kind: "absent" },
              source: "cargo",
            },
          ],
        },
      });

      renderDiagnostics(vi.fn());

      expect(await screen.findByText("Unreadable")).toBeInTheDocument();
      expect(screen.getByText("Missing")).toBeInTheDocument();
      expect(screen.getByText(/Unreadable: Access is denied\. \(os error 5\)/)).toBeInTheDocument();
      expect(
        screen.getByText("Could not determine the wenlan-mcp binary: nothing was written."),
      ).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Open tool setup" })).not.toBeInTheDocument();
    });

    // C1.4 on the UI. An input that could not be determined produces NO
    // candidate row at all — its paths were never built — so a panel that only
    // renders `candidates` shows a short, clean list and gives the user no
    // reason for it. Worse, the short list reads as a completed search and
    // re-offers "Run setup again", which reinstalls a binary that may well be
    // sitting exactly where it should.
    it("shows an input that could not be determined, and does not offer missing-binary tool setup", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        mcp_binary: {
          command: null,
          args: [],
          unresolved: {
            message: "Could not determine the wenlan-mcp binary: nothing was written.",
            unreadable: [],
          },
          undetermined: [
            {
              input: "the home directory",
              blocked: "installed and cargo",
              error: "the platform would not report a home directory",
            },
          ],
          candidates: [],
        },
      });

      renderDiagnostics(vi.fn());

      expect(await screen.findByText(/Not checked/)).toBeInTheDocument();
      expect(
        screen.getByText(/the home directory could not be determined/),
      ).toBeInTheDocument();
      expect(screen.getByText(/installed and cargo/)).toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "Open tool setup" })).not.toBeInTheDocument();
    });

    // C1.7. These three fields reached `DaemonWire` and stopped there:
    // `daemon_start` maps `Spawn` and `SpawnOnUnknownOwner` to the same
    // `Started` result, so the difference was recorded and never rendered.
    // Each one is about a daemon that outlives or duplicates the one the user
    // thinks they have, and nothing else in the app shows any of them.
    it("renders the sidecar facts a Started result cannot distinguish", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        daemon: {
          ...wireFixture.daemon,
          sidecar_job_binding: { state: "unbound", reason: "job assignment refused" },
          sidecar_spawned_on_unknown_owner: true,
          last_sidecar_stop: { outcome: "could_not_measure", reason: "pid identity was lost" },
        },
      });

      renderDiagnostics();

      expect(await screen.findByText("Survives a hard kill")).toBeInTheDocument();
      expect(screen.getByText("job assignment refused")).toBeInTheDocument();
      expect(screen.getByText("Last stop: could not confirm it ended")).toBeInTheDocument();
      expect(screen.getByText("pid identity was lost")).toBeInTheDocument();
      expect(
        screen.getByText(/two copies may be running/),
      ).toBeInTheDocument();
    });

    // The other side: the ordinary machine, where this app owns no sidecar and
    // has stopped nothing. A panel that announced "no sidecar" on every launch
    // would be noise, and noise is how a real warning gets ignored.
    it("says nothing about a sidecar when there is nothing to say", async () => {
      renderDiagnostics();

      expect(await screen.findByText("Reachable")).toBeInTheDocument();
      expect(screen.queryByText("Daemon started by this app")).not.toBeInTheDocument();
      expect(screen.queryByText(/two copies may be running/)).not.toBeInTheDocument();
    });
  });

  // Round 5, D5 residual. `lastStop.outcome === "ended" ? up : down` put
  // `could_not_measure` in the SAME red as `still_running`. The two labels
  // differ, but colour and tone are read first and both said "measured bad" —
  // one of them about a stop that was never observed. Asserted as a
  // comparison, not against a literal class list, so the test is about the two
  // being DIFFERENT rather than about which tokens the chip happens to use.
  describe("sidecar last-stop tone", () => {
    const chipClassFor = (label: string) =>
      (screen.getByText(label).closest("span[aria-live]") as HTMLElement).className;

    async function classForOutcome(last_sidecar_stop: WireState["daemon"]["last_sidecar_stop"]) {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        daemon: { ...wireFixture.daemon, last_sidecar_stop },
      });
      const { unmount } = renderDiagnostics();
      await screen.findByText("Daemon started by this app");
      const className = chipClassFor(
        last_sidecar_stop?.outcome === "still_running"
          ? "Last stop: still running"
          : "Last stop: could not confirm it ended",
      );
      unmount();
      return className;
    }

    it("does not paint an unconfirmed stop in the same tone as a measured one", async () => {
      const unverified = await classForOutcome({
        outcome: "could_not_measure",
        reason: "pid identity was lost",
      });
      const stillRunning = await classForOutcome({
        outcome: "still_running",
        reason: "the process is still alive",
      });

      expect(stillRunning).toContain("var(--mem-status-danger-bg)");
      // A stop nobody could confirm is not a stop that was measured not to
      // have happened. Different reading, different picture.
      expect(unverified).not.toContain("var(--mem-status-danger-bg)");
      expect(unverified).not.toEqual(stillRunning);
      // ...and it must not swing the other way either.
      expect(unverified).not.toContain("var(--mem-status-success-bg)");
    });
  });

  // Round 5, D4, at the surface it was invisible from. An input that could not
  // be determined used to be reachable only inside `unresolved`, i.e. only
  // when NO command was chosen. So the case that actually hides it — the
  // search found a binary under a determined input while another input went
  // unread — rendered a clean, complete-looking panel. Fails against the old
  // `mcpBinary.unresolved?.undetermined` read, where a found command means
  // `unresolved` is absent and the row cannot exist.
  describe("an undetermined input on a search that still found a binary", () => {
    const foundWithUndetermined = {
      ...wireFixture,
      mcp_binary: {
        command: "/Users/x/.wenlan/bin/wenlan-mcp",
        args: [],
        undetermined: [
          {
            input: "WENLAN_MCP_DEV_BIN",
            blocked: "WENLAN_MCP_DEV_BIN",
            error: "environment variable was not valid Unicode",
          },
        ],
        candidates: [
          {
            path: "/Users/x/.wenlan/bin/wenlan-mcp",
            state: { kind: "file" as const },
            source: "installed" as const,
          },
        ],
      },
    };

    it("still names the input that was never read", async () => {
      vi.mocked(getWireState).mockResolvedValue(foundWithUndetermined);

      renderDiagnostics();

      // The command is genuinely there — this is a successful resolution.
      expect(
        await screen.findByTestId("diagnostics-selected-executable"),
      ).toBeInTheDocument();
      expect(screen.getByTestId("diagnostics-selected-executable")).toHaveTextContent("/Users/x/.wenlan/bin/wenlan-mcp");
      // And the unread input is still reported, in the tone for "no reading",
      // never as a measured absence.
      expect(screen.getByText(/Not checked/)).toBeInTheDocument();
      expect(screen.getByText(/WENLAN_MCP_DEV_BIN could not be determined/)).toBeInTheDocument();
    });

  });

  // ── Round 5, defect 4: a client the app could not look at ─────────────
  //
  // `detect_mcp_clients` answered every one of these with a bare boolean:
  // `read_to_string(..).map(..).unwrap_or(false)` for the config questions and
  // `Path::exists()` for the bundle ones. This card is the surface where that
  // collapse is most expensive, because its entire job is telling a user which
  // part of their setup is broken.
  describe("a client whose state could not be read", () => {
    const unreadableClient = {
      ...wireFixture,
      clients: [
        {
          client_type: "cursor",
          name: "Cursor",
          detected: unreadable("Access is denied. (os error 5)"),
          config_path: null,
          has_raw_entry: unreadable("Access is denied. (os error 5)"),
          has_raw_duplicate: unreadable("Access is denied. (os error 5)"),
          has_plugin: unreadable("Access is denied. (os error 5)"),
          route: "unknown",
        },
      ],
    } as WireState;

    it("says it could not check, never that the client is not detected", async () => {
      vi.mocked(getWireState).mockResolvedValue(unreadableClient);

      renderDiagnostics();

      await screen.findByText("Reachable");
      // Regex: the chip appends the reason after the label — the half a
      // boolean could never carry.
      expect(screen.getAllByText(/Could not check/).length).toBeGreaterThan(0);
      expect(
        screen.getByText((_, element) =>
          element?.tagName === "P" &&
          element.textContent?.includes("has_raw_entry: could not check (Access is denied. (os error 5))") === true,
        ),
      ).toBeInTheDocument();
      expect(screen.queryByText("Not detected")).not.toBeInTheDocument();
      // And no instruction is derived from it: `route: unknown` renders as
      // "could not tell", not as one of the three real routes.
      expect(screen.getByText("Could not tell")).toBeInTheDocument();
      expect(screen.queryByText("Sets up an MCP entry")).not.toBeInTheDocument();
    });

    it("shows the neutral chip, not the same chip a measured absence gets", async () => {
      vi.mocked(getWireState).mockResolvedValue(unreadableClient);

      renderDiagnostics();

      await screen.findByText("Reachable");
      const chip = screen.getAllByText(/Could not check/)[0].closest("span.inline-flex");
      expect(chip).toHaveTextContent("Could not check");
      expect(chip).not.toHaveTextContent("Access is denied");
      // The warning tokens, which the "idle"/"down" chips never use. Reading
      // this off the class list is deliberate: it is the only way to prove the
      // two states are visually distinguishable, not merely differently worded.
      expect(chip?.className).toContain("--mem-status-warning-bg");
      expect(chip?.className).not.toContain("--mem-status-danger-bg");
    });

    it("shows a path placeholder rather than the word null", async () => {
      vi.mocked(getWireState).mockResolvedValue(unreadableClient);

      renderDiagnostics();

      await screen.findByText("Reachable");
      expect(
        screen.getByText("config location could not be determined"),
      ).toBeInTheDocument();
      expect(screen.queryByText("null")).not.toBeInTheDocument();
    });

    // THE destructive one. The raw+raw box was gated on `!client.has_plugin`,
    // which is TRUE for a plugin state that could not be read — so a failed
    // read raised a warning whose premise ("this client has no plugin") was
    // never measured, offering a one-click config edit off the back of it.
    it("raises no duplicate-entry warning off a plugin state that was never read", async () => {
      vi.mocked(getWireState).mockResolvedValue({
        ...wireFixture,
        clients: [
          {
            client_type: "cursor",
            name: "Cursor",
            detected: YES,
            config_path: "/Users/x/.cursor/mcp.json",
            has_raw_entry: YES,
            has_raw_duplicate: YES,
            has_plugin: unreadable("Access is denied. (os error 5)"),
            route: "unknown",
          },
        ],
      } as WireState);

      renderDiagnostics();

      await screen.findByText("Reachable");
      expect(screen.queryByText(/Cursor's config lists Wenlan twice/)).not.toBeInTheDocument();
      expect(screen.queryByText("Remove the old entry")).not.toBeInTheDocument();
      // Nor the plugin+raw box, whose premise is equally unmeasured.
      expect(screen.queryByText("Remove duplicate entry")).not.toBeInTheDocument();
    });

  });

  describe("i18n", () => {
    afterEach(async () => {
      await i18n.changeLanguage("en");
    });

    it("renders its heading through the translation layer, not hardcoded English", async () => {
      await i18n.changeLanguage("zh-Hans");
      renderDiagnostics();

      expect(await screen.findByText("后台工作")).toBeInTheDocument();
      expect(screen.queryByText("Background work")).not.toBeInTheDocument();
    });

    it("renders the wiring heading through the translation layer, not hardcoded English", async () => {
      await i18n.changeLanguage("zh-Hans");
      renderDiagnostics();

      expect(await screen.findByText("AI 工具集成")).toBeInTheDocument();
      expect(screen.queryByText("Wiring")).not.toBeInTheDocument();
    });
  });
});
