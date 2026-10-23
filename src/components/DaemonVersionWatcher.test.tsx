// SPDX-License-Identifier: AGPL-3.0-only
import { act, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";

import type { DaemonVersionEvent } from "../lib/tauri";

const getStatusMock = vi.hoisted(() => vi.fn());

vi.mock("../lib/tauri", () => ({
  getDaemonVersionStatus: getStatusMock,
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

  it("reports a mismatch found at launch", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getServiceNotices().version).toEqual(mismatchedStatus));
  });

  it("follows daemon://version: a mismatch reports, a match clears", async () => {
    render(<DaemonVersionWatcher />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());

    fireVersionEvent({ ...mismatchedStatus, restarted: true, owner: "sidecar" });
    expect(getServiceNotices().version?.daemon).toBe("0.18.1");

    fireVersionEvent({ ...matchedStatus, restarted: true, owner: "sidecar" });
    expect(getServiceNotices().version).toBeNull();
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
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));

    getStatusMock.mockResolvedValue(mismatchedStatus);
    act(() => setDaemonStartupPhase("ready"));

    await waitFor(() => expect(getServiceNotices().version).toEqual(mismatchedStatus));
    expect(getStatusMock).toHaveBeenCalledTimes(2);
  });
});
