// SPDX-License-Identifier: AGPL-3.0-only
import { act, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";

import type { DaemonVersionEvent } from "../lib/tauri";

const { getStatusMock, restartMock } = vi.hoisted(() => ({ getStatusMock: vi.fn(), restartMock: vi.fn() }));

vi.mock("../lib/tauri", () => ({
  getDaemonVersionStatus: getStatusMock,
  restartDaemon: restartMock,
}));

import DaemonVersionWatcher from "./DaemonVersionWatcher";
import { setDaemonStartupPhase } from "../lib/daemonStartup";
import { getServiceNotices, setVersionReport } from "../lib/serviceNotices";

const listenMock = vi.mocked(listen);

function fireVersionEvent(payload: DaemonVersionEvent) {
  const call = listenMock.mock.calls.find((args) => args[0] === "daemon://version");
  if (!call) throw new Error("watcher did not subscribe to daemon://version");
  act(() => (call[1] as (event: { payload: DaemonVersionEvent }) => void)({ payload }));
}

const matchedStatus = { daemon: "0.18.2", app: "0.18.2", matched: true, owner: "launchd", program: null } as const;
const mismatchedStatus = { daemon: "0.18.1", app: "0.18.2", matched: false, owner: "launchd", program: null } as const;

describe("DaemonVersionWatcher", () => {
  beforeEach(() => {
    getStatusMock.mockReset().mockResolvedValue(matchedStatus);
    restartMock.mockReset().mockResolvedValue("0.18.2");
    listenMock.mockClear();
    setVersionReport(null);
  });
  afterEach(() => {
    act(() => setDaemonStartupPhase("ready"));
  });

  it("reports nothing when the versions match", async () => {
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());
    expect(getServiceNotices().version).toBeNull();
  });

  it("shows a cached terminal failure on the first ready read without restarting again", async () => {
    const cachedFailure = {
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    };
    getStatusMock.mockResolvedValue(cachedFailure);

    render(<DaemonVersionWatcher />);

    await waitFor(() => expect(getServiceNotices().version).toMatchObject(cachedFailure));
    expect(restartMock).not.toHaveBeenCalled();
  });

  it("automatically recovers an initial mismatch without publishing a warning", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    let finishRestart!: (version: string) => void;
    restartMock.mockImplementation(() => new Promise<string>((resolve) => { finishRestart = resolve; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));
    expect(getServiceNotices().version).toBeNull();
    finishRestart("0.18.2");
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
  });

  it("keeps one automatic recovery quiet across a startup phase change", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    let finishRestart!: (version: string) => void;
    restartMock.mockImplementation(() => new Promise<string>((resolve) => { finishRestart = resolve; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));

    act(() => setDaemonStartupPhase("starting"));
    act(() => setDaemonStartupPhase("ready"));
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(2));
    expect(restartMock).toHaveBeenCalledTimes(1);
    expect(getServiceNotices().version).toBeNull();

    finishRestart("0.18.2");
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
  });

  it("does not show a cached terminal read while a newer automatic retry is pending", async () => {
    const cachedFailure = {
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    };
    getStatusMock
      .mockResolvedValueOnce(mismatchedStatus)
      .mockResolvedValueOnce(cachedFailure);
    let failRestart!: (error: string) => void;
    restartMock.mockImplementation(() => new Promise<string>((_resolve, reject) => { failRestart = reject; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));

    act(() => setDaemonStartupPhase("starting"));
    act(() => setDaemonStartupPhase("ready"));
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(2));
    expect(getServiceNotices().version).toBeNull();

    act(() => failRestart("newer retry failed"));
    await waitFor(() => expect(getServiceNotices().version).toMatchObject({
      restarted: true,
      error: "newer retry failed",
    }));
  });

  it("ignores a delayed starting probe after the ready-phase recovery begins", async () => {
    act(() => setDaemonStartupPhase("starting"));
    const cachedFailure = {
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    };
    let finishStartingProbe!: (status: typeof cachedFailure) => void;
    getStatusMock
      .mockImplementationOnce(() => new Promise((resolve) => { finishStartingProbe = resolve; }))
      .mockResolvedValueOnce(mismatchedStatus);
    let failRestart!: (error: string) => void;
    restartMock.mockImplementation(() => new Promise<string>((_resolve, reject) => { failRestart = reject; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));

    act(() => setDaemonStartupPhase("ready"));
    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));
    act(() => finishStartingProbe(cachedFailure));
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(2));
    expect(getServiceNotices().version).toBeNull();

    act(() => failRestart("ready recovery failed"));
    await waitFor(() => expect(getServiceNotices().version).toMatchObject({
      error: "ready recovery failed",
    }));
  });

  it("defers a failed startup event until the frontend retry returns its error", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    let failRestart!: (error: string) => void;
    restartMock.mockImplementation(() => new Promise<string>((_resolve, reject) => { failRestart = reject; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));

    fireVersionEvent({
      ...mismatchedStatus,
      restarted: true,
      owner: "launchd",
      error: "daemon-restart:still-mismatched",
    });
    expect(getServiceNotices().version).toBeNull();

    act(() => failRestart("fallback message"));
    await waitFor(() => expect(getServiceNotices().version).toMatchObject({
      daemon: "0.18.1",
      error: "daemon-restart:still-mismatched",
      restarted: true,
    }));
  });

  it("lets a matching event clear the row while retry is pending", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    let failRestart!: (error: string) => void;
    restartMock.mockImplementation(() => new Promise<string>((_resolve, reject) => { failRestart = reject; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));
    fireVersionEvent({ ...mismatchedStatus, restarted: true, owner: "launchd" });
    expect(getServiceNotices().version).toBeNull();

    fireVersionEvent({ ...matchedStatus, restarted: true, owner: "launchd" });
    expect(getServiceNotices().version).toBeNull();
    act(() => failRestart("stale restart failure"));
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
  });

  it("still checks the version if event-listener setup fails", async () => {
    listenMock.mockRejectedValueOnce(new Error("event plugin unavailable"));
    getStatusMock.mockResolvedValue(mismatchedStatus);
    restartMock.mockRejectedValue(new Error("restart failed"));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(getServiceNotices().version).toMatchObject({ error: "restart failed" }));
  });

  it("reports the mismatch only when automatic recovery fails", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    restartMock.mockRejectedValue("daemon-restart:still-mismatched");
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getServiceNotices().version).toMatchObject({
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    }));
  });

  it("publishes a terminal startup recovery failure while the startup gate is still waiting", async () => {
    act(() => setDaemonStartupPhase("starting"));
    getStatusMock.mockRejectedValue(new Error("connection refused after service restart"));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    fireVersionEvent({
      ...mismatchedStatus,
      restarted: true,
      owner: "launchd",
      error: "wenlan restart failed: daemon did not become healthy",
    });

    await waitFor(() => expect(getServiceNotices().version).toMatchObject({
      daemon: mismatchedStatus.daemon,
      restarted: true,
      error: "wenlan restart failed: daemon did not become healthy",
    }));
  });

  it("recovers a missed terminal startup event from the cached status without retrying", async () => {
    act(() => setDaemonStartupPhase("starting"));
    const cachedFailure = {
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    };
    getStatusMock
      .mockResolvedValueOnce(cachedFailure)
      .mockResolvedValueOnce(mismatchedStatus);
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getServiceNotices().version).toMatchObject(cachedFailure));

    act(() => setDaemonStartupPhase("ready"));

    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(2));
    expect(restartMock).not.toHaveBeenCalled();
    expect(getServiceNotices().version).toMatchObject(cachedFailure);
  });

  it("clears the terminal failure when the startup status probe confirms a match", async () => {
    act(() => setDaemonStartupPhase("starting"));
    setVersionReport({
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    });
    getStatusMock.mockResolvedValue(matchedStatus);
    render(<DaemonVersionWatcher />);

    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
    expect(restartMock).not.toHaveBeenCalled();
  });

  it("preserves the previous terminal failure when a retry event has no daemon version", async () => {
    const previousFailure = {
      ...mismatchedStatus,
      restarted: true,
      error: "daemon-restart:still-mismatched",
    };
    setVersionReport(previousFailure);
    getStatusMock.mockRejectedValueOnce(new Error("daemon unavailable"));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));

    fireVersionEvent({
      daemon: "",
      app: mismatchedStatus.app,
      matched: false,
      restarted: true,
      owner: "launchd",
      error: "daemon-restart:no-answer",
    });

    expect(getServiceNotices().version).toMatchObject(previousFailure);
  });

  it("does not overwrite a newer successful event with a stale mismatch read", async () => {
    let finishStatus!: (status: typeof mismatchedStatus) => void;
    getStatusMock.mockImplementation(() => new Promise((resolve) => { finishStatus = resolve; }));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());

    fireVersionEvent({ ...matchedStatus, restarted: true, owner: "sidecar" });
    finishStatus(mismatchedStatus);
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
    expect(restartMock).not.toHaveBeenCalled();
  });

  it("follows daemon://version: a mismatch reports, a match clears", async () => {
    getStatusMock.mockResolvedValueOnce(matchedStatus).mockResolvedValueOnce(mismatchedStatus);
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());

    fireVersionEvent({ ...mismatchedStatus, restarted: true, owner: "sidecar" });
    await waitFor(() => expect(getServiceNotices().version?.daemon).toBe("0.18.1"));

    fireVersionEvent({ ...matchedStatus, restarted: true, owner: "sidecar" });
    expect(getServiceNotices().version).toBeNull();
  });

  it("ignores a late failed startup event when the newer restart already matched", async () => {
    getStatusMock.mockResolvedValue(matchedStatus);
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));

    fireVersionEvent({
      ...mismatchedStatus,
      restarted: true,
      owner: "launchd",
      error: "daemon-restart:still-mismatched",
    });
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(getServiceNotices().version).toBeNull());
  });

  it("ignores a report sent while no daemon answered", async () => {
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());

    // A restart pressed with the daemon down: no daemon version to compare.
    fireVersionEvent({ daemon: "", app: "0.18.2", matched: false, restarted: false, owner: "launchd", program: null });
    expect(getServiceNotices().version).toBeNull();
  });

  it("asks again once the startup wait ends", async () => {
    act(() => setDaemonStartupPhase("failed"));
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));
    expect(getStatusMock).not.toHaveBeenCalled();

    getStatusMock.mockResolvedValue(mismatchedStatus);
    act(() => setDaemonStartupPhase("ready"));

    await waitFor(() => expect(restartMock).toHaveBeenCalledTimes(1));
    expect(getStatusMock).toHaveBeenCalledTimes(1);
  });
});
