// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it } from "vitest";
import { act, render, screen } from "@testing-library/react";
import "../../i18n";
import { resources } from "../../i18n/resources";
import { setDaemonStartupPhase } from "../../lib/daemonStartup";
import DaemonStartupStatus from "./DaemonStartupStatus";

const LABEL = resources.en.translation.boot.startingInBackground;

describe("DaemonStartupStatus", () => {
  afterEach(() => setDaemonStartupPhase("ready"));

  it("shows one line only while the daemon is starting", () => {
    render(<DaemonStartupStatus />);
    expect(screen.queryByTestId("daemon-startup-status")).not.toBeInTheDocument();

    act(() => setDaemonStartupPhase("starting"));
    expect(screen.getByTestId("daemon-startup-status")).toHaveTextContent(LABEL);

    act(() => setDaemonStartupPhase("ready"));
    expect(screen.queryByTestId("daemon-startup-status")).not.toBeInTheDocument();
  });

  it("leaves a failure to the banner", () => {
    setDaemonStartupPhase("starting");
    render(<DaemonStartupStatus />);
    act(() => setDaemonStartupPhase("failed"));
    expect(screen.queryByTestId("daemon-startup-status")).not.toBeInTheDocument();
  });

  it("keeps only the dot in the icons sidebar, named for screen readers", () => {
    setDaemonStartupPhase("starting");
    render(<DaemonStartupStatus compact />);
    const status = screen.getByRole("status", { name: LABEL });
    expect(status).not.toHaveTextContent(LABEL);
  });
});
