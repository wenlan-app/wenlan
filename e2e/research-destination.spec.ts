// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpaceProjectFixture } from "./fixtures/spaceProject";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

test("selected sources continue an existing research page with visible scope changes", async ({ page }, info) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { fixture: createSpaceProjectFixture(), locale: "en", rawActions: [], localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = window.__TAURI_INTERNALS__!, original = bridge.invoke;
    const rules = { space_id: "project-reading", revision: 2, name: "", instructions: "Compare limitations", content_digest: "rules" };
    const oldRun = { id: "old-run", space_id: "project-reading", revision: 1, rules, items: [{ source: "webpage", source_id: "old-paper", title: "Previous paper", coverage: "complete", content_digest: "digest", decision_revision: 0, next_offset: null, error: null }], target_page_id: "page-existing", target_page_version: 1, result_page_version: 1, pending_revision_id: null, state: "completed", created_at: 1, updated_at: 2 };
    const state = { requests: [] as unknown[], clipboard: "" };
    (window as unknown as { destinationEvidence: typeof state }).destinationEvidence = state;
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text: string) => { state.clipboard = text; } } });
    bridge.invoke = (command, args) => {
      const params = args as { prompt?: string; domain?: string } | undefined;
      if (command === "list_research_runs") return Promise.resolve({ runs: [oldRun] });
      if (command === "get_research_rules") return Promise.resolve(rules);
      if (command === "open_tool_with_prompt") { state.clipboard = String(params?.prompt ?? ""); return Promise.resolve(); }
      if (command === "list_pages" && params?.domain === "讀書會") return Promise.resolve([{ id: "page-existing", title: "Existing research", space: "讀書會" }]);
      if (command === "create_research_run") {
        state.requests.push(args);
        return Promise.resolve({ ...oldRun, id: "new-run", revision: 1, state: "pending" });
      }
      return original(command, args);
    };
  });
  await openPrimaryDestination(page, "Spaces", "More");
  await page.getByTestId("space-row-project-reading").getByRole("button", { name: "讀書會", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", { name: "Select", exact: true }).click();
  await page.getByRole("checkbox", { name: "Select 讀書會摘錄", exact: true }).check();
  await page.getByRole("button", { name: "Summarize & suggest classifications… (1)" }).click();
  await expect(page.getByLabel("Write to")).toHaveValue("page-existing");
  await page.getByText("Compared with the last run: 1 added, 1 no longer selected").click();
  await expect(page.getByText("Previous paper", { exact: true })).toBeVisible();
  await expect(page.getByText("This changes the sources for this run. It does not remove sources or existing writing.")).toBeVisible();
  await page.screenshot({ path: info.outputPath("destination.png") });
  await page.getByRole("button", { name: "Open Claude Code with prompt" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { destinationEvidence: { clipboard: string } }).destinationEvidence.clipboard)).toContain('existing target page "page-existing"');
  const evidence = await page.evaluate(() => (window as unknown as { destinationEvidence: { requests: { request: { target_page_id: string; sources: { source_id: string }[] } }[] } }).destinationEvidence);
  expect(evidence.requests[0].request.target_page_id).toBe("page-existing");
  expect(evidence.requests[0].request.sources.map(source => source.source_id)).toEqual(["design.md"]);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
