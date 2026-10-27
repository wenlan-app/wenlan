// SPDX-License-Identifier: AGPL-3.0-only
// No network fallback: every value belongs to the browser-only verification library.
import { createSpacesNavigationFixture } from "../../e2e/fixtures/spacesNavigation";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
import type { ImportBatchStatus, ImportPhase, Page } from "../../src/lib/tauri";
import { organizeInvoke, seedOrganizeRun } from "./first-use-organize";
const scenario = new URLSearchParams(location.search).get("state") ?? "empty";
const fixture = createSpacesNavigationFixture();
const organizeBasePage = fixture.pages[0] as unknown as Page | undefined;
seedOrganizeRun();
const runtime = new TauriMockRuntime(scenario === "ready" ? fixture : {
  ...fixture, pages: [], memories: [], entities: [], entityDetails: [], refinements: [],
  distillReview: { ...fixture.distillReview, pending: [], stale_pages: [], orphan_topics: [] },
});
const phases: ImportPhase[] = ["ingest", "store", "detect", "enrich", "link", "distill"];
const batch: ImportBatchStatus = {
  batch_id: "first-use-verification", source: "other", started_at: 1788991200, updated_at: 1788991200,
  chunks_received: 1, memories_imported: 3, memories_skipped: 0, entities_detected: 2, entities_established: 1,
  pages_distilled: 0, complete: scenario === "failed",
  phases: phases.map((phase, index) => ({ phase, state: index < 3 ? "complete" : scenario === "failed" ? "failed" : index === 3 ? "running" : "pending", done: index < 3 ? 3 : 0, total: phase === "distill" ? 0 : 3, failed: scenario === "failed" && index >= 3 ? 3 : 0 })),
};
let submittedBatch: ImportBatchStatus | null = null;
export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (scenario === "error" && (command === "list_pages" || command === "active_import_batches_cmd")) throw new Error("Verification: daemon unavailable");
  const organized = organizeInvoke(command, args, organizeBasePage);
  if (organized) return organized.value as T;
  let value: unknown;
  switch (command) {
    case "active_import_batches_cmd": {
      const active = submittedBatch ?? batch;
      value = { batches: !active.complete && (submittedBatch !== null || ["processing", "failed"].includes(scenario)) ? [active] : [] };
      break;
    }
    case "import_memories_cmd": {
      // Exercise the real import UI without writing a library or retaining text.
      const count = String(args?.content ?? "").split("\n").filter((line) => line.trim()).length;
      submittedBatch = {
        ...batch, batch_id: String(args?.batchId), source: String(args?.source),
        memories_imported: count + (submittedBatch?.memories_imported ?? 0),
        entities_detected: 0, entities_established: 0, complete: false,
        phases: phases.map((phase, index) => ({ phase, state: index < 2 ? "complete" : "pending", done: index < 2 ? count : 0, total: index < 2 ? count : 0, failed: 0 })),
      };
      value = { imported: count, skipped: 0, breakdown: {}, entities_created: 0, observations_added: 0, relations_created: 0, batch_id: submittedBatch.batch_id };
      break;
    }
    case "import_batch_status_cmd": value = submittedBatch ?? batch; break;
    case "get_api_key": value = null; break;
    case "get_external_llm": value = [null, null, null]; break;
    case "get_on_device_model": value = { loaded: null, selected: null, models: [] }; break;
    default: return await runtime.invoke(command, args) as T;
  }
  return value as T;
}
export function convertFileSrc(path: string): string { return `review-fixture://asset/${encodeURIComponent(path)}`; }
