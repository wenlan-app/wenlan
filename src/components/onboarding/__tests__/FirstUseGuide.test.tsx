// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, within, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { FirstUseGuide } from "../FirstUseGuide";
import type { ImportBatchStatus, ImportPhase, Page } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", () => ({
  getActiveImportBatches: vi.fn(),
  getResolvedRouting: vi.fn(),
  getImportBatchStatus: vi.fn(),
  getMemoryStats: vi.fn(),
  listPages: vi.fn(),
  clipboardWrite: vi.fn(),
}));

import {
  getActiveImportBatches,
  getImportBatchStatus,
  getResolvedRouting,
  getMemoryStats,
  listPages,
} from "../../../lib/tauri";

const mockedBatches = vi.mocked(getActiveImportBatches);
const mockedBatchStatus = vi.mocked(getImportBatchStatus);
const mockedMemoryStats = vi.mocked(getMemoryStats);
const mockedListPages = vi.mocked(listPages);

function entry(
  phase: ImportPhase,
  state: "pending" | "running" | "complete" | "failed",
  done: number,
  total: number,
  failed = 0,
) {
  return { phase, state, done, total, failed };
}

function makeBatch(overrides: Partial<ImportBatchStatus> = {}): ImportBatchStatus {
  return {
    batch_id: "batch-1",
    source: "chatgpt",
    started_at: 1_700_000_000,
    updated_at: 1_700_000_100,
    chunks_received: 1,
    memories_imported: 4,
    memories_skipped: 1,
    entities_detected: 2,
    entities_established: 1,
    pages_distilled: 0,
    phases: [
      entry("ingest", "complete", 4, 4),
      entry("store", "complete", 4, 4),
      entry("detect", "running", 2, 4),
      entry("enrich", "pending", 0, 0),
      entry("link", "pending", 0, 0),
      entry("distill", "pending", 0, 0),
    ],
    complete: false,
    space: null,
    ...overrides,
  } as ImportBatchStatus;
}

function makePage(overrides: Partial<Page> = {}): Page {
  return {
    id: "page-real-1",
    title: "A real knowledge page",
    summary: null,
    content: "body",
    entity_id: null,
    domain: null,
    space: null,
    source_memory_ids: [],
    version: 1,
    status: "active",
    creation_kind: "distilled",
    review_status: null,
    created_at: "2026-01-01",
    last_compiled: "2026-01-02",
    last_modified: "2026-01-02",
    ...overrides,
  } as Page;
}

