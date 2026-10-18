// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider, onlineManager } from "@tanstack/react-query";
import App from "./App";

const eventListeners = vi.hoisted(
  () => new Map<string, (event: { payload: unknown }) => void>(),
);
const quitGuardMock = vi.hoisted(
  () => vi.fn<() => Promise<boolean>>(),
);
const quitWenlanFullMock = vi.hoisted(
  () => vi.fn<() => Promise<void>>(),
);
const acknowledgeGuardedQuitRequestMock = vi.hoisted(
  () => vi.fn<(requestId: number, deliveryId: number) => Promise<boolean>>(),
);
const cancelGuardedQuitRequestMock = vi.hoisted(
  () => vi.fn<(requestId: number, deliveryId: number) => Promise<boolean>>(),
);
const hideWindowMock = vi.hoisted(() => vi.fn<() => Promise<void>>());
const showWindowMock = vi.hoisted(() => vi.fn<() => Promise<void>>());
const focusWindowMock = vi.hoisted(() => vi.fn<() => Promise<void>>());
const emitMock = vi.hoisted(() => vi.fn());
const startDaemonSidecarMock = vi.hoisted(
  () => vi.fn<() => Promise<{ status: string; message?: string }>>(),
);
const getWireStateMock = vi.hoisted(
  () => vi.fn<() => Promise<{ daemon: { reachable: boolean; error: string | null } }>>(),
);

vi.mock("@tauri-apps/api/event", () => ({
  emit: emitMock,
  listen: vi.fn((event: string, handler: (event: { payload: unknown }) => void) => {
    eventListeners.set(event, handler);
    return Promise.resolve(() => eventListeners.delete(event));
  }),
}));

vi.mock("./lib/tauri", () => ({
  acknowledgeGuardedQuitRequest: acknowledgeGuardedQuitRequestMock,
  cancelGuardedQuitRequest: cancelGuardedQuitRequestMock,
  quitWenlanFull: quitWenlanFullMock,
  shouldShowWizard: vi.fn(),
  setSetupCompleted: vi.fn().mockResolvedValue(undefined),
  setTrafficLightsVisible: vi.fn().mockResolvedValue(undefined),
  startDaemonSidecar: startDaemonSidecarMock,
  getWireState: getWireStateMock,
}));

// No setSize/setPosition/scaleFactor/currentMonitor here on purpose: App no
// longer touches the window geometry, so mocking those would only hide a
// regression that re-added the launch-time resize.
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    setAlwaysOnTop: vi.fn(),
    isVisible: vi.fn().mockResolvedValue(true),
    hide: hideWindowMock,
    show: showWindowMock,
    setFocus: focusWindowMock,
  }),
}));

// The real ladder is ~162s of wall clock by design (it has to outlast the
// Rust health loop). Running it here would be a three-minute unit test, so
// this file substitutes a two-attempt policy and asserts only the branching
// it owns. The production schedule and its budget are pinned as arithmetic in
// src/lib/bootRetryPolicy.test.ts.
vi.mock("./lib/bootRetryPolicy", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./lib/bootRetryPolicy")>();
  return {
    ...actual,
    ATTEMPT_TIMEOUT_MS: 5000,
    RUST_HEALTH_LOOP_BUDGET_MS: 152_200,
    BOOT_QUERY_RETRY: 1,
    // Shrunk with the rest of the ladder: the production 15s would outlast the
    // two-attempt policy above, so the notice could never render here.
    BOOT_SLOW_NOTICE_MS: 20,
    bootQueryRetryDelay: () => 10,
    bootQueryBudgetMs: () => 10_010,
  };
});

// Heavy real children — swap for markers so this test only pins App's own
// wizard-vs-home branching, not Main's or SetupWizard's internals.
vi.mock("./components/memory/Main", () => ({
  default: (props: {
    initialView?: { kind: string; section?: string };
    onRegisterQuitGuard?: (guard: (() => Promise<boolean>) | null) => void;
  }) => {
    props.onRegisterQuitGuard?.(quitGuardMock);
    return (
      <div data-testid="home-main" data-initial-view={props.initialView?.kind ?? "pages"} data-initial-section={props.initialView?.section}>
        <input aria-label="Draft title" />
      </div>
    );
  },
}));

