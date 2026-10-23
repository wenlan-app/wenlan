// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const { restartDaemonMock, startDaemonSidecarMock, getWireStateMock } = vi.hoisted(() => ({
  restartDaemonMock: vi.fn(),
  startDaemonSidecarMock: vi.fn(),
  getWireStateMock: vi.fn(),
}));
const emitMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ emit: emitMock }));
vi.mock("../../lib/tauri", () => ({
  restartDaemon: restartDaemonMock,
  startDaemonSidecar: startDaemonSidecarMock,
  getWireState: getWireStateMock,
}));

import "../../i18n";
import { resources } from "../../i18n/resources";
import { setDaemonStartupPhase } from "../../lib/daemonStartup";
import { getServiceNotices, setServiceFallback, setUpdateNotice, setVersionReport } from "../../lib/serviceNotices";
import { onUpdateAvailable, onUpdateProgress, onUpdateStatus } from "../../lib/appUpdate";
import ServiceStatus from "./ServiceStatus";

const BOOT = resources.en.translation.boot;
const SERVICE = resources.en.translation.daemonService;
const UPDATER = resources.en.translation.updater;
const OUT_OF_DATE = { daemon: "0.19.3", app: "0.20.0", matched: false, owner: "launchd", program: null } as const;

function renderStatus(node: ReactNode) {
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, "invalidateQueries");
  render(<QueryClientProvider client={queryClient}>{node}</QueryClientProvider>);
  return { invalidate };
}

const row = () => screen.getByTestId("service-status");
const action = () => screen.getByTestId("service-status-action");