function renderGuide(props = {}) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <FirstUseGuide
        onBack={vi.fn()}
        onImport={vi.fn()}
        onOpenIntelligence={vi.fn()}
        onOpenPage={vi.fn()}
        {...props}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockedBatchStatus.mockReset();
  vi.mocked(getResolvedRouting).mockResolvedValue({ everyday: { source: "on_device", model: "qwen3-4b", mode: "pinned", pin: "on_device" }, synthesis: { source: "on_device", model: "qwen3-4b", mode: "pinned", pin: "on_device" }, pool: { anthropic: { configured: false, everyday_model: null, synthesis_model: null }, external: null, on_device: { selected: "qwen3-4b", loaded: true } } });
  mockedBatches.mockResolvedValue({ batches: [] });
  mockedListPages.mockResolvedValue([]);
  mockedMemoryStats.mockResolvedValue({
    total: 0,
    new_today: 0,
    confirmed: 0,
    domains: [],
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("FirstUseGuide navigation", () => {
  it("opens the sample walkthrough directly, with no guide or chooser, and stays query-silent", () => {
    renderGuide();
    expect(screen.getByTestId("first-use-sample")).toBeInTheDocument();
    expect(screen.getByTestId("first-use-guide")).toHaveAttribute("data-view", "sample");
    expect(screen.queryByRole("button", { name: "Bring my data" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Try an example" })).not.toBeInTheDocument();
    expect(mockedListPages).not.toHaveBeenCalled();
    expect(mockedBatches).not.toHaveBeenCalled();
    expect(mockedMemoryStats).not.toHaveBeenCalled();
  });

  it("returns home from the sample header back button and from its final button", () => {
    const onBack = vi.fn();
    renderGuide({ onBack });
    fireEvent.click(screen.getByRole("button", { name: "Back to home" }));
    expect(onBack).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Skip to result" }));
    fireEvent.click(screen.getByRole("button", { name: "Commands for later" }));
    expect(screen.queryByRole("button", { name: "Connect my AI" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Bring my data" })).not.toBeInTheDocument();
    const [, finish] = screen.getAllByRole("button", { name: "Back to home" });
    fireEvent.click(finish);
    expect(onBack).toHaveBeenCalledTimes(2);
    expect(mockedListPages).not.toHaveBeenCalled();
    expect(mockedBatches).not.toHaveBeenCalled();
  });

  it("returns home from the live view", async () => {
    const onBack = vi.fn();
    renderGuide({ onBack, initialView: "live" });
    fireEvent.click(screen.getByRole("button", { name: "Back to home" }));
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Nothing here yet")).toBeInTheDocument();
  });
});

describe("FirstUseGuide live view", () => {
  it("shows the empty state with real actions when the library is empty", async () => {
    const onImport = vi.fn();
    const onOpenIntelligence = vi.fn();
    renderGuide({ onImport, onOpenIntelligence, initialView: "live" });
    expect(await screen.findByText("Nothing here yet")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Import memories" }));
    fireEvent.click(screen.getByRole("button", { name: "Set up intelligence" }));
    expect(onImport).toHaveBeenCalledTimes(1);
    expect(onOpenIntelligence).toHaveBeenCalledTimes(1);
  });

  it("reports a page-list failure as failure with retry, not as empty success", async () => {
    mockedListPages.mockRejectedValueOnce(new Error("down"));
    renderGuide({ initialView: "live" });
    expect(
      await screen.findByText("Wenlan couldn't read this right now."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Nothing here yet")).not.toBeInTheDocument();
    mockedListPages.mockResolvedValue([makePage()]);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Your pages")).toBeInTheDocument();
  });

  it("reports an active-batch failure as failure, not as progress", async () => {
    mockedBatches.mockRejectedValueOnce(new Error("down"));
    mockedListPages.mockResolvedValue([makePage()]);
    renderGuide({ initialView: "live" });
    expect(
      await screen.findByText("Wenlan couldn't read this right now."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Import progress")).not.toBeInTheDocument();
  });

  it("shows a completed import returned by its status endpoint after active polling drops it", async () => {
    mockedBatchStatus.mockResolvedValue(makeBatch({
      complete: true,
      phases: [
        entry("ingest", "complete", 4, 4),
        entry("store", "complete", 4, 4),
        entry("detect", "complete", 4, 4),
        entry("enrich", "complete", 4, 4),
        entry("link", "complete", 4, 4),
        entry("distill", "complete", 0, 0),
      ],
    }));
    mockedBatches.mockResolvedValue({ batches: [] });
    renderGuide({ initialView: "live", batchId: "batch-1" });
    expect(await screen.findByText("Import progress")).toBeInTheDocument();
    expect(screen.getByText("Background work finished.")).toBeInTheDocument();
    expect(screen.getByText(/No page was created in this batch/)).toBeInTheDocument();
    expect(mockedBatchStatus).toHaveBeenCalledWith("batch-1");
  });

  it("keeps the targeted batch through completion, then stops its terminal poll", async () => {
    vi.useFakeTimers();
    mockedBatches.mockResolvedValue({ batches: [] });
    mockedBatchStatus
      .mockResolvedValueOnce(makeBatch())
      .mockResolvedValue(makeBatch({
        complete: true,
        phases: [
          entry("ingest", "complete", 4, 4),
          entry("store", "complete", 4, 4),
          entry("detect", "complete", 4, 4),
          entry("enrich", "complete", 4, 4),
          entry("link", "complete", 4, 4),
          entry("distill", "complete", 0, 0),
        ],
      }));
    renderGuide({ initialView: "live", batchId: "batch-1" });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByTestId("import-phase-store")).toHaveAttribute("data-state", "complete");
    expect(screen.queryByTestId("import-phase-detect")).not.toBeInTheDocument();
    expect(screen.queryByText("Background work finished.")).not.toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });
    await act(async () => {});
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(screen.getByText("Background work finished.")).toBeInTheDocument();
    const settledCalls = mockedBatchStatus.mock.calls.length;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(15_000);
    });
    expect(mockedBatchStatus).toHaveBeenCalledTimes(settledCalls);
  });

  it("renders observed running phases without inventing a finished result", async () => {
    mockedBatches.mockResolvedValue({ batches: [makeBatch({
      phases: [
        entry("ingest", "complete", 4, 4),
        entry("store", "running", 2, 4),
        entry("detect", "pending", 0, 0),
        entry("enrich", "pending", 0, 0),
        entry("link", "pending", 0, 0),
        entry("distill", "pending", 0, 0),
      ],
    })] });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Import progress")).toBeInTheDocument();
    expect(screen.getByTestId("import-phase-store")).toHaveAttribute(
      "data-state",
      "running",
    );
    // Nothing is stored yet, so nothing is searchable yet, so the handoff
    // sentence must not claim it is.
    expect(screen.queryByTestId("import-handoff")).not.toBeInTheDocument();
    expect(
      screen.queryByText(/No page was created in this batch/),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("Background work finished.")).not.toBeInTheDocument();
  });

  it("names every source when summarizing concurrent import batches", async () => {
    mockedBatches.mockResolvedValue({ batches: [
      makeBatch(), makeBatch({ batch_id: "batch-2", source: "claude" }),
    ] });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText(/8 memories imported from ChatGPT · Claude/)).toBeVisible();
  });

  it("shows failed status on a settled batch with failures and zero pages", async () => {
    const onOpenIntelligence = vi.fn();
    mockedBatches.mockResolvedValue({ batches: [] });
    mockedBatchStatus.mockResolvedValue(makeBatch({
      complete: true,
      phases: [
        entry("ingest", "complete", 4, 4),
        entry("store", "complete", 4, 4),
        entry("detect", "failed", 2, 4, 2),
        entry("enrich", "complete", 4, 4),
        entry("link", "complete", 4, 4),
        entry("distill", "complete", 0, 0),
      ],
    }));
    renderGuide({ onOpenIntelligence, initialView: "live", batchId: "batch-1" });
    expect(await screen.findByText("Import progress")).toBeInTheDocument();
    expect(
      await screen.findByText(/Some items could not be processed/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/No page was created in this batch/),
    ).toBeInTheDocument();
    // A background phase failed, and the import surface has no row for it any
    // more. The failure still reaches the user, through the note above.
    expect(screen.queryByTestId("import-phase-detect")).not.toBeInTheDocument();
    expect(screen.getByTestId("import-phase-store")).toHaveAttribute(
      "data-state",
      "complete",
    );
    fireEvent.click(screen.getByRole("button", { name: "Set up intelligence" }));
    expect(onOpenIntelligence).toHaveBeenCalledOnce();
  });

  it("does not double-count a targeted batch that is also in the active list", async () => {
    const batch = makeBatch();
    mockedBatches.mockResolvedValue({ batches: [batch] });
    mockedBatchStatus.mockResolvedValue(batch);
    renderGuide({ initialView: "live", batchId: "batch-1" });
    expect(await screen.findByText("Import progress")).toBeInTheDocument();
    expect(screen.getByText(/4 memories imported from ChatGPT/)).toBeInTheDocument();
    expect(screen.queryByText(/8 memories imported from ChatGPT/)).not.toBeInTheDocument();
  });

  it("offers real knowledge pages and hides entity shadow pages", async () => {
    const onOpenPage = vi.fn();
    mockedListPages.mockResolvedValue([
      makePage({
        id: "shadow-1",
        title: "Shadow entity page",
        creation_kind: "entity",
      }),
      makePage({ id: "page-real-7", title: "v2 launch plan" }),
    ]);
    renderGuide({ onOpenPage, initialView: "live" });
    expect(await screen.findByText("Your pages")).toBeInTheDocument();
    expect(screen.queryByText("Shadow entity page")).not.toBeInTheDocument();
    const live = screen.getByTestId("first-use-live");
    fireEvent.click(within(live).getByText("v2 launch plan"));
    expect(onOpenPage).toHaveBeenCalledTimes(1);
    expect(onOpenPage).toHaveBeenCalledWith("page-real-7");
  });

  it("enters the live view directly when initialView is live", async () => {
    renderGuide({ initialView: "live" });
    expect(screen.getByTestId("first-use-live")).toBeInTheDocument();
    expect(await screen.findByText("Nothing here yet")).toBeInTheDocument();
  });

  it("uses the active endpoint for a direct live view without a targeted batch", async () => {
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Nothing here yet")).toBeInTheDocument();
    expect(mockedBatchStatus).not.toHaveBeenCalled();
    expect(mockedBatches).toHaveBeenCalled();
  });

  it("keeps polling pages and batches while the live view is mounted", async () => {
    vi.useFakeTimers();
    renderGuide({ initialView: "live" });
    await act(async () => {});
    expect(mockedListPages.mock.calls.length).toBeGreaterThanOrEqual(1);
    expect(mockedBatches.mock.calls.length).toBeGreaterThanOrEqual(1);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });
    // Pages re-read on their own cadence so a newly distilled page appears
    // without a manual refresh; batches keep their faster poll.
    expect(mockedListPages.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(mockedBatches.mock.calls.length).toBeGreaterThanOrEqual(2);
  });
});


describe("FirstUseGuide live view saved memories", () => {
  it("shows saved memories instead of the empty state when memories exist without batches or pages", async () => {
    mockedMemoryStats.mockResolvedValue({
      total: 12,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Your memories are saved")).toBeInTheDocument();
    expect(
      screen.getByText(
        "12 memories are saved in your library. No pages have been written yet.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Pages show up here once Wenlan writes them."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Nothing here yet")).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Import memories" }),
    ).not.toBeInTheDocument();
  });

  it("uses singular copy for one saved memory", async () => {
    mockedMemoryStats.mockResolvedValue({
      total: 1,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    renderGuide({ initialView: "live" });
    expect(
      await screen.findByText(
        "1 memory is saved in your library. No pages have been written yet.",
      ),
    ).toBeInTheDocument();
  });

  it("points saved memories to intelligence setup when no model route exists", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue({
      everyday: { source: "basic", model: null, mode: "unconfigured", pin: null },
      synthesis: { source: "none", model: null, mode: "unconfigured", pin: null },
      pool: {
        anthropic: { configured: false, everyday_model: null, synthesis_model: null },
        external: null,
        on_device: null,
      },
    });
    mockedMemoryStats.mockResolvedValue({
      total: 4,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    const onOpenIntelligence = vi.fn();
    renderGuide({ initialView: "live", onOpenIntelligence });
    expect(await screen.findByText("Your memories are saved")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Your data is saved. Set up an available AI model to continue organizing it into pages.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("Pages show up here once Wenlan writes them."),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Set up intelligence" }));
    expect(onOpenIntelligence).toHaveBeenCalledOnce();
  });

  it("shows the loading state, not the empty card, while the memory count is pending", async () => {
    vi.useFakeTimers();
    mockedMemoryStats.mockImplementation(() => new Promise(() => {}));
    renderGuide({ initialView: "live" });
    // Let the page and batch queries settle first; the count query never
    // does, so the view must still read as pending afterward.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(mockedListPages).toHaveBeenCalled();
    expect(screen.getByText("Reading your library…")).toBeInTheDocument();
    expect(screen.queryByText("Nothing here yet")).not.toBeInTheDocument();
  });

  it("treats a memory-count failure as a failure, not an empty library, and recovers on retry", async () => {
    mockedMemoryStats.mockRejectedValueOnce(new Error("down"));
    renderGuide({ initialView: "live" });
    expect(
      await screen.findByText("Wenlan couldn't read this right now."),
    ).toBeInTheDocument();
    expect(screen.queryByText("Nothing here yet")).not.toBeInTheDocument();
    mockedMemoryStats.mockResolvedValue({
      total: 3,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByText(
        "3 memories are saved in your library. No pages have been written yet.",
      ),
    ).toBeInTheDocument();
  });

  it("shows the saved state until a page appears, then switches to the pages list", async () => {
    mockedMemoryStats.mockResolvedValue({
      total: 7,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Your memories are saved")).toBeInTheDocument();
    mockedListPages.mockResolvedValue([makePage()]);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("Your pages")).toBeInTheDocument();
    expect(screen.queryByText("Your memories are saved")).not.toBeInTheDocument();
  });

  it("does not read memory stats when the page list already decides the view", async () => {
    mockedListPages.mockResolvedValue([makePage()]);
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Your pages")).toBeInTheDocument();
    expect(mockedMemoryStats).not.toHaveBeenCalled();
  });

  it("keeps the saved card hidden when pages exist even if memories are saved", async () => {
    mockedMemoryStats.mockResolvedValue({
      total: 5,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    mockedListPages.mockResolvedValue([makePage()]);
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Your pages")).toBeInTheDocument();
    expect(screen.queryByTestId("first-use-live-saved")).not.toBeInTheDocument();
    expect(screen.queryByText("Your memories are saved")).not.toBeInTheDocument();
  });

  it("does not read memory stats when an active batch already decides the view", async () => {
    mockedBatches.mockResolvedValue({ batches: [makeBatch()] });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Import progress")).toBeInTheDocument();
    expect(mockedMemoryStats).not.toHaveBeenCalled();
  });

  it("keeps the neutral saved page when routing is unavailable instead of claiming organization", async () => {
    // A null/failed routing response leaves `routing` null (the same null
    // guard covers the still-pending case). The saved card must not invent
    // an intelligence-setup claim from unknown state, and a routing failure
    // alone must not fail the whole library view.
    vi.mocked(getResolvedRouting).mockRejectedValue(new Error("down"));
    mockedMemoryStats.mockResolvedValue({
      total: 4,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Your memories are saved")).toBeInTheDocument();
    expect(
      screen.getByText(
        "Pages show up here once Wenlan writes them.",
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(
        "Your data is saved. Set up an available AI model to continue organizing it into pages.",
      ),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Set up intelligence" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("Wenlan couldn't read this right now."),
    ).not.toBeInTheDocument();
  });

  it("hides the cached saved card when an import batch appears", async () => {
    // The positive memory count stays cached after it loads; a newly visible
    // batch must still win the view so stale data cannot overlay live work.
    // (The pages-appearing transition is already covered by the saved-until-
    // page-appears test above.)
    mockedMemoryStats.mockResolvedValue({
      total: 6,
      new_today: 0,
      confirmed: 0,
      domains: [],
    });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Your memories are saved")).toBeInTheDocument();
    mockedBatches.mockResolvedValue({ batches: [makeBatch()] });
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("Import progress")).toBeInTheDocument();
    expect(screen.queryByTestId("first-use-live-saved")).not.toBeInTheDocument();
    expect(screen.queryByText("Your memories are saved")).not.toBeInTheDocument();
  });
});

describe("live model availability", () => {
  it("shows model-status pending without hiding real import progress", async () => {
    vi.mocked(getResolvedRouting).mockReturnValue(new Promise(() => {}));
    mockedBatches.mockResolvedValue({ batches: [makeBatch()] });
    renderGuide({ initialView: "live" });
    expect(await screen.findByText("Checking whether an AI model is available to organize your pages.")).toBeVisible();
    expect(screen.getByTestId("import-phase-store")).toHaveAttribute("data-state", "complete");
    expect(screen.queryByTestId("import-phase-detect")).not.toBeInTheDocument();
    expect(screen.queryByText("Nothing here yet")).not.toBeInTheDocument();
  });

  it("reports a failed model-status read and recovers through Retry", async () => {
    const healthyRouting = await getResolvedRouting();
    vi.mocked(getResolvedRouting).mockRejectedValue(new Error("routing unavailable"));
    mockedBatches.mockResolvedValue({ batches: [makeBatch()] });
    renderGuide({ initialView: "live" });
    expect(await screen.findByRole("alert")).toHaveTextContent("Wenlan couldn't check the AI model.");
    expect(screen.getByTestId("import-phase-store")).toHaveAttribute("data-state", "complete");
    expect(screen.queryByTestId("import-phase-detect")).not.toBeInTheDocument();
    vi.mocked(getResolvedRouting).mockResolvedValue(healthyRouting);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Your memories are saved and ready to use. Wenlan will show organization progress as it works.")).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows a setup action instead of promising progress when imported data has no AI route", async () => {
    vi.mocked(getResolvedRouting).mockResolvedValue({
      everyday: { source: "basic", model: null, mode: "unconfigured", pin: null },
      synthesis: { source: "none", model: null, mode: "unconfigured", pin: null },
      pool: { anthropic: { configured: false, everyday_model: null, synthesis_model: null }, external: null, on_device: null },
    });
    mockedBatches.mockResolvedValue({ batches: [makeBatch()] });
    const onOpenIntelligence = vi.fn();
    renderGuide({ initialView: "live", onOpenIntelligence });
    expect(await screen.findByText("Your data is saved. Set up an available AI model to continue organizing it into pages.")).toBeInTheDocument();
    expect(screen.queryByText(/Your data is queued for background work/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Set up intelligence" }));
    expect(onOpenIntelligence).toHaveBeenCalledOnce();
  });
});