vi.mock("./components/SetupWizard", () => ({
  // Mirrors the Done step's contract with onComplete: await it and keep the
  // wizard on a rejection (the real step shows an inline alert).
  default: (props: {
    onComplete: () => void | Promise<void>;
    daemonGateErrored?: boolean;
  }) => (
    <div data-testid="setup-wizard" data-gate-errored={String(!!props.daemonGateErrored)}>
      wizard
      <button
        type="button"
        onClick={() => {
          void Promise.resolve()
            .then(() => props.onComplete())
            .catch(() => {});
        }}
      >
        Finish setup
      </button>
    </div>
  ),
}));

vi.mock("./components/RuntimeOverlays", () => ({
  RuntimeOverlays: ({ variant = "main" }: { variant?: string }) => (
    <div data-testid="runtime-overlays" data-variant={variant}>runtime overlays</div>
  ),
}));

// EntityDetail transitively imports AtlasView → sigma, whose dist touches
// WebGL2RenderingContext at module scope — jsdom has no such global, so the
// real import crashes this whole suite before a single test runs.
vi.mock("./components/memory/EntityDetail", () => ({
  default: () => null,
}));

vi.mock("./components/onboarding/MilestoneToaster", () => ({
  MilestoneToaster: () => null,
}));

vi.mock("./components/UpdaterDialog", () => ({
  default: () => null,
}));

import { setSetupCompleted, shouldShowWizard } from "./lib/tauri";
import { bootQueryBudgetMs } from "./lib/bootRetryPolicy";
import { resources } from "./i18n/resources";
import { dragStripHeight } from "./lib/windowChrome";

const STARTING_RUNTIME = resources.en.translation.common.startingRuntime;
const BOOT = resources.en.translation.boot;

function renderApp() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>,
  );
}

function dispatchQuit(requestId = 1, deliveryId = 1) {
  eventListeners.get("quit-requested")?.({
    payload: { requestId, deliveryId },
  });
}

