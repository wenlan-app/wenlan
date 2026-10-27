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
vi.mock("../../lib/tauri", () => ({
  restartDaemon: restartDaemonMock,
  startDaemonSidecar: startDaemonSidecarMock,
  getWireState: getWireStateMock,
}));

import "../../i18n";
import { resources } from "../../i18n/resources";
import { setDaemonStartupPhase } from "../../lib/daemonStartup";
import { getServiceNotices, setServiceFallback, setUpdateNotice, setVersionReport } from "../../lib/serviceNotices";
import ServiceStatus from "./ServiceStatus";

const BOOT = resources.en.translation.boot;
const SERVICE = resources.en.translation.daemonService;
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

  it("shows an automatic recovery failure and leaves the retry action available", () => {
    setVersionReport({ ...OUT_OF_DATE, restarted: true, error: "daemon-restart:still-mismatched" });
    renderStatus(<ServiceStatus compact />);
    expect(screen.getByRole("alert")).toHaveTextContent(SERVICE.errors.stillMismatched);
    expect(action()).toHaveTextContent(SERVICE.restart);
    expect(row()).toHaveAttribute("data-kind", "outOfDate");
  });

  it.each([
    "daemon-restart:sidecar-still-running: pid still alive",
    "daemon-restart:sidecar-stop-unverified: permission denied",
  ])("explains how to recover when the app cannot confirm its sidecar stopped (%s)", (error) => {
    setVersionReport({ ...OUT_OF_DATE, restarted: false, error });
    renderStatus(<ServiceStatus compact />);

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent(SERVICE.errors.sidecarStopUnconfirmed);
    expect(alert).not.toHaveTextContent("pid still alive");
    expect(alert).not.toHaveTextContent("permission denied");
    expect(action()).toBeEnabled();
  });

  it("keeps raw service-manager diagnostics out of the notice", () => {
    const rawError = `launchctl print gui/501/com.wenlan.server\n${"diagnostic line\n".repeat(80)}`;
    setVersionReport({ ...OUT_OF_DATE, restarted: true, error: rawError });
    renderStatus(<ServiceStatus compact />);

    expect(screen.getByRole("alert")).toHaveTextContent(BOOT.startFailed);
    expect(screen.getByRole("alert")).not.toHaveTextContent("launchctl print");
    expect(screen.getByRole("alert").textContent?.length).toBeLessThan(100);
  });

  it("uses the typed still-mismatched message when a terminal report has no detail", () => {
    setVersionReport({ ...OUT_OF_DATE, restarted: true, error: null });
    renderStatus(<ServiceStatus compact />);

    expect(screen.getByRole("alert")).toHaveTextContent(SERVICE.errors.stillMismatched);
  });

  it("shows a failed automatic version recovery while the startup gate is still waiting", () => {
    act(() => setDaemonStartupPhase("starting"));
    setVersionReport({ ...OUT_OF_DATE, restarted: true, error: "daemon-restart:still-mismatched" });

    renderStatus(<ServiceStatus compact />);

    expect(row()).toHaveAttribute("data-kind", "outOfDate");
    expect(screen.getByRole("alert")).toHaveTextContent(SERVICE.errors.stillMismatched);
    expect(action()).toBeEnabled();
  });

  it("keeps the version retry action when startup later gives up", () => {
    act(() => setDaemonStartupPhase("failed"));
    setVersionReport({ ...OUT_OF_DATE, restarted: true, error: "daemon-restart:still-mismatched" });

    renderStatus(<ServiceStatus />);

    expect(row()).toHaveAttribute("data-kind", "outOfDate");
    expect(screen.getByRole("alert")).toHaveTextContent(SERVICE.errors.stillMismatched);
    expect(action()).toBeEnabled();
  });

  it("keeps an ordinary mismatch quiet while the startup gate is still waiting", () => {
    act(() => setDaemonStartupPhase("starting"));
    setVersionReport(OUT_OF_DATE);

    renderStatus(<ServiceStatus />);

    expect(screen.getByTestId("service-status-starting")).toBeInTheDocument();
    expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
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
    const { invalidate } = renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    await waitFor(() => expect(screen.queryByTestId("service-status")).not.toBeInTheDocument());
    expect(restartDaemonMock).toHaveBeenCalledTimes(1);
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["shouldShowWizard"] });
  });

  it.each(["starting", "failed"] as const)("refreshes the startup gate after a version retry succeeds while %s", async (phase) => {
    act(() => setDaemonStartupPhase(phase));
    setVersionReport({ ...OUT_OF_DATE, restarted: true, error: "daemon-restart:still-mismatched" });
    const { invalidate } = renderStatus(<ServiceStatus />);

    fireEvent.click(action());

    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["shouldShowWizard"] }));
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
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

  describe("in Activity's place", () => {
    const activity = <button type="button" data-testid="activity-status">Activity</button>;

    it("leaves the slot to Activity when the service is fine", () => {
      renderStatus(<ServiceStatus>{activity}</ServiceStatus>);
      expect(screen.getByTestId("activity-status")).toBeInTheDocument();
    });

    it("takes the slot while the service starts, then gives it back", () => {
      renderStatus(<ServiceStatus>{activity}</ServiceStatus>);
      act(() => setDaemonStartupPhase("starting"));
      expect(screen.queryByTestId("activity-status")).not.toBeInTheDocument();
      expect(screen.getByTestId("service-status-starting")).toHaveTextContent(BOOT.startingInBackground);

      act(() => setDaemonStartupPhase("ready"));
      expect(screen.getByTestId("activity-status")).toBeInTheDocument();
    });

    it("takes the slot while something needs the user", () => {
      renderStatus(<ServiceStatus>{activity}</ServiceStatus>);
      act(() => setVersionReport(OUT_OF_DATE));
      expect(screen.queryByTestId("activity-status")).not.toBeInTheDocument();
      expect(row()).toHaveAttribute("data-kind", "outOfDate");
    });

    it("never speaks about an app update, which lives on the account entry", () => {
      renderStatus(<ServiceStatus>{activity}</ServiceStatus>);
      for (const phase of ["available", "installing", "failed"] as const) {
        act(() => setUpdateNotice({ phase, version: "0.20.1", pct: 42 }));
        expect(screen.queryByTestId("service-status")).not.toBeInTheDocument();
        expect(screen.getByTestId("activity-status")).toBeInTheDocument();
      }
    });
  });
});
