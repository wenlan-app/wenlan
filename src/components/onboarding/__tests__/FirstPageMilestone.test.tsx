import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getPage, listPagesExplicitBrowse, type MilestoneRecord, type Page } from "../../../lib/tauri";
import { FirstPageMilestone } from "../FirstPageMilestone";

const milestoneState = vi.hoisted(() => ({
  milestones: [] as MilestoneRecord[],
  acknowledge: vi.fn(),
}));
vi.mock("../useMilestones", () => ({
  useMilestones: () => milestoneState,
}));
vi.mock("../../../lib/tauri", () => ({
  getPage: vi.fn(),
  listPagesExplicitBrowse: vi.fn(),
}));

const target: Page = {
  id: "first-page", title: "Background knowledge", content: "First body", summary: null,
  entity_id: null, domain: null, space: null, source_memory_ids: ["source-1"], version: 1,
  status: "active", creation_kind: "compiled", review_status: "unconfirmed",
  created_at: "", last_compiled: "", last_modified: "",
};
const pending: MilestoneRecord = {
  id: "first-concept", first_triggered_at: 1, acknowledged_at: null,
  payload: { page_id: target.id },
};

function renderMilestone(pages: readonly Page[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onSelectPage = vi.fn();
  const element = (nextPages: readonly Page[]) => (
    <QueryClientProvider client={client}>
      <FirstPageMilestone pages={nextPages} onSelectPage={onSelectPage} />
    </QueryClientProvider>
  );
  const result = render(element(pages));
  return { ...result, client, onSelectPage, refresh: (nextPages: readonly Page[] = pages) => result.rerender(element(nextPages)) };
}

beforeEach(() => {
  vi.resetAllMocks();
  milestoneState.milestones = [];
  localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("FirstPageMilestone passive target lookup", () => {
  it("opens the modal when a milestone arrives after Wiki's empty snapshot without an explicit browse", async () => {
    vi.mocked(getPage).mockResolvedValue(target);
    const view = renderMilestone();
    expect(getPage).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    milestoneState.milestones = [pending];
    view.refresh();
    expect(await screen.findByRole("dialog", { name: target.title })).toBeInTheDocument();
    expect(getPage).toHaveBeenCalledTimes(1);
    expect(getPage).toHaveBeenCalledWith(target.id);
    expect(listPagesExplicitBrowse).not.toHaveBeenCalled();
    expect(milestoneState.acknowledge).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Open page" }));
    expect(view.onSelectPage).toHaveBeenCalledWith(target.id);
    expect(milestoneState.acknowledge).toHaveBeenCalledWith("first-concept");
  });

  it("uses a supplied target page without any extra request", async () => {
    milestoneState.milestones = [pending];
    renderMilestone([target]);
    expect(await screen.findByRole("dialog", { name: target.title })).toBeInTheDocument();
    expect(getPage).not.toHaveBeenCalled();
    expect(listPagesExplicitBrowse).not.toHaveBeenCalled();
  });

  it("polls a temporarily absent target at a bounded interval and stops once it is found", async () => {
    vi.useFakeTimers();
    milestoneState.milestones = [pending];
    vi.mocked(getPage).mockResolvedValueOnce(null).mockResolvedValue(target);
    renderMilestone();
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(getPage).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(milestoneState.acknowledge).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(getPage).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("dialog", { name: target.title })).toBeInTheDocument();
    expect(milestoneState.acknowledge).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(getPage).toHaveBeenCalledTimes(2);
    expect(listPagesExplicitBrowse).not.toHaveBeenCalled();
  });

  it("stops polling on acknowledgment and ignores the departed milestone's late response", async () => {
    let resolve!: (page: Page) => void;
    vi.mocked(getPage).mockReturnValue(new Promise((done) => { resolve = done; }));
    milestoneState.milestones = [pending];
    const view = renderMilestone();
    await waitFor(() => expect(getPage).toHaveBeenCalledTimes(1));
    milestoneState.milestones = [{ ...pending, acknowledged_at: 2 }];
    view.refresh();
    await act(async () => resolve(target));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    vi.useFakeTimers();
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(getPage).toHaveBeenCalledTimes(1);
  });
});
