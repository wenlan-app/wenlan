// SPDX-License-Identifier: AGPL-3.0-only
//
// The import handoff.
//
// The import used to show six phases and then a trailing bill of what the
// background still owed. This proves the new contract: the import surface
// shows the two phases that end in something the user can use, says plainly
// that those memories are searchable now, and points to Activity for the
// background work that continues after the import.
//
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

/** The four phases that must never appear on the import surface again. */
const BACKGROUND_PHASE_LABELS = [
  "Detecting entities",
  "Enriching memories",
  "Linking memories",
  "Organizing pages",
] as const;

type ImportChunkArgs = {
  batchId?: string | null;
  chunkIndex?: number | null;
  chunkTotal?: number | null;
  content?: unknown;
};

declare global {
  interface Window {
    __releaseHandoffImport?: boolean;
    __handoffImportCalls?: ImportChunkArgs[];
  }
}

/**
 * One stateful mock for both surfaces, because the point of this spec is that
 * they agree: once memories are stored, Activity reports the work continuing
 * in the background. `get_activity` therefore answers from the same counter
 * the batch status does, rather than from a fixed fixture.
 */
async function installHandoffMock(page: Page): Promise<void> {
  await page.addInitScript(() => {
    let imported = false;
    const calls: ImportChunkArgs[] = [];
    window.__handoffImportCalls = calls;
    const internals = window.__TAURI_INTERNALS__;
    if (!internals) return;
    const orig = internals.invoke.bind(internals);
    const asset = (
      kind: string,
      state: string,
      done: number,
      total: number,
    ) => ({
      kind, state, done, total, blocked: 0,
      // The rail reflects measured running steps, not the aggregate state alone.
      steps: [{ name: kind === "memories" ? "summarize" : kind === "entities" ? "detect" : "write", state, done, total, failed: 0, job: kind === "pages" ? "synthesis" : "everyday" }],
    });

    internals.invoke = (async (command: string, args?: unknown) => {
      const params = (args ?? {}) as ImportChunkArgs;

      if (command === "import_memories_cmd") {
        calls.push(params);
        // Held until the spec has asserted on the progress phase, so the
        // summary cannot race past it.
        await new Promise<void>((resolve) => {
          const check = () => {
            if (window.__releaseHandoffImport) resolve();
            else setTimeout(check, 25);
          };
          check();
        });
        imported = true;
        return {
          imported: 3,
          skipped: 0,
          breakdown: { fact: 3 },
          entities_created: 0,
          observations_added: 0,
          relations_created: 0,
          batch_id: params.batchId ?? "batch-handoff",
        };
      }

      if (command === "import_batch_status_cmd") {
        return {
          batch_id: params.batchId ?? "batch-handoff",
          source: "chatgpt",
          started_at: 1_700_000_000,
          updated_at: 1_700_000_100,
          chunks_received: 1,
          memories_imported: 3,
          memories_skipped: 0,
          entities_detected: 2,
          entities_established: 0,
          pages_distilled: 0,
          phases: [
            { phase: "ingest", state: "complete", done: 3, total: 3, failed: 0 },
            { phase: "store", state: "complete", done: 3, total: 3, failed: 0 },
            { phase: "detect", state: "running", done: 1, total: 3, failed: 0 },
            { phase: "enrich", state: "running", done: 1, total: 3, failed: 0 },
            { phase: "link", state: "pending", done: 0, total: 0, failed: 0 },
            { phase: "distill", state: "running", done: 0, total: 0, failed: 0 },
          ],
          // Never settles: the handoff sentence has to hold while the
          // background is still working, which is exactly when it matters.
          complete: false,
          space: null,
        };
      }

      if (command === "active_import_batches_cmd") return { batches: [] };

      if (command === "get_activity") {
        // Before the import there is nothing to organize; after it, the
        // background work the import handed off is what the line reports.
        return imported
          ? {
              state: "organizing",
              last_activity_at: 1_783_728_000,
              assets: [
                asset("memories", "running", 1, 3),
                asset("entities", "running", 1, 3),
                asset("pages", "idle", 0, 0),
              ],
              everyday: { job: "everyday", lane: "on_device", model: "Qwen3 4B", mode: "auto", available: true },
              synthesis: { job: "synthesis", lane: "on_device", model: "Qwen3 4B", mode: "auto", available: true },
              refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
            }
          : {
              // The fixture begins up to date; the Activity rail renders its
              // existing neutral label for this idle state.
              state: "up_to_date",
              last_activity_at: null,
              assets: [
                asset("memories", "idle", 0, 0),
                asset("entities", "idle", 0, 0),
                asset("pages", "idle", 0, 0),
              ],
              everyday: { job: "everyday", lane: "on_device", model: "Qwen3 4B", mode: "auto", available: true },
              synthesis: { job: "synthesis", lane: "on_device", model: "Qwen3 4B", mode: "auto", available: true },
              refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
            };
      }

      return orig(command, args);
    }) as typeof internals.invoke;
  });
}

