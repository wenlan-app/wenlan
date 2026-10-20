// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import GeneralSection from "./GeneralSection";
import { open } from "@tauri-apps/plugin-dialog";
import {
  exportPagesAsOkf,
  getKnowledgePath,
  getTelemetryStatus,
  isRunAtLoginEnabled,
  openFile,
  setRunAtLogin,
  setTelemetryEnabled,
} from "../../../../lib/tauri";

vi.mock("../../../../lib/tauri", async () => {
  const actual =
    await vi.importActual<typeof import("../../../../lib/tauri")>("../../../../lib/tauri");
  return {
  // The real parsers: the row must read the exact string a rejected command
  // carries, not a test double's idea of it.
  daemonErrorMessage: actual.daemonErrorMessage,
  isOkfExportDaemonTooOld: actual.isOkfExportDaemonTooOld,
  exportPagesAsOkf: vi.fn(),
  getKnowledgePath: vi.fn(),
  openFile: vi.fn(),
  getProfile: vi.fn(() =>
    Promise.resolve({
      id: "p1",
      name: "Lucian",
      display_name: "Lucian",
      email: null,
      bio: null,
      avatar_path: null,
      created_at: 0,
    }),
  ),
  updateProfile: vi.fn(() => Promise.resolve()),
  setAvatar: vi.fn(() => Promise.resolve()),
  removeAvatar: vi.fn(() => Promise.resolve()),
  setSetupCompleted: vi.fn(() => Promise.resolve()),
  isRunAtLoginEnabled: vi.fn(() => Promise.resolve(false)),
  setRunAtLogin: vi.fn(() => Promise.resolve()),
  getTelemetryStatus: vi.fn(() =>
    Promise.resolve({ enabled: false, available: true, pending_operations: 0 }),
  ),
  setTelemetryEnabled: vi.fn((enabled: boolean) =>
    Promise.resolve({ enabled, available: true, pending_operations: 0 }),
  ),
  };
});

vi.mock("../../../../lib/theme", () => ({
  useTheme: () => ["system", vi.fn()] as const,
}));

function renderGeneralSection() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return {
    ...render(
      <QueryClientProvider client={queryClient}>
        <GeneralSection />
      </QueryClientProvider>,
    ),
    // Exposed so a test can drive a REAL refetch failure rather than
    // simulating the state it would produce.
    queryClient,
  };
}

beforeEach(() => {
  vi.mocked(getKnowledgePath).mockResolvedValue("/Users/test/Wenlan/pages");
  vi.mocked(openFile).mockResolvedValue(undefined);
});

