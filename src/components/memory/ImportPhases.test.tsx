import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act, within } from "@testing-library/react";
import {
  ImportDetailPanel,
  ImportPhaseList,
  ImportStatusPill,
  summarizeImportBatches,
  useImportBatchStatus,
} from "./ImportPhases";
import type { ImportBatchStatus, ImportPhase } from "../../lib/tauri";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { OrganizeActionsContext } from "./askAi/TurnIntoPages";

vi.mock("../../lib/tauri", () => ({
  getImportBatchStatus: vi.fn(),
}));

import { getImportBatchStatus } from "../../lib/tauri";

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
    memories_imported: 10,
    memories_skipped: 2,
    entities_detected: 4,
    entities_established: 1,
    pages_distilled: 3,
    phases: [
      entry("ingest", "complete", 10, 10),
      entry("store", "complete", 10, 10),
      entry("detect", "running", 5, 12),
      entry("enrich", "pending", 0, 0),
      entry("link", "pending", 0, 0),
      entry("distill", "running", 3, 0),
    ],
    complete: false,
    space: null,
    ...overrides,
  } as ImportBatchStatus;
}

function Probe({ batchId, uploading = false }: { batchId: string | null; uploading?: boolean }) {
  const status = useImportBatchStatus(batchId, uploading);
  return <div data-testid="probe">{status ? status.batch_id : "none"}</div>;
}

describe("summarizeImportBatches", () => {
  it("adds counts across batches and keeps one pill", () => {
    const summary = summarizeImportBatches([
      makeBatch({ memories_imported: 10, pages_distilled: 3 }),
      makeBatch({ batch_id: "batch-2", memories_imported: 5, pages_distilled: 1 }),
    ]);
    expect(summary.memoriesImported).toBe(15);
    expect(summary.pagesDistilled).toBeNull();
    expect(summary.hasRelatedPages).toBe(true);
    expect(summary.complete).toBe(false);
    expect(summary.runningPhases).toContain("detect");
  });

  it("reads failed while any batch reports a failed phase", () => {
    const failed = makeBatch({
      phases: [entry("detect", "failed", 2, 3, 1)],
    });
    const summary = summarizeImportBatches([makeBatch(), failed]);
    expect(summary.failedPhases).toEqual(["detect"]);
    const detect = summary.phases.find((p) => p.phase === "detect")!;
    expect(detect.state).toBe("failed");
    expect(detect.failed).toBe(1);
  });

  it("is complete only when every batch settles every phase", () => {
    const done = makeBatch({
      complete: true,
      phases: [
        entry("ingest", "complete", 10, 10),
        entry("store", "complete", 10, 10),
        entry("detect", "complete", 12, 12),
        entry("enrich", "complete", 10, 10),
        entry("link", "complete", 10, 10),
        entry("distill", "complete", 3, 0),
      ],
    });
    expect(summarizeImportBatches([done]).complete).toBe(true);
    expect(summarizeImportBatches([done, makeBatch()]).complete).toBe(false);
  });
});

describe("ImportPhaseList", () => {
  it("renders only the two phases that end in something the user can use", () => {
    render(<ImportPhaseList phases={makeBatch().phases} />);
    expect(screen.getByText("Receiving memories")).toBeInTheDocument();
    expect(screen.getByText("Storing memories")).toBeInTheDocument();
    expect(screen.getAllByText("10 of 10")).toHaveLength(2);
    // The four background phases report on Activity. In the foreground they
    // read as cost before proof: minutes of grinding with nothing usable in sight.
    expect(screen.queryByText("Detecting entities")).not.toBeInTheDocument();
    expect(screen.queryByText("Enriching memories")).not.toBeInTheDocument();
    expect(screen.queryByText("Linking memories")).not.toBeInTheDocument();
    expect(screen.queryByText("Distilling pages")).not.toBeInTheDocument();
    expect(screen.queryByTestId("import-phase-detect")).not.toBeInTheDocument();
    expect(screen.queryByTestId("import-phase-distill")).not.toBeInTheDocument();
  });

  it("reads a phase with no rows as waiting, never 0%", () => {
    render(<ImportPhaseList phases={[]} />);
    expect(screen.getAllByText("Waiting").length).toBeGreaterThan(0);
    expect(screen.queryByText(/0%/)).not.toBeInTheDocument();
  });

  it("hands the rest off to the background once storing completes", () => {
    render(<ImportPhaseList phases={makeBatch().phases} />);
    const handoff = screen.getByTestId("import-handoff");
    expect(handoff).toHaveTextContent("10 memories stored and searchable now");
    expect(handoff).toHaveTextContent("See further progress on Activity");
  });

  it("says nothing about the handoff while storing is still running", () => {
    render(
      <ImportPhaseList
        phases={[entry("ingest", "complete", 10, 10), entry("store", "running", 4, 10)]}
      />,
    );
    expect(screen.queryByTestId("import-handoff")).not.toBeInTheDocument();
    expect(within(screen.getByTestId("import-phase-store")).getByRole("progressbar"))
      .toHaveAttribute("aria-valuenow", "4");
  });
});