test("an import hands background work to Activity after returning to the workspace", async ({ page }) => {
  const errors = collectBrowserErrors(page);

  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
  await installHandoffMock(page);
  await page.goto("/");

  // The idle Activity rail stays neutral before the import begins.
  const statusLine = page.getByTestId("activity-status");
  await expect(statusLine).toHaveAccessibleName("Activity");
  await expect(statusLine).not.toHaveAttribute("data-state");

  // Account menu → Settings → Sources → Import memories.
  await page.getByRole("button", { name: /account menu/i }).click();
  await page.getByRole("menuitem", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Paste or import", exact: true }).click();

  await page
    .getByPlaceholder(/paste your memories/i)
    .fill("Memory one\nMemory two\nMemory three");
  await page.getByRole("button", { name: "Import", exact: true }).click();

  // ── Mid-flight: two phases, and only two ──
  await expect(page.getByText("Receiving memories")).toBeVisible();
  await expect(page.getByText("Storing memories")).toBeVisible();
  for (const label of BACKGROUND_PHASE_LABELS) {
    await expect(page.getByText(label)).toHaveCount(0);
  }
  // The daemon's row counts, not an elapsed-time bar or a generic spinner line.
  await expect(page.getByText("3 of 3").first()).toBeVisible();
  await expect(page.getByText(/Processing your memories/)).not.toBeVisible();

  await page.evaluate(() => {
    window.__releaseHandoffImport = true;
  });

  // ── The handoff sentence, naming the number stored ──
  await expect(page.getByText(/3 memories imported/i)).toBeVisible();
  await expect(page.getByText(/3 memories stored and searchable now/)).toBeVisible();
  // The mock starts On demand, so the handoff names the AI tool, not the background.
  await expect(page.getByText(/Your AI tool steeps them as it uses Wenlan\. See progress on Activity/)).toBeVisible();

  // One chunk, one batch id.
  const calls = await page.evaluate(() => window.__handoffImportCalls ?? []);
  expect(calls).toHaveLength(1);
  expect(typeof calls[0]?.batchId).toBe("string");
  expect(calls[0]?.batchId).not.toHaveLength(0);
  expect(calls[0]?.chunkIndex).toBe(0);
  expect(calls[0]?.chunkTotal).toBe(1);

  // The background phases stay gone on the summary too, along with the
  // trailing figures that used to read as an unpaid bill.
  for (const label of BACKGROUND_PHASE_LABELS) {
    await expect(page.getByText(label)).toHaveCount(0);
  }
  await expect(page.getByText(/detected entities/)).toHaveCount(0);
  await expect(page.getByText(/related (pages|pages)/)).toHaveCount(0);

  // Settings has no Activity rail. Use the shell's actual Back history to
  // return through Sources and General to the primary Wiki workspace first.
  const historyBack = page
    .getByRole("group", { name: "History navigation" })
    .getByRole("button", { name: "Back", exact: true });
  await historyBack.click();
  const settingsHeading = page.getByRole("heading", { level: 1, name: "Sources", exact: true });
  await expect(settingsHeading).toBeVisible();
  await expect(page.getByRole("heading", { name: "Import Memories", exact: true })).toBeVisible();
  await historyBack.click();
  await expect(page.getByRole("heading", { level: 1, name: "General", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "App", exact: true })).toBeVisible();
  await historyBack.click();
  await expect(page.getByRole("navigation", { name: "Primary navigation" })).toBeVisible();

  // The sidebar's Activity status now reflects the background work handed off
  // by this import. Its summary preserves the actual memory count and units.
  await expect(statusLine).toHaveAttribute("data-state", "organizing", { timeout: 15_000 });
  await expect(statusLine).toHaveAccessibleName("Activity, Steeping");
  await expect(page.getByTestId("activity-status-icon")).toHaveAttribute(
    "data-icon-state",
    "organizing",
  );
  await statusLine.click();
  const popover = page.getByRole("dialog", { name: "Background activity" });
  await expect(popover).toBeVisible();
  await expect(popover.getByTestId("activity-asset-memories")).toContainText(
    "1 of 3 summarized and linked",
  );

  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