describe("GeneralSection page storage location", () => {
  it("shows the Pages-only location and opens its folder", async () => {
    renderGeneralSection();

    expect(await screen.findByText("/Users/test/Wenlan/pages")).toBeInTheDocument();
    expect(screen.getByText("Page storage location")).toBeInTheDocument();
    expect(screen.getByText("Wenlan pages are stored here as Markdown files.")).toBeInTheDocument();
    expect(screen.queryByText(/page files/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Open folder" }));
    await waitFor(() => expect(openFile).toHaveBeenCalledWith("/Users/test/Wenlan/pages"));
  });

  it("shows a loading status and does not offer an empty-path action", async () => {
    let resolvePath!: (path: string) => void;
    vi.mocked(getKnowledgePath).mockReturnValue(new Promise((resolve) => { resolvePath = resolve; }));

    renderGeneralSection();
    expect(screen.getByRole("status", { name: "Loading page storage location" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open folder" })).not.toBeInTheDocument();
    resolvePath("/Users/test/Wenlan/pages");
    expect(await screen.findByRole("button", { name: "Open folder" })).toBeInTheDocument();
  });

  it("treats a blank returned path as unavailable", async () => {
    vi.mocked(getKnowledgePath).mockResolvedValueOnce("   ");
    renderGeneralSection();
    expect(await screen.findByText("Could not read the page storage location.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open folder" })).not.toBeInTheDocument();
  });

  it("states when the location could not be read and reports open failures", async () => {
    vi.mocked(getKnowledgePath).mockRejectedValueOnce(new Error("permission denied"));
    renderGeneralSection();
    expect(await screen.findByText("Could not read the page storage location.")).toBeInTheDocument();

    vi.mocked(getKnowledgePath).mockResolvedValueOnce("/Users/test/Wenlan/pages");
    vi.mocked(openFile).mockRejectedValueOnce(new Error("Finder unavailable"));
    const { unmount } = renderGeneralSection();
    expect(await screen.findByRole("button", { name: "Open folder" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open folder" }));
    expect(await screen.findByText("Finder unavailable")).toBeInTheDocument();
    unmount();
  });
});

/**
 * Wait for the run-at-login state to actually be MEASURED, then click.
 *
 * The row is inert until the query settles, because until then there is no
 * value to take the complement of — `!(undefined ?? false)` is `true` whatever
 * launchd holds. Tests that clicked without waiting used to pass only because
 * the old code was willing to act on that fiction; waiting is what makes the
 * click a real user action rather than a race.
 */
async function clickRunAtLoginOnce(): Promise<HTMLElement> {
  const toggle = await screen.findByRole("button", { name: "Run Wenlan in background at login" });
  await waitFor(() => expect(toggle).not.toBeDisabled());
  fireEvent.click(toggle);
  return toggle;
}

// S6: run-at-login, theme, and language used to be three separate
// `Card padding="rows"` blocks. They now share one. `.divide-y` is the class
// `Card` only applies for padding="rows", so counting it proves the merge
// instead of just trusting the JSX shape. ProfileSettingsBlock also renders
// a `rows` Card (Photo + Display name), so scope to the card that contains
// "Language" — unique to the App section — rather than every `.divide-y`.
describe("GeneralSection app card merge", () => {
  it("renders run-at-login, theme, and language inside a single rows Card", async () => {
    renderGeneralSection();

    await screen.findByLabelText("Language");

    const rowsCards = Array.from(document.querySelectorAll(".divide-y")).filter((el) =>
      el.textContent?.includes("Language"),
    );
    expect(rowsCards).toHaveLength(1);

    const merged = rowsCards[0];
    expect(merged.textContent).toContain("Run Wenlan in background at login");
    expect(merged.textContent).toContain("Theme");
    expect(merged.textContent).toContain("Language");
  });
});

// `is_run_at_login_enabled` reports an unreadable launchctl as an error rather
// than as `false`. A rejected query must not leave the row looking like a
// measured "off": the user would read the feature as disabled while launchd
// may still be starting Wenlan every boot.
describe("GeneralSection run-at-login state", () => {
  it("says the state could not be read instead of silently showing it off", async () => {
    vi.mocked(isRunAtLoginEnabled).mockRejectedValueOnce(
      new Error("Could not read the Run at Login state: launchctl did not answer."),
    );

    renderGeneralSection();

    expect(
      await screen.findByText(/could not read whether this is on/i),
    ).toBeInTheDocument();
  });

  it("shows no such notice when the state was actually measured", async () => {
    renderGeneralSection();

    await screen.findByLabelText("Run Wenlan in background at login");
    expect(screen.queryByText(/could not read whether this is on/i)).toBeNull();
  });
});

// `set_run_at_login` refuses the handover when it could not confirm Wenlan's
// own daemon stopped — registering launchd against a port the old daemon
// still holds would leave two owners. The refusal names which of the two
// happened and what to do about it, and that sentence exists only in the
// rejected `invoke` promise. Without it rendered, the toggle simply fails to
// move and the user is told nothing about why.
describe("GeneralSection run-at-login refusal", () => {
  it("shows the reason the handover was refused", async () => {
    const refusal =
      "Run at Login was not changed: Wenlan's own daemon is still running and still holds " +
      "the port, so handing it to the system launcher would leave two owners. Quit and " +
      "reopen Wenlan, then try again.";
    // A Tauri command declared `Result<_, String>` rejects with the bare
    // string, not an Error — so this is the shape the component really sees.
    vi.mocked(setRunAtLogin).mockRejectedValueOnce(refusal);

    renderGeneralSection();
    await clickRunAtLoginOnce();

    expect(await screen.findByText(refusal)).toBeInTheDocument();
  });

  it("says nothing when the handover succeeded", async () => {
    renderGeneralSection();
    await clickRunAtLoginOnce();

    await waitFor(() => expect(vi.mocked(setRunAtLogin)).toHaveBeenCalled());
    expect(screen.queryByText(/was not changed/i)).toBeNull();
  });

  // A rejection that carries no readable sentence must leave the row silent
  // rather than printing `[object Object]` or an empty red line: a row that
  // claims to be explaining something, and is not, is worse than no row.
  // An unread state is not `false`. Painting the switch off claims launchd was
  // measured, and `!(undefined ?? false)` is `true` whatever launchd actually
  // holds — so a click on an unreadable row would send a value derived from
  // nothing. Both halves are asserted: the row must not claim, and must not act.
  it("neither paints nor acts on a run-at-login state it could not read", async () => {
    vi.mocked(isRunAtLoginEnabled).mockRejectedValueOnce(
      new Error("Could not read the Run at Login state: launchctl did not answer."),
    );

    renderGeneralSection();
    await screen.findByText(/could not read whether this is on/i);

    const unknownStatus = screen.getByRole("status", { name: "Run Wenlan in background at login" });
    expect(unknownStatus).toHaveTextContent("Unavailable");
    expect(unknownStatus.tagName).toBe("SPAN");
    expect(screen.queryByRole("button", { name: "Run Wenlan in background at login" })).not.toBeInTheDocument();

    // Counted rather than `not.toHaveBeenCalled()`: the module mock is shared
    // across this file and is not cleared between tests, so an absolute
    // assertion here would be measuring earlier tests' calls, not this click.
    const callsBefore = vi.mocked(setRunAtLogin).mock.calls.length;
    fireEvent.click(unknownStatus);
    expect(vi.mocked(setRunAtLogin).mock.calls.length).toBe(callsBefore);
  });

  // The two failures are independent and say different things: one reports the
  // value is unknown, the other names why a change was refused and what to do.
  // Ranking them drops the half carrying the remedy.
  it("shows the unreadable notice and the refusal together when both are live", async () => {
    const refusal =
      "Run at Login was not changed: Wenlan's own daemon is still running and still holds " +
      "the port, so handing it to the system launcher would leave two owners.";
    vi.mocked(setRunAtLogin).mockRejectedValueOnce(refusal);

    const { queryClient } = renderGeneralSection();
    await clickRunAtLoginOnce();
    await screen.findByText(new RegExp(refusal.slice(0, 40).replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));

    // A real refetch that fails, not a simulated error state.
    vi.mocked(isRunAtLoginEnabled).mockRejectedValueOnce(new Error("launchctl went away"));
    await queryClient.invalidateQueries({ queryKey: ["runAtLogin"] });

    await waitFor(() => {
      const row = screen.getByRole("status", { name: "Run Wenlan in background at login" }).closest(".px-5");
      expect(row?.textContent).toMatch(/could not read whether this is on/i);
      expect(row?.textContent).toContain("still holds the port");
    });
  });

  it("stays silent when the rejection carries no message", async () => {
    vi.mocked(setRunAtLogin).mockRejectedValueOnce({ code: 42 });

    renderGeneralSection();
    await clickRunAtLoginOnce();

    await waitFor(() => expect(vi.mocked(setRunAtLogin)).toHaveBeenCalled());
    expect(screen.queryByText(/object Object/i)).toBeNull();
  });
});

// Round 5, D2. The guard used to be `runAtLoginQuery.data === undefined`, which
// answers "has any value ever been cached", not "did the current read succeed".
// React Query RETAINS the last `data` when a refetch fails, so the reachable
// state `data === false, isError === true` is a FAILED read still wearing an
// earlier read's answer — and the row rendered it as an enabled, measured
// switch beside the notice saying the state could not be read. Both retained
// values are staged, because they fail in opposite directions: a retained
// `false` sends `mutate(true)`, a retained `true` sends `mutate(false)`. Both
// of these fail against the old guard, where the toggle stays enabled.
describe("GeneralSection unknown control states", () => {
  it("shows loading until the run-at-login setting has been measured", async () => {
    let resolveRead!: (value: boolean) => void;
    vi.mocked(isRunAtLoginEnabled).mockImplementationOnce(() => new Promise((resolve) => { resolveRead = resolve; }));
    renderGeneralSection();

    expect(screen.getByRole("status", { name: "Run Wenlan in background at login" })).toHaveTextContent("Loading");
    expect(screen.queryByRole("button", { name: "Run Wenlan in background at login" })).not.toBeInTheDocument();
    resolveRead(true);
    await waitFor(() => expect(screen.getByRole("button", { name: "Run Wenlan in background at login" })).toHaveAttribute("aria-pressed", "true"));
  });
});

describe("GeneralSection run-at-login value retained across a failed refresh", () => {
  const runAtLoginToggle = () =>
    screen.getByLabelText("Run Wenlan in background at login");

  async function measureThenFailRefresh(measured: boolean) {
    vi.mocked(isRunAtLoginEnabled).mockResolvedValueOnce(measured);
    const { queryClient } = renderGeneralSection();

    await screen.findByLabelText("Run Wenlan in background at login");
    await waitFor(() => expect(runAtLoginToggle()).not.toBeDisabled());
    expect(runAtLoginToggle()).toHaveAttribute("aria-pressed", String(measured));

    // A real refetch that fails, leaving `data` at `measured` and `isError`
    // true — not a simulated error state.
    vi.mocked(isRunAtLoginEnabled).mockRejectedValueOnce(
      new Error("Could not read the Run at Login state: launchctl did not answer."),
    );
    await queryClient.invalidateQueries({ queryKey: ["runAtLogin"] });
    await waitFor(() =>
      expect(screen.getByText(/could not read whether this is on/i)).toBeInTheDocument(),
    );
  }

  it.each([true, false])(
    "stops asserting a retained %s once the refresh that would confirm it failed",
    async (measured) => {
      await measureThenFailRefresh(measured);

      // The switch must stop claiming the stale reading is a measurement.
      await waitFor(() => expect(screen.getByRole("status", { name: "Run Wenlan in background at login" })).toBeInTheDocument());
      expect(screen.getByRole("status", { name: "Run Wenlan in background at login" })).toHaveTextContent("Unavailable");
      expect(screen.queryByRole("button", { name: "Run Wenlan in background at login" })).not.toBeInTheDocument();

      // And it must not ACT on it: the complement of a stale reading is a
      // write to launchd derived from an earlier instant.
      // Counted rather than `not.toHaveBeenCalled()` — the module mock is
      // shared across this file and never cleared.
      const callsBefore = vi.mocked(setRunAtLogin).mock.calls.length;
      fireEvent.click(screen.getByRole("status", { name: "Run Wenlan in background at login" }));
      expect(vi.mocked(setRunAtLogin).mock.calls.length).toBe(callsBefore);
    },
  );
});

describe("GeneralSection optional usage stats consent", () => {
  const telemetryToggle = () => screen.getByLabelText("Share optional usage stats");
  const status = (enabled: boolean, available = true, pending_operations = 0) => ({
    enabled,
    available,
    pending_operations,
  });

  it("shows loading while the usage-stats setting is being read, then the measured value", async () => {
    let resolveRead!: (value: { enabled: boolean; available: boolean; pending_operations: number }) => void;
    vi.mocked(getTelemetryStatus).mockImplementationOnce(() => new Promise((resolve) => { resolveRead = resolve; }));
    renderGeneralSection();

    expect(screen.getByRole("status", { name: "Share optional usage stats" })).toHaveTextContent("Loading");
    expect(screen.queryByRole("button", { name: "Share optional usage stats" })).not.toBeInTheDocument();
    resolveRead(status(false));
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "false"));
  });

  it("keeps the toggle unknown and inert when the status cannot be read", async () => {
    vi.mocked(getTelemetryStatus).mockRejectedValueOnce(new Error("daemon unavailable"));

    renderGeneralSection();

    expect(
      await screen.findByText(/could not read the usage-stats setting/i),
    ).toBeInTheDocument();
    const status = screen.getByRole("status", { name: "Share optional usage stats" });
    expect(status).toHaveTextContent("Unavailable");
    expect(screen.queryByRole("button", { name: "Share optional usage stats" })).not.toBeInTheDocument();
    const callsBefore = vi.mocked(setTelemetryEnabled).mock.calls.length;
    fireEvent.click(status);
    expect(vi.mocked(setTelemetryEnabled).mock.calls.length).toBe(callsBefore);
  });

  it("renders a measured default-off status and enables it only after an explicit click", async () => {
    vi.mocked(getTelemetryStatus).mockResolvedValueOnce(status(false));

    renderGeneralSection();

    await screen.findByLabelText("Share optional usage stats");
    await waitFor(() => expect(telemetryToggle()).not.toBeDisabled());
    expect(telemetryToggle()).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(telemetryToggle());
    await waitFor(() =>
      expect(vi.mocked(setTelemetryEnabled)).toHaveBeenCalledWith(true, expect.anything()),
    );
  });

  it("renders intentional opt-in as on and reflects a successful opt-out reload", async () => {
    vi.mocked(getTelemetryStatus)
      .mockResolvedValueOnce(status(true))
      .mockResolvedValueOnce(status(false));
    vi.mocked(setTelemetryEnabled).mockResolvedValueOnce(status(false));

    renderGeneralSection();

    await screen.findByLabelText("Share optional usage stats");
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "true"));
    fireEvent.click(telemetryToggle());

    await waitFor(() => {
      expect(vi.mocked(setTelemetryEnabled)).toHaveBeenCalledWith(false, expect.anything());
      expect(telemetryToggle()).toHaveAttribute("aria-pressed", "false");
    });
  });

  it("shows a generic localized save failure and reloads the daemon's actual state", async () => {
    vi.mocked(getTelemetryStatus)
      .mockResolvedValueOnce(status(true))
      .mockResolvedValueOnce(status(true));
    vi.mocked(setTelemetryEnabled).mockRejectedValueOnce(new Error("HTTP 500: secret detail"));

    renderGeneralSection();

    await screen.findByLabelText("Share optional usage stats");
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "true"));
    fireEvent.click(telemetryToggle());

    expect(
      await screen.findByText(
        "Wenlan could not persist your usage-stats setting. Check the current switch state and retry before restarting Wenlan.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/secret detail/i)).not.toBeInTheDocument();
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "true"));
  });

  it("keeps an unavailable build disabled without claiming that telemetry is off", async () => {
    vi.mocked(getTelemetryStatus).mockResolvedValueOnce({ enabled: false, available: false, pending_operations: 0 });

    renderGeneralSection();

    expect(
      await screen.findByText(/unavailable in this build or environment/i),
    ).toBeInTheDocument();
    const unknownStatus = screen.getByRole("status", { name: "Share optional usage stats" });
    expect(unknownStatus).toHaveTextContent("Unavailable");
    expect(screen.queryByRole("button", { name: "Share optional usage stats" })).not.toBeInTheDocument();
    const callsBefore = vi.mocked(setTelemetryEnabled).mock.calls.length;
    fireEvent.click(unknownStatus);
    expect(vi.mocked(setTelemetryEnabled).mock.calls.length).toBe(callsBefore);
  });

  it("allows turning off existing consent when telemetry is unavailable", async () => {
    vi.mocked(getTelemetryStatus).mockResolvedValueOnce(status(true, false));
    renderGeneralSection();

    await screen.findByLabelText("Share optional usage stats");
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "true"));
    fireEvent.click(telemetryToggle());
    await waitFor(() => expect(vi.mocked(setTelemetryEnabled)).toHaveBeenCalledWith(false, expect.anything()));
  });

  it("reports saving while the consent change is pending", async () => {
    vi.mocked(getTelemetryStatus).mockResolvedValueOnce(status(false)).mockResolvedValueOnce(status(true));
    let resolveSave!: (value: ReturnType<typeof status>) => void;
    vi.mocked(setTelemetryEnabled).mockImplementationOnce(() => new Promise((resolve) => { resolveSave = resolve; }));
    renderGeneralSection();

    await screen.findByLabelText("Share optional usage stats");
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "false"));
    const toggle = telemetryToggle();
    toggle.focus();
    fireEvent.click(toggle);
    const saving = await screen.findByRole("status", { name: "Saving" });
    expect(saving).toHaveTextContent("Saving");
    expect(telemetryToggle()).toBe(toggle);
    expect(toggle).toHaveAttribute("aria-disabled", "true");
    expect(toggle).toHaveAttribute("aria-busy", "true");
    expect(toggle).toHaveFocus();

    const callsDuringSave = vi.mocked(setTelemetryEnabled).mock.calls.length;
    fireEvent.click(toggle);
    expect(vi.mocked(setTelemetryEnabled).mock.calls.length).toBe(callsDuringSave);
    expect(toggle).toHaveFocus();

    resolveSave(status(true));
    await waitFor(() => expect(telemetryToggle()).toHaveAttribute("aria-pressed", "true"));
    expect(telemetryToggle()).toBe(toggle);
    expect(toggle).toHaveFocus();
  });

  it("surfaces pending operations without adding an automatic opt-in", async () => {
    vi.mocked(getTelemetryStatus).mockResolvedValueOnce(status(true, true, 2));
    const callsBefore = vi.mocked(setTelemetryEnabled).mock.calls.length;

    renderGeneralSection();

    expect(await screen.findByText("2 usage events waiting to send")).toBeInTheDocument();
    expect(telemetryToggle()).toHaveAttribute("aria-pressed", "true");
    expect(vi.mocked(setTelemetryEnabled).mock.calls.length).toBe(callsBefore);
  });
});

