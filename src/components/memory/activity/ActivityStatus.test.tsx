// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ActivityStatus from "./ActivityStatus";
import type { ActivityResponse } from "../../../lib/tauri";

const getActivityMock = vi.hoisted(() => vi.fn());

vi.mock("../../../lib/tauri", () => ({ getActivity: getActivityMock }));

function activity(fields: Partial<ActivityResponse> = {}): ActivityResponse {
  return {
    state: "up_to_date",
    last_activity_at: null,
    assets: [],
    everyday: { job: "everyday", lane: "none", model: null, mode: "unconfigured", available: false },
    synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
    refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
    ...fields,
  };
}

function renderStatus(data: ActivityResponse, onOpenActivity = vi.fn()) {
  getActivityMock.mockResolvedValue(data);
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return {
    onOpenActivity,
    ...render(
      <QueryClientProvider client={queryClient}>
        <>
          <ActivityStatus onOpenActivity={onOpenActivity} />
          <button>Outside</button>
        </>
      </QueryClientProvider>,
    ),
  };
}

beforeEach(() => getActivityMock.mockReset());

describe("ActivityStatus", () => {
  it("keeps a neutral Pulse for idle and unconfigured activity, and opens the summary before Activity", async () => {
    const onOpenActivity = vi.fn();
    renderStatus(activity({ state: "blocked" }), onOpenActivity);

    const button = await screen.findByRole("button", { name: "Activity" });
    expect(button).not.toHaveAttribute("data-state");
    expect(screen.getByTestId("activity-status-icon")).toHaveAttribute("data-icon-kind", "pulse");
    await userEvent.click(button);
    expect(screen.getByRole("dialog")).toBeVisible();
    expect(onOpenActivity).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTestId("activity-summary-open"));
    expect(onOpenActivity).toHaveBeenCalledOnce();
  });

  it("uses the light pulse only when a real step is running", async () => {
    renderStatus(activity({
      state: "organizing",
      assets: [{
        kind: "memories",
        state: "running",
        done: 1,
        total: 2,
        blocked: 0,
        steps: [{ name: "summarize", state: "running", done: 1, total: 2, failed: 0, job: "everyday" }],
      }],
    }));

    const button = await screen.findByRole("button", { name: "Activity, Steeping" });
    expect(button).toHaveAttribute("data-state", "organizing");
    expect(screen.getByTestId("activity-status-icon")).toHaveClass("mem-activity-pulse-running");
    expect(screen.getByTestId("activity-status-icon")).toHaveAttribute("data-icon-kind", "pulse");
  });

  it("never says Steeping in On demand, even with steps waiting", async () => {
    renderStatus(activity({
      state: "off",
      everyday: { job: "everyday", lane: "on_device", model: "a-model", mode: "pinned", available: true },
      assets: [{
        kind: "memories",
        state: "running",
        done: 1,
        total: 2,
        blocked: 0,
        steps: [{ name: "summarize", state: "running", done: 1, total: 2, failed: 0, job: "everyday" }],
      }],
    }));

    const button = await screen.findByRole("button", { name: "Activity" });
    expect(button).not.toHaveAttribute("data-state");
    expect(screen.getByTestId("activity-status-icon")).not.toHaveClass("mem-activity-pulse-running");
  });

  it("shows attention only for a failed step and still opens the summary before Activity", async () => {
    const onOpenActivity = vi.fn();
    renderStatus(activity({
      state: "blocked",
      assets: [{
        kind: "pages",
        state: "blocked",
        done: 0,
        total: 3,
        blocked: 3,
        steps: [{ name: "write", state: "blocked", done: 0, total: 3, failed: 2, job: "synthesis" }],
      }],
    }), onOpenActivity);

    const button = await screen.findByRole("button", { name: "Activity, Failed" });
    expect(button).toHaveAttribute("data-state", "failed");
    expect(screen.getByTestId("activity-status-icon")).toHaveAttribute("data-icon-kind", "attention");
    await userEvent.click(button);
    expect(screen.getByRole("dialog")).toBeVisible();
    expect(onOpenActivity).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTestId("activity-summary-open"));
    expect(onOpenActivity).toHaveBeenCalledOnce();
  });


  it("hands forward Tab from the open trigger to the summary action, but leaves Shift+Tab alone", async () => {
    renderStatus(activity({ state: "blocked" }));
    const trigger = await screen.findByRole("button", { name: "Activity" });
    await userEvent.click(trigger);
    await screen.findByRole("dialog");
    expect(trigger).toHaveFocus();

    expect(fireEvent.keyDown(trigger, { key: "Tab", shiftKey: true })).toBe(true);
    expect(trigger).toHaveFocus();
    expect(fireEvent.keyDown(trigger, { key: "Tab" })).toBe(false);
    expect(screen.getByTestId("activity-summary-open")).toHaveFocus();

    await userEvent.tab();
    expect(screen.getByRole("button", { name: "Outside" })).toHaveFocus();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("hands forward Tab to the retry action when activity loading failed", async () => {
    getActivityMock.mockRejectedValueOnce(new Error("offline"));
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={queryClient}><ActivityStatus onOpenActivity={vi.fn()} /></QueryClientProvider>);

    const trigger = await screen.findByTestId("activity-status");
    await userEvent.click(trigger);
    await screen.findByRole("alert");
    const retry = screen.getByRole("button", { name: "Read again" });
    expect(fireEvent.keyDown(trigger, { key: "Tab" })).toBe(false);
    expect(retry).toHaveFocus();
  });

});
