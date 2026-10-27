// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";

const { tauriInvokeMock } = vi.hoisted(() => ({ tauriInvokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauriInvokeMock }));

import { daemonStartupWait, getDaemonStartupPhase, setDaemonStartupPhase } from "./daemonStartup";
import {
  getStartupPreflightStatus,
  getDaemonVersionStatus,
  listRecentPages,
  restartDaemon,
  shouldShowWizard,
  startDaemonSidecar,
} from "./tauri";

async function flush() {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

describe("daemon startup hold", () => {
  afterEach(() => {
    setDaemonStartupPhase("ready");
    tauriInvokeMock.mockReset();
  });

  it("sends calls at once while the daemon is ready", () => {
    tauriInvokeMock.mockResolvedValue([]);
    void listRecentPages(5);
    // Synchronous, as before the hold existed.
    expect(tauriInvokeMock).toHaveBeenCalledTimes(1);
  });

  it("holds daemon calls while starting and sends each once when it answers", async () => {
    tauriInvokeMock.mockResolvedValue([]);
    setDaemonStartupPhase("starting");

    const pages = listRecentPages(5);
    await flush();
    expect(tauriInvokeMock).not.toHaveBeenCalled();

    setDaemonStartupPhase("ready");
    await expect(pages).resolves.toEqual([]);
    expect(tauriInvokeMock).toHaveBeenCalledTimes(1);
  });

  it("lets the gate and the restart action through while starting", async () => {
    tauriInvokeMock.mockResolvedValue(false);
    setDaemonStartupPhase("starting");

    void shouldShowWizard();
    void startDaemonSidecar();
    expect(tauriInvokeMock.mock.calls.map((call) => call[0])).toEqual([
      "should_show_wizard",
      "start_daemon_sidecar",
    ]);
  });

  it("lets the startup preflight status through while starting", () => {
    tauriInvokeMock.mockResolvedValue("failed");
    setDaemonStartupPhase("starting");

    void getStartupPreflightStatus();

    expect(tauriInvokeMock).toHaveBeenCalledWith("get_startup_preflight_status");
  });

  it("lets version diagnosis and manual recovery through while starting", () => {
    tauriInvokeMock.mockResolvedValue({});
    setDaemonStartupPhase("starting");

    void getDaemonVersionStatus();
    void restartDaemon();

    expect(tauriInvokeMock.mock.calls.map((call) => call[0])).toEqual([
      "daemon_version_status",
      "restart_daemon",
    ]);
  });

  it("releases held calls on failure so their error states can show", async () => {
    tauriInvokeMock.mockRejectedValue(new Error("connection refused"));
    setDaemonStartupPhase("starting");

    const pages = listRecentPages(5);
    setDaemonStartupPhase("failed");
    await expect(pages).rejects.toThrow("connection refused");
    expect(daemonStartupWait("list_recent_pages")).toBeNull();
  });

  it("starts a fresh hold when a retry goes back to starting", () => {
    setDaemonStartupPhase("starting");
    const first = daemonStartupWait("list_recent_pages");
    setDaemonStartupPhase("failed");
    setDaemonStartupPhase("starting");
    const second = daemonStartupWait("list_recent_pages");
    expect(second).not.toBeNull();
    expect(second).not.toBe(first);
    expect(getDaemonStartupPhase()).toBe("starting");
  });
});