describe("ImportStatusPill", () => {
  it("renders nothing when no batch is active", () => {
    const { container } = render(<ImportStatusPill batches={[]} onOpen={vi.fn()} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("summarises the running phase and opens the panel on click", () => {
    const onOpen = vi.fn();
    render(<ImportStatusPill batches={[makeBatch()]} onOpen={onOpen} />);
    const pill = screen.getByTestId("import-status-pill");
    expect(pill).toHaveTextContent("Import progress");
    expect(pill).toHaveTextContent("Detecting entities");
    fireEvent.click(pill);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("flags a failed batch instead of claiming progress", () => {
    render(
      <ImportStatusPill
        batches={[makeBatch({ phases: [entry("detect", "failed", 2, 3, 1)] })]}
        onOpen={vi.fn()}
      />,
    );
    expect(screen.getByTestId("import-status-pill")).toHaveTextContent("Import needs attention");
  });
});

describe("ImportDetailPanel", () => {
  it("shows per-phase progress with a Back control", () => {
    const onBack = vi.fn();
    render(<ImportDetailPanel batches={[makeBatch()]} onBack={onBack} />);
    expect(screen.getByTestId("import-detail-panel")).toBeInTheDocument();
    expect(screen.getByText("Import progress")).toBeInTheDocument();
    expect(screen.getAllByText("10 of 10")).toHaveLength(2);
    expect(screen.getByTestId("import-handoff")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("import-detail-back"));
    expect(onBack).toHaveBeenCalledTimes(1);
  });
});

describe("useImportBatchStatus", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("polls immediately, keeps polling, and stops on complete", async () => {
    (getImportBatchStatus as ReturnType<typeof vi.fn>).mockResolvedValue(makeBatch());

    render(<Probe batchId="batch-1" />);
    // Immediate read, no interval tick needed.
    expect(getImportBatchStatus).toHaveBeenCalledTimes(1);
    expect(getImportBatchStatus).toHaveBeenCalledWith("batch-1");
    await act(async () => {});
    expect(screen.getByTestId("probe")).toHaveTextContent("batch-1");

    await act(async () => {
      vi.advanceTimersByTime(1_500);
    });
    expect(getImportBatchStatus).toHaveBeenCalledTimes(2);

    // The daemon says complete: the interval is cleared, not left running.
    (getImportBatchStatus as ReturnType<typeof vi.fn>).mockResolvedValue(
      makeBatch({ complete: true }),
    );
    await act(async () => {
      vi.advanceTimersByTime(1_500);
    });
    const settledCalls = (getImportBatchStatus as ReturnType<typeof vi.fn>).mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(getImportBatchStatus).toHaveBeenCalledTimes(settledCalls);
  });

  it("keeps polling through a complete report while chunks are still uploading", async () => {
    // The daemon reports on the memories it has, not the ones still queued in
    // the browser. On an install with no LLM provider every step records
    // `skipped` on arrival, so chunk 1 can report complete before chunk 2 is
    // sent. Stopping there froze the phase list at chunk-1 numbers for the
    // rest of the import, with nothing to restart it.
    (getImportBatchStatus as ReturnType<typeof vi.fn>).mockResolvedValue(
      makeBatch({ complete: true }),
    );

    const { rerender } = render(<Probe batchId="batch-1" uploading />);
    await act(async () => {});
    const afterFirstComplete = (getImportBatchStatus as ReturnType<typeof vi.fn>).mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(4_500);
    });
    expect((getImportBatchStatus as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(
      afterFirstComplete,
    );

    // Upload finished: the next complete report stops the poll for good.
    rerender(<Probe batchId="batch-1" uploading={false} />);
    await act(async () => {
      vi.advanceTimersByTime(1_500);
    });
    const settledCalls = (getImportBatchStatus as ReturnType<typeof vi.fn>).mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(getImportBatchStatus).toHaveBeenCalledTimes(settledCalls);
  });

  it("leaves no interval running after unmount", async () => {
    (getImportBatchStatus as ReturnType<typeof vi.fn>).mockResolvedValue(makeBatch());

    const { unmount } = render(<Probe batchId="batch-1" />);
    await act(async () => {});
    const callsAtUnmount = (getImportBatchStatus as ReturnType<typeof vi.fn>).mock.calls.length;
    unmount();
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(getImportBatchStatus).toHaveBeenCalledTimes(callsAtUnmount);
  });

  it("does nothing without a batch id", async () => {
    render(<Probe batchId={null} />);
    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    expect(getImportBatchStatus).not.toHaveBeenCalled();
    expect(screen.getByTestId("probe")).toHaveTextContent("none");
  });
});


it("does not add overlapping page counts across batches", () => {
  const batches = [makeBatch({ pages_distilled: 2 }), makeBatch({ batch_id: "batch-2", pages_distilled: 2 })];
  const summary = summarizeImportBatches(batches);
  expect(summary.pagesDistilled).toBeNull();
  render(<ImportDetailPanel batches={batches} onBack={vi.fn()} />);
  // The panel never prints a page count, summed or otherwise: page work is no
  // longer part of the import surface at all.
  expect(screen.queryByText("4 related pages")).not.toBeInTheDocument();
  expect(screen.queryByText(/related pages/)).not.toBeInTheDocument();
});

it("finishes the import without pretending that a thin batch produced a page", () => {
  const batch = makeBatch({
    complete: true,
    pages_distilled: 0,
    phases: [
      entry("ingest", "complete", 2, 2),
      entry("store", "complete", 2, 2),
      entry("detect", "complete", 4, 4),
      entry("enrich", "complete", 2, 2),
      entry("link", "complete", 2, 2),
      entry("distill", "pending", 0, 0),
    ],
  });
  const summary = summarizeImportBatches([batch]);
  expect(summary.complete).toBe(true);
  expect(summary.runningPhases).toEqual([]);
  expect(summary.hasRelatedPages).toBe(false);
  expect(summary.phases.find((p) => p.phase === "distill")?.state).toBe("pending");
  render(<ImportPhaseList phases={batch.phases} />);
  // A thin batch produced no page, and the import surface no longer says
  // anything about pages either way. What it does say is what is true: the
  // memories are stored and the rest continues in the background.
  expect(screen.getByTestId("import-handoff")).toHaveTextContent(
    "2 memories stored and searchable now",
  );
  expect(screen.queryByTestId("import-phase-distill")).not.toBeInTheDocument();
});

describe("ImportPhaseList inside the main window", () => {
  it("leads with what was imported and hands it on, in place of the old handoff", () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <OrganizeActionsContext.Provider
          value={{ onConnectTool: vi.fn(), onOpenIntelligence: vi.fn(), onOpenActivity: vi.fn(), onSelectPage: vi.fn() }}
        >
          <ImportPhaseList phases={makeBatch().phases} onDemand />
        </OrganizeActionsContext.Provider>
      </QueryClientProvider>,
    );
    expect(screen.getByTestId("import-lead")).toHaveTextContent("10 notes imported. You can search them now.");
    expect(screen.queryByTestId("import-handoff")).not.toBeInTheDocument();
    expect(screen.getByTestId("import-organize").textContent).not.toMatch(/steep/i);
  });
});