describe("GeneralSection OKF export", () => {
  const TARGET = "/Users/someone/okf-bundle";

  function pickFolder(path: string | null) {
    vi.mocked(open).mockReset();
    vi.mocked(open).mockResolvedValue(path as never);
  }

  async function chooseFolder() {
    fireEvent.click(await screen.findByRole("button", { name: "Choose folder" }));
  }

  it("exports every page into the picked folder and reports the counts", async () => {
    pickFolder(TARGET);
    vi.mocked(exportPagesAsOkf).mockReset();
    vi.mocked(exportPagesAsOkf).mockResolvedValue({ exported: 3, skipped: 1, failed: 0 });
    renderGeneralSection();

    await chooseFolder();

    expect(
      await screen.findByText(`Exported 3 pages to ${TARGET}.`),
    ).toBeInTheDocument();
    expect(open).toHaveBeenCalledWith(
      expect.objectContaining({ directory: true, multiple: false }),
    );
    expect(exportPagesAsOkf).toHaveBeenCalledTimes(1);
    expect(exportPagesAsOkf).toHaveBeenCalledWith(TARGET);
    expect(
      screen.getByText(
        "1 page was not exported because it is not yet cleared for use outside Wenlan.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/could not be written/)).not.toBeInTheDocument();
  });

  it("exports nothing when the folder picker is cancelled", async () => {
    pickFolder(null);
    vi.mocked(exportPagesAsOkf).mockReset();
    renderGeneralSection();

    await chooseFolder();

    await waitFor(() => expect(open).toHaveBeenCalledTimes(1));
    expect(exportPagesAsOkf).not.toHaveBeenCalled();
    expect(screen.queryByText(/^Exported /)).not.toBeInTheDocument();
  });

  it("shows the daemon's refusal for a folder that is not an earlier export, without the HTTP prefix", async () => {
    pickFolder(TARGET);
    vi.mocked(exportPagesAsOkf).mockReset();
    vi.mocked(exportPagesAsOkf).mockRejectedValueOnce(
      'HTTP POST /api/pages/export returned 409 Conflict: {"error":"directory is not empty and is not a Wenlan OKF export"}',
    );
    vi.mocked(exportPagesAsOkf).mockResolvedValueOnce({ exported: 2, skipped: 0, failed: 0 });
    renderGeneralSection();

    await chooseFolder();

    expect(
      await screen.findByText(
        "Export failed: directory is not empty and is not a Wenlan OKF export",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/HTTP POST/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^Exported /)).not.toBeInTheDocument();

    // A retry into a folder the daemon accepts replaces the refusal.
    await chooseFolder();

    expect(await screen.findByText(`Exported 2 pages to ${TARGET}.`)).toBeInTheDocument();
    expect(screen.queryByText(/^Export failed/)).not.toBeInTheDocument();
  });

  it("tells the user to restart a background service too old to write OKF", async () => {
    pickFolder(TARGET);
    vi.mocked(exportPagesAsOkf).mockReset();
    vi.mocked(exportPagesAsOkf).mockRejectedValueOnce("okf-export:daemon-too-old");
    renderGeneralSection();

    await chooseFolder();

    expect(
      await screen.findByText(
        "Export failed: the background service is too old to write an OKF bundle. Restart the service, then try again. Nothing was exported.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/okf-export:/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^Exported /)).not.toBeInTheDocument();
  });

  it("warns when some pages could not be written", async () => {
    pickFolder(TARGET);
    vi.mocked(exportPagesAsOkf).mockReset();
    vi.mocked(exportPagesAsOkf).mockResolvedValue({ exported: 1, skipped: 0, failed: 2 });
    renderGeneralSection();

    await chooseFolder();

    expect(await screen.findByText(`Exported 1 page to ${TARGET}.`)).toBeInTheDocument();
    expect(screen.getByText("2 pages could not be written.")).toBeInTheDocument();
    expect(screen.queryByText(/not yet cleared/)).not.toBeInTheDocument();
  });
});
