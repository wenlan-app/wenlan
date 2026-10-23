// SPDX-License-Identifier: AGPL-3.0-only
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";

import type { DaemonVersionEvent } from "../lib/tauri";

const getStatusMock = vi.hoisted(() => vi.fn());
const restartMock = vi.hoisted(() => vi.fn());

vi.mock("../lib/tauri", () => ({
  getDaemonVersionStatus: getStatusMock,
  restartDaemon: restartMock,
}));

import DaemonVersionBanner from "./DaemonVersionBanner";
import { setDaemonStartupPhase } from "../lib/daemonStartup";

const listenMock = vi.mocked(listen);

function eventHandler() {
  const call = listenMock.mock.calls.find((args) => args[0] === "daemon://version");
  if (!call) throw new Error("banner did not subscribe to daemon://version");
  return call[1] as (event: { payload: DaemonVersionEvent }) => void;
}

function fireVersionEvent(payload: DaemonVersionEvent) {
  eventHandler()({ payload });
}

const matchedStatus = {
  daemon: "0.18.2",
  app: "0.18.2",
  matched: true,
  owner: "launchd",
  program: null,
};

const mismatchedStatus = {
  daemon: "0.18.1",
  app: "0.18.2",
  matched: false,
  owner: "launchd",
  program: null,
};

describe("DaemonVersionBanner", () => {
  beforeEach(() => {
    getStatusMock.mockReset();
    restartMock.mockReset();
    listenMock.mockClear();
    getStatusMock.mockResolvedValue(matchedStatus);
    restartMock.mockResolvedValue("0.18.2");
  });

  it("renders nothing when the versions match", async () => {
    render(<DaemonVersionBanner />);

    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());
    expect(screen.queryByTestId("daemon-version-banner")).not.toBeInTheDocument();
  });

  it("renders both versions when the service is out of date", async () => {
    getStatusMock.mockResolvedValue(mismatchedStatus);
    render(<DaemonVersionBanner />);

    const banner = await screen.findByTestId("daemon-version-banner");
    expect(banner).toHaveTextContent("Wenlan's background service is out of date");
    expect(banner).toHaveTextContent("0.18.1");
    expect(banner).toHaveTextContent("0.18.2");
    expect(screen.queryByTestId("daemon-version-program")).not.toBeInTheDocument();
  });

  it("adds the program sentence when the plist program is outside the bundle", async () => {
    getStatusMock.mockResolvedValue({
      ...mismatchedStatus,
      program: "/opt/homebrew/bin/wenlan-server",
    });
    render(<DaemonVersionBanner />);

    const program = await screen.findByTestId("daemon-version-program");
    expect(program).toHaveTextContent("/opt/homebrew/bin/wenlan-server");
    expect(program).toHaveTextContent("Homebrew or npm");
  });

  it("appears on a mismatched daemon://version event and hides on a matched one", async () => {
    render(<DaemonVersionBanner />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalled());

    fireVersionEvent({
      daemon: "0.18.1",
      app: "0.18.2",
      matched: false,
      restarted: true,
      owner: "sidecar",
      program: null,
    });
    expect(await screen.findByTestId("daemon-version-banner")).toBeInTheDocument();

    fireVersionEvent({
      daemon: "0.18.2",
      app: "0.18.2",
      matched: true,
      restarted: true,
      owner: "sidecar",
    });
    await waitFor(() =>
      expect(screen.queryByTestId("daemon-version-banner")).not.toBeInTheDocument(),
    );
  });

  it("drops a report sent while the daemon was not answering once it answers", async () => {
    act(() => setDaemonStartupPhase("failed"));
    render(<DaemonVersionBanner />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));

    // A restart pressed in the sidebar reports before any daemon answers.
    act(() => fireVersionEvent({ daemon: "", app: "0.18.2", matched: false, restarted: false, owner: "launchd", program: null }));
    expect(screen.queryByTestId("daemon-version-banner")).not.toBeInTheDocument();

    act(() => setDaemonStartupPhase("ready"));
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("daemon-version-banner")).not.toBeInTheDocument();
  });

  it("still warns after the wait when the daemon that answers is out of date", async () => {
    act(() => setDaemonStartupPhase("starting"));
    render(<DaemonVersionBanner />);
    await waitFor(() => expect(getStatusMock).toHaveBeenCalledTimes(1));

    getStatusMock.mockResolvedValue(mismatchedStatus);
    act(() => setDaemonStartupPhase("ready"));

    expect(await screen.findByTestId("daemon-version-banner")).toHaveTextContent("0.18.1");
  });

  it("invokes restart_daemon and hides the banner on success", async () => {
    const user = userEvent.setup();
    getStatusMock.mockResolvedValue(mismatchedStatus);
    render(<DaemonVersionBanner />);

    await user.click(await screen.findByTestId("daemon-version-restart"));

    await waitFor(() => expect(restartMock).toHaveBeenCalledWith());
    await waitFor(() =>
      expect(screen.queryByTestId("daemon-version-banner")).not.toBeInTheDocument(),
    );
  });

  it("shows the returned message inline when the restart fails", async () => {
    const user = userEvent.setup();
    getStatusMock.mockResolvedValue(mismatchedStatus);
    restartMock.mockRejectedValue(new Error("daemon did not become healthy after restart"));
    render(<DaemonVersionBanner />);

    await user.click(await screen.findByTestId("daemon-version-restart"));

    const error = await screen.findByTestId("daemon-version-error");
    expect(error).toHaveTextContent("daemon did not become healthy after restart");
    expect(screen.getByTestId("daemon-version-banner")).toBeInTheDocument();
  });

  it("localizes a typed restart error instead of showing the code", async () => {
    const user = userEvent.setup();
    getStatusMock.mockResolvedValue(mismatchedStatus);
    restartMock.mockRejectedValue("daemon-restart:not-owned");
    render(<DaemonVersionBanner />);

    await user.click(await screen.findByTestId("daemon-version-restart"));

    const error = await screen.findByTestId("daemon-version-error");
    expect(error).toHaveTextContent("was not started by this app");
    expect(error).not.toHaveTextContent("daemon-restart:not-owned");
  });

  it("dismisses for the session only", async () => {
    const user = userEvent.setup();
    getStatusMock.mockResolvedValue(mismatchedStatus);
    render(<DaemonVersionBanner />);

    await user.click(await screen.findByTestId("daemon-version-dismiss"));
    expect(screen.queryByTestId("daemon-version-banner")).not.toBeInTheDocument();
  });
});
