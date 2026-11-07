// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { resolve } from "node:path";
test("existing note notice and review card explain the selection change", async ({page}) => {
  const errors:string[]=[]; page.on("pageerror",error=>errors.push(error.message));
  await page.addInitScript(() => { Object.assign(window,{__TAURI_INTERNALS__:{invoke:async(command:string) => {
    if(command === "list_pending_revisions") return [{target_source_id:"selection-page",revision_source_id:"pending",revision_content:"Proposed research",source_agent:"fixture",last_modified:1,target_kind:"page",review_reason:"source_selection_changed"}];
    if(command === "list_refinements") return {proposals:[]};
    if(command === "get_page" || command === "get_page_explicit_browse") return {id:"selection-page",title:"Research",content:"Saved research",space:"Research"};
    if(command === "get_page_sources") return [];
    if(command === "get_resolved_routing") return null;
    if(command === "get_activity") return {state:"up_to_date",assets:[],everyday:{available:true},synthesis:{available:true}};
    throw new Error(`Unexpected command ${command}`);
  }}}); });
  await page.goto("/preview/research-review.html?mode=selection-notice");
  await expect(page.getByText("The Zotero collection or selection changed. Your saved work is retained; review the updated research.")).toBeVisible();
  await expect(page.getByRole("button",{name:"Review changes"})).toBeVisible();
  if(process.env.RESEARCH_REVIEW_EVIDENCE_DIR) await page.screenshot({path:resolve(process.env.RESEARCH_REVIEW_EVIDENCE_DIR,"selection-notice.png")});
  await page.goto("/preview/research-review.html?mode=selection-review");
  await expect(page.getByText("The source selection changed. Check this update before applying it.")).toBeVisible();
  if(process.env.RESEARCH_REVIEW_EVIDENCE_DIR) await page.screenshot({path:resolve(process.env.RESEARCH_REVIEW_EVIDENCE_DIR,"selection-review.png")});
  expect(errors).toEqual([]);
});
