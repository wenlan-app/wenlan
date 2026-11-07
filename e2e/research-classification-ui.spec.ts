// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpaceProjectFixture } from "./fixtures/spaceProject";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

test("saved organizing run is visible and resumes its exact scope", async ({ page }, info) => {
  const fixture = createSpaceProjectFixture();
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { fixture, locale: "en", rawActions: [], localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = window.__TAURI_INTERNALS__!, original = bridge.invoke;
    const rules = { space_id: "project-reading", revision: 2, name: "", instructions: "Compare limitations", content_digest: "rules" };
    const run = { id: "run-one", space_id: "project-reading", revision: 1, rules, items: [{ source: "webpage", source_id: "web::one", title: "Paper One", coverage: "complete", content_digest: "digest", decision_revision: 0, next_offset: null, error: null }, { source: "webpage", source_id: "web::two", title: "Paper Two", coverage: "failed", content_digest: "", decision_revision: 0, next_offset: null, error: "Unavailable" }], target_page_id: "page-existing", target_page_version: 1, result_page_version: 1, pending_revision_id: null, state: "partial", created_at: 1, updated_at: 2 };
    bridge.invoke = (command, args) => command === "get_page" && (args as { id?: string } | undefined)?.id === "page-existing" ? Promise.resolve({id:"page-existing",title:"How retrieval changes answers",space:"讀書會"}) : command === "list_research_runs" ? Promise.resolve({ runs: [run] }) : command === "get_research_rules" ? Promise.resolve(rules) : original(command, args);
  });
  await openPrimaryDestination(page, "Spaces", "More");
  await page.getByTestId("space-row-project-reading").getByRole("button", { name: "讀書會", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await expect(page.getByText("Previous run: 1 complete, 1 failed or unavailable, 0 pending.")).toBeVisible();
  await page.getByRole("button", { name: "Continue previous run" }).click();
  await expect(page.getByText("Continue writing “How retrieval changes answers”")).toBeVisible();
  await expect(page.getByText("Paper One", { exact: true })).toBeVisible();
  await expect(page.getByText("Paper Two", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Previous instructions")).toBeDisabled();
  await page.screenshot({ path: info.outputPath("research-resume.png") });
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("source tag merge stays inside selected sources and shows the outcome", async ({ page }, info) => {
  const fixture = createSpaceProjectFixture();
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { fixture, locale: "en", rawActions: [], localStorage: { "wenlan-spaces-view-mode": "rows" } });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = window.__TAURI_INTERNALS__!, original = bridge.invoke;
    const calls: unknown[] = [];
    (window as unknown as { researchMergeCalls: unknown[] }).researchMergeCalls = calls;
    bridge.invoke = (command, args) => {
      if (command === "list_research_runs") return Promise.resolve({runs:[]});
      if (command === "merge_research_tags") { calls.push(args); return Promise.resolve({updated: 1}); }
      return original(command,args);
    };
  });
  await openPrimaryDestination(page, "Spaces", "More");
  await page.getByTestId("space-row-project-reading").getByRole("button", { name: "讀書會", exact: true }).click();
  await page.getByRole("tab", { name: "Sources", exact: true }).click();
  await page.getByRole("button", {name:"Select",exact:true}).click();
  await page.getByRole("checkbox", {name:"Select 讀書會摘錄",exact:true}).check();
  await page.getByRole("button", {name:"Edit tags… (1)"}).click();
  await page.getByLabel("Action", {exact:true}).selectOption("merge");
  await page.getByLabel("Tag", {exact:true}).fill("retrieval");
  await page.getByLabel("Merge into", {exact:true}).fill("rag");
  await page.getByRole("button", {name:"Apply to 1 source"}).click();
  await expect(page.getByText("Tag operation succeeded on 1 source.")).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as {researchMergeCalls: unknown[]}).researchMergeCalls)).toEqual([{request:{space:"讀書會",sources:[{source:"file",source_id:"design.md",title:"讀書會摘錄"}],from_tag:"retrieval",into_tag:"rag"}}]);
  await page.screenshot({path:info.outputPath("research-tag-merge.png")});
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
