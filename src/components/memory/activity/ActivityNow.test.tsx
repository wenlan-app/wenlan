// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import ActivityNow from "./ActivityNow";
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

function renderNow(data?: ActivityResponse | Promise<ActivityResponse>, onOpenIntelligence?: () => void) {
  if (data !== undefined) getActivityMock.mockReturnValue(data);
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <ActivityNow onOpenIntelligence={onOpenIntelligence} />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  getActivityMock.mockReset();
  await i18n.changeLanguage("en");
});

describe("ActivityNow", () => {
  it("does not call an unconfigured blocked system idle", async () => {
    renderNow(activity({
      state: "blocked",
      everyday: { job: "everyday", lane: "none", model: null, mode: "unconfigured", available: false },
      synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
    }));

    expect(await screen.findByTestId("activity-now-blocked")).toHaveTextContent(/paused|unavailable/i);
    expect(screen.getByText(/no model is set up/i)).toBeInTheDocument();
    expect(screen.getByText(/no writing model is set up/i)).toBeInTheDocument();
    expect(screen.queryByTestId("activity-now-idle")).toBeNull();
  });

  it("shows only real running steps", async () => {
    renderNow(activity({
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

    expect(await screen.findByTestId("activity-now-running")).toHaveTextContent("Steeping is running.");
    expect(screen.getByText("Summarize")).toBeInTheDocument();
    expect(screen.getByTestId("activity-now-running-step")).toHaveTextContent("Running");
    expect(screen.queryByTestId("activity-now-idle")).toBeNull();
  });

  it("keeps a known organizing state while per-step detail has not arrived", async () => {
    renderNow(activity({ state: "organizing" }));
    expect(await screen.findByTestId("activity-now-running")).toBeInTheDocument();
    expect(screen.queryByTestId("activity-now-running-step")).toBeNull();
    expect(screen.queryByTestId("activity-now-unknown")).toBeNull();
  });

  it("keeps provider details in Settings instead of the Activity overview", async () => {
    renderNow(activity({ everyday: { job: "everyday", lane: "anthropic", model: "selected-model", mode: "pinned", available: true } }));
    await screen.findByTestId("activity-now-idle");
    expect(screen.queryByText("Models and privacy")).toBeNull();
    expect(screen.queryByText(/selected-model/)).toBeNull();
  });

  it("offers Intelligence settings immediately when background work is blocked", async () => {
    const open = vi.fn();
    renderNow(activity({ state: "blocked" }), open);
    const button = await screen.findByRole("button", { name: "Open settings" });
    expect(button.closest("details")).toBeNull();
    await userEvent.click(button);
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("reports the actual failed count with the unit counted by that step", async () => {
    renderNow(activity({
      state: "blocked",
      assets: [{
        kind: "pages",
        state: "blocked",
        done: 0,
        total: 5,
        blocked: 5,
        steps: [{ name: "write", state: "blocked", done: 0, total: 5, failed: 2, job: "synthesis" }],
      }],
    }));

    expect(await screen.findByTestId("activity-now-failed")).toHaveTextContent(
      "2 pages could not be updated.",
    );
    expect(screen.queryByTestId("activity-now-idle")).toBeNull();
  });

  it("keeps Detect counts in memories and Confirm counts in entities", async () => {
    renderNow(activity({
      state: "organizing",
      assets: [{
        kind: "entities", state: "running", done: 0, total: 4, blocked: 0,
        steps: [
          { name: "detect", state: "running", done: 1, total: 4, failed: 0, job: "everyday" },
          { name: "confirm", state: "idle", done: 2, total: 2, failed: 0, job: null },
        ],
      }],
      everyday: { job: "everyday", lane: "on_device", model: "local", mode: "pinned", available: true },
    }));

    expect(await screen.findByText("Detect")).toBeInTheDocument();
    expect(screen.getByText("1 / 4 memories")).toBeInTheDocument();
    expect(screen.getByText("Confirm")).toBeInTheDocument();
    expect(screen.getByText("2 / 2 entities")).toBeInTheDocument();
  });

  it.each([
    { language: "en", notes: "Pages", idle: "Idle" },
    { language: "zh-Hans", notes: "笔记", idle: "待机" },
    { language: "zh-Hant", notes: "筆記", idle: "待機" },
  ])("localizes the always-visible Pages row in $language", async ({ language, notes, idle }) => {
    await i18n.changeLanguage(language);
    renderNow(activity({
      assets: [{
        kind: "pages", state: "idle", done: 0, total: 0, blocked: 0,
        steps: [{ name: "write", state: "idle", done: 0, total: 0, failed: 0, job: "synthesis" }],
      }],
    }));

    expect(await screen.findByRole("heading", { name: notes })).toBeInTheDocument();
    expect(screen.getAllByText(idle)).toHaveLength(2);
  });

  it("says waiting for an idle computer without claiming work is running", async () => {
    renderNow(activity({ state: "waiting_for_idle" }));
    expect(await screen.findByTestId("activity-now-waiting")).toHaveTextContent(
      "Waiting for your computer to be idle.",
    );
    expect(screen.queryByTestId("activity-now-running")).toBeNull();
  });

  it("uses neutral paused wording for a selected but unavailable model", async () => {
    renderNow(activity({
      state: "blocked",
      everyday: { job: "everyday", lane: "external", model: "chosen", mode: "pinned_unavailable", available: false },
    }));
    expect(await screen.findByTestId("activity-now-blocked")).toHaveTextContent(/paused|unavailable/i);
    expect(screen.getByText(/its model is unavailable/i)).toBeInTheDocument();
    expect(screen.queryByText(/no .* model|choose a model|turn on/i)).toBeNull();
  });

  it("shows compact labeled asset and step rows while keeping long asset detail optional", async () => {
    renderNow(activity({
      state: "organizing",
      assets: [
        {
          kind: "memories", state: "running", done: 1, total: 2, blocked: 1,
          steps: [
            { name: "summarize", state: "running", done: 1, total: 2, failed: 0, job: "everyday" },
            { name: "link", state: "blocked", done: 1, total: 2, failed: 1, job: "everyday" },
          ],
        },
      ],
      everyday: { job: "everyday", lane: "on_device", model: "local", mode: "pinned", available: true },
    }));

    expect(await screen.findByTestId("activity-now-running")).toBeInTheDocument();
    expect(screen.getByTestId("activity-now-failed")).toHaveTextContent("1 memory could not be linked.");
    expect(screen.getByRole("heading", { name: "Memories" })).toBeInTheDocument();
    expect(screen.getByText("Summarize")).toBeInTheDocument();
    expect(screen.getByText("Link")).toBeInTheDocument();
    expect(screen.getAllByText("1 / 2 memories")).toHaveLength(2);
    expect(screen.queryByText("Details")).toBeNull();
    expect(screen.getByText(/searchable/i)).toBeVisible();
  });

  it("distinguishes the turned-off state from idle", async () => {
    renderNow(activity({ state: "off" }));
    expect(await screen.findByTestId("activity-now-off")).toHaveTextContent("Steeping runs on demand.");
    expect(screen.queryByTestId("activity-now-idle")).toBeNull();
  });

  it.each([
    { kind: "unknown state", payload: activity({ state: "unknown" }) },
    { kind: "unknown asset", payload: activity({ assets: [{ kind: "unknown", state: "idle", done: 0, total: 0, blocked: 0, steps: [] }] }) },
    { kind: "unknown step state", payload: activity({ assets: [{ kind: "memories", state: "unknown", done: 0, total: 0, blocked: 0, steps: [] }] }) },
    { kind: "unknown step", payload: activity({ assets: [{ kind: "memories", state: "idle", done: 0, total: 0, blocked: 0, steps: [{ name: "unknown", state: "idle", done: 0, total: 0, failed: 0, job: null }] }] }) },
  ])("does not call $kind payload idle", async ({ payload }) => {
    renderNow(payload);
    expect(await screen.findByTestId("activity-now-unknown")).toBeInTheDocument();
    expect(screen.queryByTestId("activity-now-idle")).toBeNull();
  });

  it("shows read errors and offers a real reread", async () => {
    getActivityMock.mockRejectedValueOnce(new Error("read failed"));
    getActivityMock.mockResolvedValueOnce(activity());
    renderNow();

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not read current work.");
    await userEvent.click(screen.getByRole("button", { name: "Read again" }));
    expect(await screen.findByTestId("activity-now-idle")).toBeInTheDocument();
  });
});
