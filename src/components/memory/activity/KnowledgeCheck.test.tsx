// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RepairMutation, RepairPlanEntry, RepairLintReport, RepairPlanSummary } from "../../../lib/repairTypes";
import type { KnowledgeCheckState } from "../../../lib/knowledgeCheckWorkflow";
import { getPage } from "../../../lib/tauri";
import KnowledgeCheck from "./KnowledgeCheck";

const harness = vi.hoisted(() => {
  const listeners = new Set<() => void>();
  let snapshot: KnowledgeCheckState;
  const controller = {
    subscribe: (listener: () => void) => { listeners.add(listener); return () => listeners.delete(listener); },
    getSnapshot: () => snapshot,
    initialize: vi.fn(async () => {}),
    check: vi.fn(async () => {}),
    repair: vi.fn(async () => {}),
    recover: vi.fn(async () => {}),
  };
  return {
    controller,
    listeners,
    setSnapshot: (value: KnowledgeCheckState) => {
      snapshot = value;
      for (const listener of listeners) listener();
    },
    key: (entry: RepairPlanEntry) => String((entry as RepairPlanEntry & { testKey: string }).testKey),
    automatic: (entry: RepairPlanEntry) => Boolean((entry as RepairPlanEntry & { testAutomatic: boolean }).testAutomatic),
  };
});

vi.mock("../../../lib/knowledgeCheckWorkflow", () => ({
  knowledgeCheckController: harness.controller,
  knowledgeEntryKey: harness.key,
  isAutomaticRepair: harness.automatic,
}));

vi.mock("../../../lib/tauri", () => ({
  getPage: vi.fn(async (id: string) => ({ id, title: id === "target-page" ? "Rain plan" : "Trip pages" })),
  getMemoryDetail: vi.fn(async (id: string) => ({ source_id: id, title: "Packing reminder" })),
}));

function report(complete = true): RepairLintReport {
  return { complete, totals: { checks: 2, passed: 0, findings: 2, actionable_findings: 2, advisory_findings: 0, incomplete: 0 } } as RepairLintReport;
}

function plan(): RepairPlanSummary {
  return { entry_count: 2, deterministic_complete: true, semantic_complete: false } as RepairPlanSummary;
}

function readyEntry(key: string, automatic: boolean, target: "page_link" | "memory" = "page_link"): RepairPlanEntry {
  const manifestTarget = target === "page_link"
    ? { kind: "page_link", source_page_id: "source-page", label_key: "Rain plan", scope: { kind: "global" } }
    : { kind: "memory", source_id: "memory-1", scope: { kind: "global" } };
  const mutation = target === "page_link"
    ? { kind: "bind_page_link", before_target_page_id: null, after_target_page_id: "target-page" }
    : { kind: "normalize_memory_source_agent", before_source_agent: "" };
  return {
    check_id: target === "page_link" ? "pages.links.orphan_labels" : "identity.memory_state_integrity",
    occurrence_digest: `${key}-digest`,
    affected_records: [{ kind: target === "page_link" ? "page_link" : "memory", durable_id: target === "page_link" ? "source-page" : "memory-1" }],
    resolution: { disposition: "ready", manifest: { target: manifestTarget, mutation } as RepairPlanEntry["resolution"] extends { disposition: "ready"; manifest: infer M } ? M : never },
    testKey: key,
    testAutomatic: automatic,
  } as RepairPlanEntry;
}

function memoryRepairEntry(key: string, checkId: string, mutation: RepairMutation): RepairPlanEntry {
  const entry = readyEntry(key, true, "memory");
  entry.check_id = checkId;
  if (entry.resolution.disposition === "ready") entry.resolution.manifest.mutation = mutation;
  return entry;
}

function state(overrides: Partial<KnowledgeCheckState> = {}): KnowledgeCheckState {
  return {
    phase: "ready",
    report: report(),
    plan: plan(),
    entries: [readyEntry("eligible-link", true), readyEntry("manual-memory", false, "memory")],
    results: { "eligible-link": "not_started", "manual-memory": "not_started" },
    error: null,
    recoveryRequired: false,
    completed: 0,
    total: 0,
    ...overrides,
  };
}

async function renderCheck(snapshot = state()) {
  harness.setSnapshot(snapshot);
  const rendered = render(<KnowledgeCheck onBack={() => {}} onOpenPage={() => {}} onNavigateMemory={() => {}} onOpenReview={() => {}} />);
  if (snapshot.entries.length > 0) await screen.findAllByText(/Trip pages|Packing reminder/);
  return rendered;
}