describe("App - first-run wizard gate", () => {
  beforeEach(() => {
    eventListeners.clear();
    emitMock.mockReset().mockResolvedValue(undefined);
    focusWindowMock.mockReset().mockResolvedValue(undefined);
    hideWindowMock.mockReset().mockResolvedValue(undefined);
    quitGuardMock.mockReset().mockResolvedValue(true);
    acknowledgeGuardedQuitRequestMock.mockReset().mockResolvedValue(true);
    cancelGuardedQuitRequestMock.mockReset().mockResolvedValue(true);
    quitWenlanFullMock.mockReset().mockResolvedValue(undefined);
    showWindowMock.mockReset().mockResolvedValue(undefined);
    vi.mocked(shouldShowWizard).mockReset();
    startDaemonSidecarMock.mockReset().mockResolvedValue({ status: "started" });
  });

  it("renders Home when shouldShowWizard resolves false", async () => {
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();

    expect(await screen.findByTestId("home-main")).toBeInTheDocument();
    expect(screen.queryByTestId("setup-wizard")).not.toBeInTheDocument();
    expect(screen.getByTestId("runtime-overlays")).toHaveAttribute("data-variant", "main");
  });

  it("renders SetupWizard when shouldShowWizard resolves true", async () => {
    vi.mocked(shouldShowWizard).mockResolvedValue(true);
    renderApp();

    expect(await screen.findByTestId("setup-wizard")).toBeInTheDocument();
    expect(screen.getByTestId("runtime-overlays")).toHaveAttribute(
      "data-variant",
      "updater-only",
    );
    expect(screen.queryByTestId("home-main")).not.toBeInTheDocument();
  });

  // The gate asks the daemon over localhost IPC, so it must still answer on a
  // machine with no network. Under react-query's default "online" networkMode
  // an offline browser PAUSES the query (fetchStatus "paused", never
  // "fetching"), which would strand the gate with no data and no error.
  it("still answers the gate when the machine is offline", async () => {
    onlineManager.setOnline(false);
    try {
      vi.mocked(shouldShowWizard).mockResolvedValue(true);
      renderApp();

      expect(await screen.findByTestId("setup-wizard")).toBeInTheDocument();
      expect(screen.queryByTestId("home-main")).not.toBeInTheDocument();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  // A cold start can leave the window with nothing to show for seconds. The
  // gate must say what it is waiting for rather than holding an empty window,
  // and it must not pre-empt that wait with the first-run wizard.
  it("shows the starting-runtime status while the wizard query is pending", async () => {
    let resolveWizard!: (showWizard: boolean) => void;
    vi.mocked(shouldShowWizard).mockReturnValue(
      new Promise<boolean>((resolve) => {
        resolveWizard = resolve;
      }),
    );
    renderApp();

    const status = await screen.findByRole("status");
    expect(status).toHaveTextContent(STARTING_RUNTIME);
    expect(screen.queryByTestId("setup-wizard")).not.toBeInTheDocument();
    expect(screen.queryByTestId("home-main")).not.toBeInTheDocument();

    // The panel sizes itself to the whole window and centres its own content,
    // so it reads correctly only while it keeps those classes. The guard above
    // — Home is not mounted — is what keeps it out of the app shell; this
    // asserts the sizing half. Where it actually lands on screen is geometry
    // jsdom cannot measure: e2e/home-default-window.spec.ts owns that.
    expect(status).toHaveClass(
      "w-screen",
      "h-screen",
      "items-center",
      "justify-center",
    );
    expect(screen.getByTestId("runtime-overlays")).toHaveAttribute(
      "data-variant",
      "updater-only",
    );

    // …and it is a waiting state, not a terminal one.
    await act(async () => {
      resolveWizard(false);
    });
    expect(await screen.findByTestId("home-main")).toBeInTheDocument();
    expect(screen.queryByText(STARTING_RUNTIME)).not.toBeInTheDocument();
  });

  // The daemon's first-run install is async (app/src/lib.rs) and can race this
  // query. App.tsx's per-query retry overrides main.tsx's global retry:false,
  // and this pins that the query retries at all and then fails CLOSED. How
  // MANY times and for how long is the policy module's contract, tested there.
  it("fails closed to SetupWizard when shouldShowWizard rejects (daemon unreachable)", async () => {
    vi.mocked(shouldShowWizard).mockRejectedValue(new Error("connection refused"));
    renderApp();

    expect(await screen.findByTestId("setup-wizard")).toBeInTheDocument();
    expect(screen.queryByTestId("home-main")).not.toBeInTheDocument();
    // Proves retries actually happened, not just a single failed attempt.
    expect(vi.mocked(shouldShowWizard).mock.calls.length).toBeGreaterThan(1);
  });

  // The gate answering "show the wizard" is a fact about setup. The gate
  // ERRORING is a fact about the daemon, and a returning user must not be
  // greeted as brand new while their memories are simply out of reach.
  it("tells the wizard when the gate errored rather than answered", async () => {
    vi.mocked(shouldShowWizard).mockRejectedValue(new Error("connection refused"));
    renderApp();

    const wizard = await screen.findByTestId("setup-wizard");
    expect(wizard).toHaveAttribute("data-gate-errored", "true");
  });

  it("does not claim a connection problem when the gate simply says first run", async () => {
    vi.mocked(shouldShowWizard).mockResolvedValue(true);
    renderApp();

    const wizard = await screen.findByTestId("setup-wizard");
    expect(wizard).toHaveAttribute("data-gate-errored", "false");
  });

  // "Starting Wenlan" can hold for the better part of three minutes. For all
  // of it the screen used to say one sentence that named nothing.
  it("names what it is waiting for once the boot wait runs long", async () => {
    vi.useFakeTimers();
    try {
      vi.mocked(shouldShowWizard).mockReturnValue(new Promise<boolean>(() => {}));
      renderApp();

      expect(screen.getByRole("status")).toBeInTheDocument();
      expect(screen.queryByTestId("boot-slow-notice")).not.toBeInTheDocument();
      // The mocked policy above shrinks the 15s threshold to 20ms.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(20);
      });
      expect(screen.getByTestId("boot-slow-notice")).toHaveTextContent(
        BOOT.stillWaiting,
      );

      // A hung IPC call must fail the gate instead of pending forever: each
      // attempt times out, so past the whole budget the gate errors and fails
      // closed to the wizard. Advance in steps so the gate's retry-delay
      // timers fire between attempts.
      await act(async () => {
        const budget = bootQueryBudgetMs();
        let elapsed = 0;
        while (elapsed < budget + 1_000) {
          await vi.advanceTimersByTimeAsync(1_000);
          elapsed += 1_000;
        }
      });
      const wizard = screen.getByTestId("setup-wizard");
      expect(wizard).toBeInTheDocument();
      expect(wizard).toHaveAttribute("data-gate-errored", "true");
      expect(screen.queryByTestId("home-main")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  // React Query pauses its own retries while document.visibilityState is
  // "hidden", which is what an occluded WKWebView window reports. The gate
  // retries in runBootGate with plain setTimeout instead, so it still fails
  // closed while the window is hidden.
  it("keeps retrying the gate while the window is hidden", async () => {
    const originalVisibility = Object.getOwnPropertyDescriptor(
      document,
      "visibilityState",
    );
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });
    vi.useFakeTimers();
    try {
      vi.mocked(shouldShowWizard).mockRejectedValue(new Error("connection refused"));
      renderApp();

      await act(async () => {
        const budget = bootQueryBudgetMs();
        let elapsed = 0;
        while (elapsed < budget + 1_000) {
          await vi.advanceTimersByTimeAsync(1_000);
          elapsed += 1_000;
        }
      });
      const wizard = screen.getByTestId("setup-wizard");
      expect(wizard).toBeInTheDocument();
      expect(wizard).toHaveAttribute("data-gate-errored", "true");
      expect(vi.mocked(shouldShowWizard).mock.calls.length).toBeGreaterThan(1);
    } finally {
      vi.useRealTimers();
      if (originalVisibility) {
        Object.defineProperty(document, "visibilityState", originalVisibility);
      } else {
        Reflect.deleteProperty(document, "visibilityState");
      }
    }
  });

  // Same hidden window, but the hang is inside one IPC call rather than a
  // fast rejection: the per-attempt timeout is what moves the loop to the
  // next attempt, and the gate still fails closed to the connection-problem
  // state instead of waiting on the hung call.
  it("keeps retrying the gate while the window is hidden and the attempt hangs", async () => {
    const originalVisibility = Object.getOwnPropertyDescriptor(
      document,
      "visibilityState",
    );
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });
    vi.useFakeTimers();
    try {
      vi.mocked(shouldShowWizard)
        .mockReturnValueOnce(new Promise<boolean>(() => {}))
        .mockRejectedValue(new Error("connection refused"));
      renderApp();

      await act(async () => {
        const budget = bootQueryBudgetMs();
        let elapsed = 0;
        while (elapsed < budget + 1_000) {
          await vi.advanceTimersByTimeAsync(1_000);
          elapsed += 1_000;
        }
      });
      expect(vi.mocked(shouldShowWizard).mock.calls.length).toBeGreaterThan(1);
      const wizard = screen.getByTestId("setup-wizard");
      expect(wizard).toBeInTheDocument();
      expect(wizard).toHaveAttribute("data-gate-errored", "true");
      expect(screen.queryByTestId("home-main")).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
      if (originalVisibility) {
        Object.defineProperty(document, "visibilityState", originalVisibility);
      } else {
        Reflect.deleteProperty(document, "visibilityState");
      }
    }
  });

  // A failed setSetupCompleted must leave the gate alone. Invalidating it would
  // refetch a query with no data, which react-query reports as pending, so App
  // would swap the wizard for the starting-runtime screen and drop every pick.
  it("a failed completion keeps the fail-closed wizard mounted and does not re-check the gate", async () => {
    vi.mocked(shouldShowWizard).mockRejectedValue(new Error("connection refused"));
    vi.mocked(setSetupCompleted).mockReset().mockRejectedValue(new Error("connection refused"));
    renderApp();

    const finish = await screen.findByRole("button", { name: "Finish setup" });
    const wizard = screen.getByTestId("setup-wizard");
    const gateChecks = vi.mocked(shouldShowWizard).mock.calls.length;
    fireEvent.click(finish);
    await waitFor(() => expect(setSetupCompleted).toHaveBeenCalledWith(true));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });

    // The same node, not a remount: a remount would reset every wizard pick.
    expect(screen.getByTestId("setup-wizard")).toBe(wizard);
    expect(screen.queryByText(STARTING_RUNTIME)).not.toBeInTheDocument();
    expect(vi.mocked(shouldShowWizard).mock.calls.length).toBe(gateChecks);
    vi.mocked(setSetupCompleted).mockReset().mockResolvedValue(undefined);
  });

  it("a completion that fails once and then saves opens Home", async () => {
    vi.mocked(shouldShowWizard).mockResolvedValueOnce(true).mockResolvedValue(false);
    vi.mocked(setSetupCompleted)
      .mockReset()
      .mockRejectedValueOnce(new Error("connection refused"))
      .mockResolvedValue(undefined);
    renderApp();

    const finish = await screen.findByRole("button", { name: "Finish setup" });
    fireEvent.click(finish);
    await waitFor(() => expect(setSetupCompleted).toHaveBeenCalledTimes(1));
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
    expect(screen.getByTestId("setup-wizard")).toBeInTheDocument();
    expect(shouldShowWizard).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Finish setup" }));
    expect(await screen.findByTestId("home-main")).toBeInTheDocument();
    expect(setSetupCompleted).toHaveBeenCalledTimes(2);
  });

  it("waits for the active editor guard before an explicit quit reaches Tauri", async () => {
    let resolveFlush!: (saved: boolean) => void;
    quitGuardMock.mockReturnValue(new Promise((resolve) => {
      resolveFlush = resolve;
    }));
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit();
      await Promise.resolve();
    });
    expect(acknowledgeGuardedQuitRequestMock).toHaveBeenCalledWith(1, 1);
    expect(quitGuardMock).toHaveBeenCalledTimes(1);
    expect(quitWenlanFullMock).not.toHaveBeenCalled();

    await act(async () => {
      resolveFlush(true);
    });
    await waitFor(() => expect(quitWenlanFullMock).toHaveBeenCalledTimes(1));
  });

  it("does not hide or flush until the native quit delivery is acknowledged", async () => {
    let resolveAcknowledgement!: (acknowledged: boolean) => void;
    acknowledgeGuardedQuitRequestMock.mockReturnValue(new Promise((resolve) => {
      resolveAcknowledgement = resolve;
    }));
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit(3, 1);
      await Promise.resolve();
    });
    expect(hideWindowMock).not.toHaveBeenCalled();
    expect(quitGuardMock).not.toHaveBeenCalled();

    await act(async () => {
      resolveAcknowledgement(true);
    });
    await waitFor(() => expect(quitGuardMock).toHaveBeenCalledTimes(1));
  });

  it("ignores a stale native quit delivery without hiding or flushing", async () => {
    acknowledgeGuardedQuitRequestMock.mockResolvedValue(false);
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit(3, 9);
      await Promise.resolve();
    });

    expect(hideWindowMock).not.toHaveBeenCalled();
    expect(quitGuardMock).not.toHaveBeenCalled();
    expect(quitWenlanFullMock).not.toHaveBeenCalled();
  });

  it("acknowledges a liveness probe without starting a second draft flush", async () => {
    let resolveFlush!: (saved: boolean) => void;
    quitGuardMock.mockReturnValue(new Promise((resolve) => {
      resolveFlush = resolve;
    }));
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit(4, 1);
      await Promise.resolve();
    });
    await waitFor(() => expect(quitGuardMock).toHaveBeenCalledTimes(1));

    await act(async () => {
      dispatchQuit(4, 2);
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(acknowledgeGuardedQuitRequestMock).toHaveBeenNthCalledWith(2, 4, 2);
    });
    expect(quitGuardMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveFlush(true);
    });
    await waitFor(() => expect(quitWenlanFullMock).toHaveBeenCalledTimes(1));
  });

  it("waits for the native window to hide before starting the draft flush", async () => {
    let resolveHide!: () => void;
    hideWindowMock.mockReturnValue(new Promise<void>((resolve) => {
      resolveHide = resolve;
    }));
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit();
      await Promise.resolve();
    });
    expect(hideWindowMock).toHaveBeenCalledTimes(1);
    expect(quitGuardMock).not.toHaveBeenCalled();

    await act(async () => {
      resolveHide();
    });
    await waitFor(() => expect(quitGuardMock).toHaveBeenCalledTimes(1));
  });

  it("keeps the native app hidden after flushing while teardown is still pending", async () => {
    let resolveTeardown!: () => void;
    quitWenlanFullMock.mockReturnValue(new Promise<void>((resolve) => {
      resolveTeardown = resolve;
    }));
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit();
      await Promise.resolve();
    });
    await waitFor(() => expect(quitWenlanFullMock).toHaveBeenCalledTimes(1));

    expect(hideWindowMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveTeardown();
    });
  });

  it("aborts quit and reveals the editor when its pending draft cannot be saved", async () => {
    quitGuardMock.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    await screen.findByTestId("home-main");

    await act(async () => {
      dispatchQuit();
      await Promise.resolve();
    });
    expect(quitWenlanFullMock).not.toHaveBeenCalled();
    expect(hideWindowMock).toHaveBeenCalledTimes(1);
    expect(showWindowMock).toHaveBeenCalledTimes(1);
    expect(focusWindowMock).toHaveBeenCalledTimes(1);
    expect(cancelGuardedQuitRequestMock).toHaveBeenCalledTimes(1);
    expect(cancelGuardedQuitRequestMock).toHaveBeenCalledWith(1, 1);
    expect(emitMock).not.toHaveBeenCalledWith("quit-cancelled");

    await act(async () => {
      dispatchQuit(2, 1);
      await Promise.resolve();
    });
    await waitFor(() => expect(quitWenlanFullMock).toHaveBeenCalledTimes(1));
  });

  it("unlocks and reveals the app when native teardown rejects", async () => {
    quitWenlanFullMock.mockRejectedValue(new Error("shutdown failed"));
    hideWindowMock.mockImplementationOnce(async () => {
      if (document.activeElement instanceof HTMLElement) {
        document.activeElement.blur();
      }
    });
    vi.mocked(shouldShowWizard).mockResolvedValue(false);
    renderApp();
    const title = await screen.findByRole("textbox", { name: "Draft title" });
    title.focus();
    expect(title).toHaveFocus();

    await act(async () => {
      dispatchQuit();
      await Promise.resolve();
    });

    await waitFor(() => expect(showWindowMock).toHaveBeenCalledTimes(1));
    expect(hideWindowMock).toHaveBeenCalledTimes(1);
    expect(focusWindowMock).toHaveBeenCalledTimes(1);
    expect(cancelGuardedQuitRequestMock).toHaveBeenCalledTimes(1);
    expect(cancelGuardedQuitRequestMock).toHaveBeenCalledWith(1, 1);
    expect(emitMock).not.toHaveBeenCalledWith("quit-cancelled");
    expect(title).toHaveFocus();
  });
});