describe("ServiceStatus", () => {
  beforeEach(() => {
    restartDaemonMock.mockReset().mockResolvedValue("0.20.0");
    startDaemonSidecarMock.mockReset().mockResolvedValue({ status: "started" });
    getWireStateMock.mockReset().mockResolvedValue({ daemon: { reachable: true, error: null } });
    emitMock.mockReset().mockResolvedValue(undefined);
  });
  afterEach(() => {
    act(() => {
      setDaemonStartupPhase("ready");
      setVersionReport(null);
      setServiceFallback(false);
      setUpdateNotice(null);
    });
  });

  it("says nothing when the service is fine", () => {
    renderStatus(<ServiceStatus />);
    expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
    expect(screen.queryByTestId("service-status-starting")).not.toBeInTheDocument();
  });

  it("shows one quiet line only while the daemon is starting", () => {
    renderStatus(<ServiceStatus />);
    act(() => setDaemonStartupPhase("starting"));
    expect(screen.getByTestId("service-status-starting")).toHaveTextContent(BOOT.startingInBackground);

    act(() => setDaemonStartupPhase("ready"));
    expect(screen.queryByTestId("service-status-starting")).not.toBeInTheDocument();
  });

  it("keeps only the dot in the icons sidebar, named for screen readers", () => {
    setDaemonStartupPhase("starting");
    renderStatus(<ServiceStatus compact />);
    const status = screen.getByRole("status", { name: BOOT.startingInBackground });
    expect(status).not.toHaveTextContent(BOOT.startingInBackground);
  });

  it.each([
    ["not answering", () => setDaemonStartupPhase("failed"), "notAnswering", BOOT.notAnsweringLabel, SERVICE.restart],
    ["out of date", () => setVersionReport(OUT_OF_DATE), "outOfDate", SERVICE.label, SERVICE.restart],
    ["not running", () => setServiceFallback(true), "notRunning", BOOT.fallbackLabel, BOOT.startService],
  ])("says %s in the same row, opened once with its one action", (_name, enter, kind, label, actionLabel) => {
    renderStatus(<ServiceStatus />);
    act(enter);

    expect(row()).toHaveAttribute("data-kind", kind);
    expect(row()).toHaveTextContent(label);
    expect(screen.getByTestId("service-status-popover")).toHaveTextContent(label);
    expect(action()).toHaveTextContent(actionLabel);

    // Closed by the user, it stays closed until asked for.
    fireEvent.click(row());
    expect(screen.queryByTestId("service-status-popover")).not.toBeInTheDocument();
    fireEvent.click(row());
    expect(screen.getByTestId("service-status-popover")).toBeInTheDocument();
  });

  it("says the one that matters most when several are true", () => {
    renderStatus(<ServiceStatus />);
    act(() => {
      setServiceFallback(true);
      setVersionReport(OUT_OF_DATE);
    });
    expect(row()).toHaveAttribute("data-kind", "outOfDate");

    act(() => setDaemonStartupPhase("failed"));
    expect(row()).toHaveAttribute("data-kind", "notAnswering");
  });

  it("names both versions when the service is out of date", () => {
    setVersionReport({ ...OUT_OF_DATE, program: "/opt/homebrew/bin/wenlan-server" });
    renderStatus(<ServiceStatus />);
    const popover = screen.getByTestId("service-status-popover");
    expect(popover).toHaveTextContent("0.19.3");
    expect(popover).toHaveTextContent("0.20.0");
    expect(popover).toHaveTextContent("/opt/homebrew/bin/wenlan-server");
  });

  it("goes quiet by itself when the problem goes away", () => {
    setVersionReport(OUT_OF_DATE);
    renderStatus(<ServiceStatus />);
    act(() => setVersionReport({ ...OUT_OF_DATE, daemon: "0.20.0", matched: true }));
    expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
  });

  it("restarts a service that is not answering and asks the gate again", async () => {
    setDaemonStartupPhase("failed");
    const { invalidate } = renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["shouldShowWizard"] }));
    expect(restartDaemonMock).toHaveBeenCalledTimes(1);
  });

  it("restarts an out-of-date service and clears the report", async () => {
    setVersionReport(OUT_OF_DATE);
    renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    await waitFor(() => expect(screen.queryByTestId("service-status")).not.toBeInTheDocument());
    expect(restartDaemonMock).toHaveBeenCalledTimes(1);
  });

  it("says why a restart failed, in the shared words", async () => {
    // Tauri rejects a command with the string it returned as the error.
    restartDaemonMock.mockRejectedValue("daemon-restart:not-owned");
    setVersionReport(OUT_OF_DATE);
    const { invalidate } = renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    expect(await screen.findByRole("alert")).toHaveTextContent(SERVICE.errors.notOwned);
    expect(row()).toHaveAttribute("data-kind", "outOfDate");
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("starts a service that is not running and clears only once it answers", async () => {
    setServiceFallback(true);
    renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    await waitFor(() => expect(getServiceNotices().fallback).toBe(false));
    expect(startDaemonSidecarMock).toHaveBeenCalledTimes(1);
    expect(getWireStateMock).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
  });

  it("keeps the warning when the start spawns but nothing answers", async () => {
    getWireStateMock.mockResolvedValue({ daemon: { reachable: false, error: "tcp connect error: connection refused" } });
    setServiceFallback(true);
    renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    expect(await screen.findByRole("alert")).toHaveTextContent("tcp connect error: connection refused");
    expect(row()).toHaveAttribute("data-kind", "notRunning");
  });

  it("shows the reason when the start fails", async () => {
    startDaemonSidecarMock.mockResolvedValue({ status: "failed", message: "launchctl bootstrap: Operation not permitted" });
    setServiceFallback(true);
    renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(BOOT.startFailed);
    expect(alert).toHaveTextContent("launchctl bootstrap: Operation not permitted");
  });

  it("refuses a second press while the first is still running", async () => {
    let release!: () => void;
    startDaemonSidecarMock.mockReturnValue(new Promise((resolve) => { release = () => resolve({ status: "started" }); }));
    setServiceFallback(true);
    renderStatus(<ServiceStatus />);

    fireEvent.click(action());
    fireEvent.click(action());
    fireEvent.click(action());

    expect(startDaemonSidecarMock).toHaveBeenCalledTimes(1);
    await act(async () => { release(); });
  });

  describe("a new version", () => {
    it("is offered in the same row, calm and closed until clicked", () => {
      renderStatus(<ServiceStatus />);
      act(() => onUpdateAvailable("0.20.1"));

      expect(row()).toHaveAttribute("data-kind", "updateAvailable");
      expect(row()).toHaveTextContent(UPDATER.available);
      expect(screen.queryByTestId("service-status-popover")).not.toBeInTheDocument();

      fireEvent.click(row());
      const popover = screen.getByTestId("service-status-popover");
      expect(popover).toHaveTextContent("Wenlan 0.20.1 is ready");
      expect(action()).toHaveTextContent(UPDATER.install);
      expect(screen.getByTestId("service-status-secondary")).toHaveTextContent(UPDATER.later);
    });

    it("gives way to a service problem", () => {
      renderStatus(<ServiceStatus />);
      act(() => {
        onUpdateAvailable("0.20.1");
        setVersionReport(OUT_OF_DATE);
      });
      expect(row()).toHaveAttribute("data-kind", "outOfDate");

      act(() => setVersionReport(null));
      expect(row()).toHaveAttribute("data-kind", "updateAvailable");
    });

    it("installs, then shows its progress as one quiet line", async () => {
      renderStatus(<ServiceStatus />);
      act(() => onUpdateAvailable("0.20.1"));
      fireEvent.click(row());

      fireEvent.click(action());

      await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://action", "install"));
      const line = screen.getByTestId("service-status-updating");
      expect(line).toHaveTextContent(UPDATER.updating);

      act(() => onUpdateProgress({ chunk: 0, total: 1000 }));
      act(() => onUpdateProgress({ chunk: 420 }));
      expect(line).toHaveTextContent("Updating 42%");
    });

    it("puts the version off and goes quiet", async () => {
      renderStatus(<ServiceStatus />);
      act(() => onUpdateAvailable("0.20.1"));
      fireEvent.click(row());

      fireEvent.click(screen.getByTestId("service-status-secondary"));

      await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://action", "later"));
      expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
    });

    it("turns into a warning when the install fails, and tries again in one press", async () => {
      renderStatus(<ServiceStatus />);
      act(() => onUpdateAvailable("0.20.1"));
      fireEvent.click(row());
      fireEvent.click(action());
      await waitFor(() => expect(screen.getByTestId("service-status-updating")).toBeInTheDocument());

      act(() => onUpdateProgress({ error: "network unavailable" }));
      expect(row()).toHaveAttribute("data-kind", "updateFailed");
      expect(screen.getByTestId("service-status-popover")).toHaveTextContent(UPDATER.failedBody);
      emitMock.mockClear();

      fireEvent.click(action());
      await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://check-now"));
      expect(screen.getByTestId("service-status-updating")).toBeInTheDocument();

      // The check offers the version again; "Try again" already said yes.
      act(() => onUpdateAvailable("0.20.1"));
      expect(emitMock).toHaveBeenCalledWith("updater://action", "install");
      expect(screen.getByTestId("service-status-updating")).toBeInTheDocument();
    });

    it("ignores background check reports when nothing was offered", () => {
      renderStatus(<ServiceStatus />);
      act(() => onUpdateStatus({ state: "error", error: "offline" }));
      expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
    });

    it("goes away when the app turns out to be current", () => {
      renderStatus(<ServiceStatus />);
      act(() => onUpdateAvailable("0.20.1"));
      act(() => onUpdateStatus({ state: "current" }));
      expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
    });
  });
});