describe("KnowledgeCheck", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    harness.listeners.clear();
    harness.setSnapshot({ phase: "idle", report: null, plan: null, entries: [], results: {}, error: null, recoveryRequired: false, completed: 0, total: 0 });
  });

  it("waits for the user's click before checking and has no AI/deep-check action", async () => {
    await renderCheck({ phase: "idle", report: null, plan: null, entries: [], results: {}, error: null, recoveryRequired: false, completed: 0, total: 0 });
    expect(harness.controller.check).not.toHaveBeenCalled();
    expect(screen.queryByText(/AI|deep/i)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Check knowledge" }));
    expect(harness.controller.check).toHaveBeenCalledTimes(1);
  });

  it("offers an explicit retry after an empty check error", async () => {
    await renderCheck({ phase: "error", report: null, plan: null, entries: [], results: {}, error: "check failed", recoveryRequired: false, completed: 0, total: 0 });
    fireEvent.click(screen.getByRole("button", { name: "Retry check" }));
    expect(harness.controller.check).toHaveBeenCalledTimes(1);
  });

  it("restores saved verified progress and entries without requiring a report", async () => {
    await renderCheck({
      phase: "ready",
      report: null,
      plan: null,
      entries: [readyEntry("eligible-link", true)],
      results: { "eligible-link": "verified" },
      error: null,
      recoveryRequired: false,
      completed: 1,
      total: 1,
    });
    expect(screen.getByText("Saved progress: 0 items need attention.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Verified/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled();
    expect(screen.getByText("All selected repairs were verified.")).toBeInTheDocument();
  });

  it("shows a neutral row with the human affected title, preserves focus on close, and shows the exact page-link change", async () => {
    await renderCheck(state({ entries: [readyEntry("eligible-link", true)] }));
    const row = await screen.findByRole("button", { name: /A page link needs reconnecting.*Trip pages/ });
    fireEvent.click(row);
    const drawer = await screen.findByRole("dialog", { name: "Issue details" });
    expect(within(drawer).getByText("“Rain plan” is not linked")).toBeInTheDocument();
    expect(await within(drawer).findByText("Rain plan")).toBeInTheDocument();
    fireEvent.click(within(drawer).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(row).toHaveFocus());
  });

  it("previews the exact four allowlisted changes in human terms", async () => {
    const entries = [
      readyEntry("page-link", true),
      memoryRepairEntry("source-agent", "identity.memory_state_integrity", { kind: "normalize_memory_source_agent", before_source_agent: "" }),
      memoryRepairEntry("self-reference", "memories.supersession_integrity", { kind: "clear_memory_supersedes", before_supersedes: "memory-1" }),
      memoryRepairEntry("memory-topic-link", "memory_entities.integrity", { kind: "delete_memory_entity_link", memory_id: "memory-1", entity_id: "topic-1" }),
    ];
    await renderCheck(state({ entries, results: Object.fromEntries(entries.map((entry) => [harness.key(entry), "not_started"])) }));
    expect(screen.getByRole("button", { name: "Repair 4 items" })).toBeInTheDocument();
    const rows = within(screen.getByRole("list")).getAllByRole("button");
    const expected = [
      ["“Rain plan” is not linked", "Rain plan"],
      ["Empty source label", "No source label"],
      ["Points to itself", "Self-reference removed"],
      ["Broken memory and topic link", "Broken link removed; memory and topic remain"],
    ];
    for (let index = 0; index < rows.length; index += 1) {
      fireEvent.click(rows[index]);
      const drawer = await screen.findByRole("dialog", { name: "Issue details" });
      expect(within(drawer).getByText(expected[index][0])).toBeInTheDocument();
      expect(await within(drawer).findByText(expected[index][1])).toBeInTheDocument();
      fireEvent.click(within(drawer).getByRole("button", { name: "Close" }));
    }
  });

  it("repairs only eligible entries from the bulk action", async () => {
    await renderCheck();
    expect(screen.queryByText(/Some findings could not be fully prepared/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Repair 1 item" }));
    expect(harness.controller.repair).toHaveBeenCalledWith(["eligible-link"]);
  });

  it("keeps applied-but-unverified work in the remaining count", async () => {
    await renderCheck(state({
      phase: "paused",
      recoveryRequired: true,
      entries: [readyEntry("eligible-link", true)],
      results: { "eligible-link": "applied_unverified" },
      total: 1,
    }));
    expect(screen.getByText("0 of 1 items verified; 1 remain.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Applied, not yet verified/ })).toBeInTheDocument();
  });

  it("does not offer repair actions for incomplete reports and blocks writes during recovery", async () => {
    const incomplete = state({ phase: "error", report: report(false), error: "unstable snapshot" });
    await renderCheck(incomplete);
    expect(screen.getByText(/Only part of the knowledge/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Repair 1 item" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(harness.controller.check).toHaveBeenCalledTimes(1);
  });

  it("does not claim a clean result when an apparently complete report was rejected", async () => {
    const rejected = report(true);
    rejected.totals.findings = 0;
    await renderCheck(state({ phase: "error", report: rejected, plan: null, entries: [], error: "snapshot changed" }));
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(screen.queryByText("No issues were found in the checked knowledge.")).not.toBeInTheDocument();
    expect(screen.queryByText(/items need attention/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled();
  });

  it("allows a fresh check after a safely paused repair with no recovery required", async () => {
    await renderCheck(state({ phase: "paused", recoveryRequired: false, error: "repair was cancelled" }));
    expect(screen.getByText("Run a fresh check before continuing with the remaining items.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Check repair status" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(harness.controller.check).toHaveBeenCalledTimes(1);
    expect(harness.controller.repair).not.toHaveBeenCalled();
  });

  it("requires recovery before another check or repair", async () => {
    await renderCheck(state({ phase: "paused", recoveryRequired: true, error: "pending" }));
    expect(screen.getByText(/A repair may have been applied/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Repair 1 item" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Check again" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Check repair status" }));
    expect(harness.controller.recover).toHaveBeenCalledTimes(1);
    expect(harness.controller.check).not.toHaveBeenCalled();
    expect(harness.controller.repair).not.toHaveBeenCalled();
  });

  it("opens the exact review item id", async () => {
    const reviewEntry = {
      check_id: "pages.duplicate_active_titles",
      occurrence_digest: "review-digest",
      affected_records: [{ kind: "page", durable_id: "source-page" }],
      resolution: { disposition: "review", review_item: { review_id: "lint_review_exact", check_id: "pages.duplicate_active_titles", issue: "duplicate titles", choices: [], suggested_research_queries: [] } },
      testKey: "review-key",
      testAutomatic: false,
    } as unknown as RepairPlanEntry;
    const onOpenReview = vi.fn();
    harness.setSnapshot(state({ entries: [reviewEntry], results: { "review-key": "not_started" } }));
    render(<KnowledgeCheck onBack={() => {}} onOpenReview={onOpenReview} />);
    await screen.findAllByText(/Trip pages/);
    fireEvent.click(await screen.findByRole("button", { name: /Pages share the same title/ }));
    const drawer = screen.getByRole("dialog", { name: "Issue details" });
    await within(drawer).findByText("Trip pages");
    fireEvent.click(within(drawer).getByRole("button", { name: "Review item" }));
    expect(onOpenReview).toHaveBeenCalledWith("refinement:lint_review_exact");
  });

  // CPU-bound: it renders 50 rows twice. Role queries compute accessible names
  // for every element they scan, which made this test take 1.5 s alone and time
  // out at 5.1 s and 5.9 s on a loaded CI runner. Row counts use plain
  // selectors (about 0.13 s now), and the explicit timeout keeps headroom.
  it("pages a 101-entry plan in groups of 50 and only loads visible row titles", async () => {
    const entries = Array.from({ length: 101 }, (_, index) => readyEntry(`entry-${index}`, index % 2 === 0));
    const results = Object.fromEntries(entries.map((entry) => [harness.key(entry), "not_started" as const]));
    await renderCheck(state({ entries, results }));
    const rows = () => screen.getByRole("list").querySelectorAll("button");

    const getPageMock = vi.mocked(getPage);
    await waitFor(() => expect(getPageMock).toHaveBeenCalledTimes(50));
    expect(rows()).toHaveLength(50);
    expect(screen.getByText("Page 1 of 3")).toBeInTheDocument();
    expect(screen.getByText("Repair 51 items").closest("button")).toBeInTheDocument();

    fireEvent.click(rows()[0]);
    const detail = await screen.findByRole("dialog", { name: "Issue details" });
    await within(detail).findByText("Rain plan");
    getPageMock.mockClear();
    fireEvent.click(screen.getByText("Next"));
    await waitFor(() => expect(detail).not.toBeInTheDocument());
    await waitFor(() => expect(getPageMock).toHaveBeenCalledTimes(50));
    expect(rows()).toHaveLength(50);
    expect(screen.getByText("Page 2 of 3")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Next"));
    await waitFor(() => expect(getPageMock).toHaveBeenCalledTimes(51));
    expect(rows()).toHaveLength(1);
    expect(screen.getByText("Page 3 of 3")).toBeInTheDocument();
  }, 20_000);
});