describe("App - origin-fallback-mode banner", () => {
  beforeEach(() => {
    eventListeners.clear();
    emitMock.mockReset().mockResolvedValue(undefined);
    vi.mocked(shouldShowWizard).mockReset().mockResolvedValue(false);
    startDaemonSidecarMock.mockReset().mockResolvedValue({ status: "started" });
    getWireStateMock
      .mockReset()
      .mockResolvedValue({ daemon: { reachable: true, error: null } });
  });

  function fireFallback() {
    act(() => {
      eventListeners.get("origin-fallback-mode")?.({ payload: null });
    });
  }

  it("stays silent until Rust says it is running degraded", async () => {
    renderApp();
    expect(await screen.findByTestId("home-main")).toBeInTheDocument();
    expect(screen.queryByTestId("fallback-service-banner")).not.toBeInTheDocument();
  });

  it.each([
    ["Home", false, "home-main"],
    ["the wizard", true, "setup-wizard"],
  ])("renders over %s when the event fires", async (_label, showWizard, testid) => {
    vi.mocked(shouldShowWizard).mockResolvedValue(showWizard);
    renderApp();
    await screen.findByTestId(testid);

    fireFallback();

    const banner = screen.getByTestId("fallback-service-banner");
    expect(banner).toHaveTextContent(BOOT.fallbackBanner);
    // Both surfaces paint a full-viewport box of their own, so the banner is
    // fixed rather than stacked, or one of them would push it off screen.
    expect(banner).toHaveStyle({ position: "fixed" });
    // Under the macOS overlay title bar the traffic lights own the top strip;
    // the banner starts below it or its first words are hidden behind them.
    expect(banner).toHaveStyle({ top: `${dragStripHeight()}px` });
    expect(screen.getByTestId(testid)).toBeInTheDocument();
  });

  it("starts the service, proves it answers, and only then clears itself", async () => {
    renderApp();
    await screen.findByTestId("home-main");
    fireFallback();

    fireEvent.click(screen.getByTestId("fallback-start-service"));

    await waitFor(() => expect(startDaemonSidecarMock).toHaveBeenCalledTimes(1));
    // A spawn is not a service. The banner is the user's only warning that
    // nothing is syncing, so only a probe that came back may retire it.
    await waitFor(() => expect(getWireStateMock).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.queryByTestId("fallback-service-banner")).not.toBeInTheDocument(),
    );
  });

  it("keeps the banner when the spawn succeeds but the service still does not answer", async () => {
    getWireStateMock.mockResolvedValue({
      daemon: { reachable: false, error: "tcp connect error: connection refused" },
    });
    renderApp();
    await screen.findByTestId("home-main");
    fireFallback();

    fireEvent.click(screen.getByTestId("fallback-start-service"));

    await waitFor(() =>
      expect(screen.getByTestId("fallback-service-banner")).toHaveTextContent(
        "tcp connect error: connection refused",
      ),
    );
    expect(screen.getByTestId("fallback-service-banner")).toBeInTheDocument();
  });

  it("refuses a second start while the first is still in flight", async () => {
    let release: (() => void) | null = null;
    startDaemonSidecarMock.mockReturnValue(
      new Promise((resolve) => { release = () => resolve({ status: "started" }); }),
    );
    renderApp();
    await screen.findByTestId("home-main");
    fireFallback();

    const button = screen.getByTestId("fallback-start-service");
    fireEvent.click(button);
    fireEvent.click(button);
    fireEvent.click(button);

    expect(startDaemonSidecarMock).toHaveBeenCalledTimes(1);
    await act(async () => { release!(); });
  });

  it("keeps the banner and shows the raw reason when the start fails", async () => {
    startDaemonSidecarMock.mockResolvedValue({
      status: "failed",
      message: "launchctl bootstrap: Operation not permitted",
    });
    renderApp();
    await screen.findByTestId("home-main");
    fireFallback();

    fireEvent.click(screen.getByTestId("fallback-start-service"));

    await waitFor(() =>
      expect(screen.getByTestId("fallback-service-banner")).toHaveTextContent(
        "launchctl bootstrap: Operation not permitted",
      ),
    );
    expect(screen.getByTestId("fallback-service-banner")).toHaveTextContent(
      BOOT.startFailed,
    );
  });

  it("dismisses on request and comes back if Rust says it again", async () => {
    renderApp();
    await screen.findByTestId("home-main");
    fireFallback();

    fireEvent.click(screen.getByRole("button", { name: BOOT.dismiss }));
    expect(screen.queryByTestId("fallback-service-banner")).not.toBeInTheDocument();

    fireFallback();
    expect(screen.getByTestId("fallback-service-banner")).toBeInTheDocument();
  });
});
