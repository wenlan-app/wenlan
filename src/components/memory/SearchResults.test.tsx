// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import { SearchResults } from "./SearchResults";
import type { ActivityResponse, ActivityStep, SearchResult } from "../../lib/tauri";

const getActivityMock = vi.hoisted(() => vi.fn());
vi.mock("../../lib/tauri", () => ({ getActivity: getActivityMock }));

function activity(embed?: ActivityStep): ActivityResponse {
  return {
    state: "off",
    last_activity_at: null,
    assets: [{
      kind: "memories",
      state: embed?.state ?? "idle",
      done: 3,
      total: 3,
      blocked: 0,
      steps: [
        { name: "store", state: "idle", done: 3, total: 3, failed: 0, job: null },
        ...(embed ? [embed] : []),
      ],
    }],
    everyday: { job: "everyday", lane: "none", model: null, mode: "unconfigured", available: false },
    synthesis: { job: "synthesis", lane: "none", model: null, mode: "unconfigured", available: false },
    refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
  };
}

const embed = (state: ActivityStep["state"], done: number, total: number): ActivityStep =>
  ({ name: "embed", state, done, total, failed: 0, job: null });

const result = { id: "m1", content: "delta grid checklist" } as SearchResult;

function renderResults(data: ActivityResponse, memoryResults: SearchResult[] = [result]) {
  getActivityMock.mockResolvedValue(data);
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <SearchResults
        query="delta"
        memoryResults={memoryResults}
        sourceResults={[]}
        entityResults={[]}
        conceptResults={[]}
        loading={false}
        error={false}
        ready
        onOpenResult={() => {}}
        onOpenPage={() => {}}
        onOpenEntity={() => {}}
      />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  getActivityMock.mockReset();
  await i18n.changeLanguage("en");
});

describe("SearchResults preparing line", () => {
  it("says how many passages are still being prepared while the step runs", async () => {
    renderResults(activity(embed("running", 700, 1000)));
    expect(await screen.findByTestId("search-preparing")).toHaveTextContent(
      "Still preparing 300 passages. Keyword matches are complete.",
    );
    expect(screen.getByTestId("search-preparing")).toHaveClass("search-modal-status");
    // Above the results, so ten results do not push it out of view.
    const line = screen.getByTestId("search-preparing");
    const firstResult = screen.getByRole("button");
    expect(line.compareDocumentPosition(firstResult) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("shows the line under an empty result too", async () => {
    renderResults(activity(embed("running", 0, 1)), []);
    expect(await screen.findByTestId("search-preparing")).toHaveTextContent(
      "Still preparing 1 passage. Keyword matches are complete.",
    );
    expect(screen.getByText("0 results for “delta”")).toBeInTheDocument();
  });

  it.each([
    { kind: "an idle step", data: activity(embed("idle", 0, 0)) },
    { kind: "a daemon without the step", data: activity() },
  ])("stays hidden with $kind", async ({ data }) => {
    renderResults(data);
    await vi.waitFor(() => expect(getActivityMock).toHaveBeenCalled());
    // Let the query settle before asserting the line never appears.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(screen.getByRole("button")).toHaveTextContent("delta grid checklist");
    expect(screen.queryByTestId("search-preparing")).toBeNull();
  });

  it("uses the Traditional Chinese copy", async () => {
    await i18n.changeLanguage("zh-Hant");
    renderResults(activity(embed("running", 5, 17)));
    expect(await screen.findByTestId("search-preparing")).toHaveTextContent(
      "還有 12 段在準備中，關鍵字搜尋已完整。",
    );
  });
});
