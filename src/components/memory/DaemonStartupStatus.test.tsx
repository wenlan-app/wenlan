// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const { restartDaemonMock } = vi.hoisted(() => ({ restartDaemonMock: vi.fn() }));
vi.mock("../../lib/tauri", () => ({ restartDaemon: restartDaemonMock }));

import "../../i18n";
import { resources } from "../../i18n/resources";
import { setDaemonStartupPhase } from "../../lib/daemonStartup";
import DaemonStartupStatus from "./DaemonStartupStatus";

const BOOT = resources.en.translation.boot;
const SERVICE = resources.en.translation.daemonService;

function renderStatus(node: ReactNode) {
  const queryClient = new QueryClient();
  const invalidate = vi.spyOn(queryClient, "invalidateQueries");
  render(<QueryClientProvider client={queryClient}>{node}</QueryClientProvider>);
  return { invalidate };
}

describe("DaemonStartupStatus", () => {
  beforeEach(() => {
    restartDaemonMock.mockReset().mockResolvedValue("0.19.3");
  });
  afterEach(() => {
    act(() => setDaemonStartupPhase("ready"));
  });

  it("shows one quiet line only while the daemon is starting", () => {
    renderStatus(<DaemonStartupStatus />);
    expect(screen.queryByTestId("daemon-startup-status")).not.toBeInTheDocument();

    act(() => setDaemonStartupPhase("starting"));
    expect(screen.getByTestId("daemon-startup-status")).toHaveTextContent(BOOT.startingInBackground);

    act(() => setDaemonStartupPhase("ready"));
    expect(screen.queryByTestId("daemon-startup-status")).not.toBeInTheDocument();
  });

  it("keeps only the dot in the icons sidebar, named for screen readers", () => {
    setDaemonStartupPhase("starting");
    renderStatus(<DaemonStartupStatus compact />);
    const status = screen.getByRole("status", { name: BOOT.startingInBackground });
    expect(status).not.toHaveTextContent(BOOT.startingInBackground);
  });

  it("turns the same row into a warning that opens once with Restart service", () => {
    setDaemonStartupPhase("starting");
    renderStatus(<DaemonStartupStatus />);
    act(() => setDaemonStartupPhase("failed"));

    expect(screen.queryByTestId("daemon-startup-status")).not.toBeInTheDocument();
    expect(screen.getByTestId("daemon-not-answering")).toHaveTextContent(BOOT.notAnsweringLabel);
    const popover = screen.getByTestId("daemon-not-answering-popover");
    expect(popover).toHaveTextContent(BOOT.notAnsweringBody);
    expect(screen.getByTestId("daemon-not-answering-restart")).toHaveTextContent(SERVICE.restart);

    // Closed by the user, it stays closed until asked for.
    fireEvent.click(screen.getByTestId("daemon-not-answering"));
    expect(screen.queryByTestId("daemon-not-answering-popover")).not.toBeInTheDocument();
  });

  it("restarts the service and asks the gate again", async () => {
    setDaemonStartupPhase("failed");
    const { invalidate } = renderStatus(<DaemonStartupStatus />);

    fireEvent.click(screen.getByTestId("daemon-not-answering-restart"));

    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: ["shouldShowWizard"] }));
    expect(restartDaemonMock).toHaveBeenCalledTimes(1);
  });

  it("says why when the restart fails, in the same words as the version notice", async () => {
    // Tauri rejects a command with the string it returned as the error.
    restartDaemonMock.mockRejectedValue("daemon-restart:not-owned");
    setDaemonStartupPhase("failed");
    const { invalidate } = renderStatus(<DaemonStartupStatus />);

    fireEvent.click(screen.getByTestId("daemon-not-answering-restart"));

    expect(await screen.findByRole("alert")).toHaveTextContent(SERVICE.errors.notOwned);
    expect(invalidate).not.toHaveBeenCalled();
  });
});
