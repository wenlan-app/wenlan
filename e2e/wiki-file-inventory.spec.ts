// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { openDocsFolder, sectionList, showWikiFolders } from "./helpers/wikiWorkspace";
import { expectEditorSource } from "./helpers/markdownEditor";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
const labels = {
 en:{all:"All pages",back:"Back",newFolder:"New folder",folderName:"Folder name",create:"Create",new:"New",newPage:"New page",empty:"This folder has no pages yet.",folders:"Folders"},
};
for (const locale of ["en"] as const) for (const width of [1280,375]) {
 test(`real directories and reading workspace/history agree ${locale} ${width}`,async({page})=>{
  const copy=labels[locale]; const errors=collectBrowserErrors(page);
  await page.setViewportSize({width,height:800});
  const runtime=await installTauriMock(page,{locale,rawActions:[],fixture:createReviewDecisionFixture("wiki-folders")});
  await page.goto("/");const list=await showWikiFolders(page);await expect(page.locator(".wiki-workspace")).toBeVisible();
  // Folders are cards on the Docs list; a card opens that folder's own pages.
  await list.getByRole("button",{name:"Work",exact:true}).click();
  await expect(list.getByRole("heading",{level:1,name:"Work",exact:true})).toBeVisible();
  await expect(list.getByRole("button",{name:"Research",exact:true})).toBeVisible();
  await expect(list.getByRole("button",{name:/Fixture architecture$/})).toBeVisible();
  await expect(list.getByRole("button",{name:/Why fixtures stay deterministic$/})).toHaveCount(0);
  await expect(list.getByRole("table")).toHaveCount(0);
  await expect(list.locator(".wiki-page-link, .asset-card")).toHaveCount(0);
  await expect(page.getByTestId("asset-lens-cards")).toHaveCount(0);
  await list.getByRole("button",{name:/Fixture architecture$/}).click();
  const editor=page.locator(".cm-content[contenteditable=true]");
  await expect(editor).toContainText("Fixture architecture");
  // The open page lights its section on the left; the left side lists no pages.
  await expect(page.locator(".notes-rail-nav .notes-inventory-page")).toHaveCount(0);
  await page.locator(".workspace-history-navigation").getByRole("button",{name:copy.back,exact:true}).click();
  // Back returns to the folder the page was opened from.
  await expect(list.getByRole("heading",{level:1,name:"Work",exact:true})).toBeVisible();
  await expect(list.getByRole("button",{name:/Fixture architecture$/})).toBeVisible();
  await list.locator(".section-list-back").click();
  await list.getByRole("button",{name:"Empty",exact:true}).click();
  await expect(list.getByText(copy.empty,{exact:true})).toBeVisible();
  await list.getByRole("button",{name:copy.new,exact:true}).click();
  await page.getByRole("menuitem",{name:copy.newFolder,exact:true}).click();
  await expect(list.getByRole("textbox",{name:copy.folderName,exact:true})).toBeFocused();
  await list.getByRole("textbox",{name:copy.folderName,exact:true}).fill("New child");
  await list.getByRole("button",{name:copy.create,exact:true}).click();
  await expect(list.getByRole("button",{name:"New child",exact:true})).toBeVisible();
  expect(runtime.calls().filter(call=>call.command==="knowledge_folder_create")).toMatchObject([{args:{parentPath:"Empty",name:"New child"}}]);
  expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
  expect(await page.locator("body").evaluate(el=>el.scrollWidth<=window.innerWidth)).toBe(true);
 });
}
test("an existing folder-and-Space draft survives reload, then auto-finalizes and moves safely",async({page})=>{
 const errors=collectBrowserErrors(page);
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[],fixture:createReviewDecisionFixture("wiki-folders")});
 await page.goto("/");await showWikiFolders(page);await expect(page.locator(".wiki-workspace")).toBeVisible();
 const sidebar=sectionList(page,"docs");
 // Seed a previously saved draft with both scopes. The current New-note form
 // inherits Space only from Space-origin creation and no longer edits Space.
 const draftId="page_10000000-0000-4000-8000-000000000001";
 const seeded=await page.evaluate(async id=>{
  const internals=window.__TAURI_INTERNALS__;
  if(!internals)throw new Error("Expected the installed Tauri mock before seeding the saved draft");
  return internals.invoke("create_page_draft",{clientDraftId:id,folderPath:"Work",title:"Folder draft",content:"Initial saved draft content.",space:"Research"});
 },draftId);
 expect(seeded).toMatchObject({id:draftId,status:"draft",folder_path:"Work",space:"Research"});
 await page.reload();await openDocsFolder(page,"Work");
 await sidebar.getByRole("button",{name:"Open Folder draft",exact:true}).click();
 await expect(page.getByRole("textbox",{name:"Title",exact:true})).toHaveValue("Folder draft");
 await expectEditorSource(page.getByRole("textbox",{name:"Content",exact:true}),"Initial saved draft content.");
 await page.getByRole("textbox",{name:"Title",exact:true}).fill("Folder draft after reload");
 await page.getByRole("textbox",{name:"Content",exact:true}).fill("Updated content survives reload and move.");
 await expect.poll(()=>runtime.calls().filter(call=>call.command==="update_page_draft").length).toBe(1);
 await expect.poll(async()=>page.evaluate(async id=>window.__TAURI_INTERNALS__!.invoke("get_page",{id}),draftId)).toMatchObject({id:draftId,status:"draft",version:2,title:"Folder draft after reload",content:"Updated content survives reload and move.",folder_path:"Work",space:"Research"});
 await page.reload();await openDocsFolder(page,"Work");
 await sidebar.getByRole("button",{name:"Open Folder draft after reload",exact:true}).click();
 await expect(page.getByRole("textbox",{name:"Title",exact:true})).toHaveValue("Folder draft after reload");
 await expectEditorSource(page.getByRole("textbox",{name:"Content",exact:true}),"Updated content survives reload and move.");
 await openDocsFolder(page,"Work");
 await sidebar.getByRole("button",{name:"Open Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("tab",{name:"Folder draft after reload",exact:true}).click();
 await expect(page.getByRole("heading",{level:1,name:"Folder draft after reload",exact:true})).toBeVisible();
 const published=runtime.calls().find(call=>call.command==="publish_page_draft");
 expect(published?.args).toEqual({id:draftId,expectedVersion:2});
 await expect(page.getByRole("button",{name:"Page actions",exact:true})).toBeVisible();
 await page.getByRole("button",{name:"Page actions",exact:true}).click();
 await page.getByRole("menuitem",{name:"Move to folder",exact:true}).click();
 const dialog=page.getByRole("dialog",{name:"Move to folder",exact:true});
 await dialog.getByRole("combobox",{name:"Destination folder",exact:true}).selectOption("Reading");
 runtime.failNext("page_move","destination collision");
 await dialog.getByRole("button",{name:"Move",exact:true}).click();
 await expect(dialog.getByRole("alert")).toBeVisible();
 await dialog.getByRole("button",{name:"Move",exact:true}).click();
 await expect(dialog).toBeHidden();
 await expect(page.locator(".cm-content[contenteditable=true]")).toContainText("Updated content survives reload and move.");
 await expect(page.locator(".page-detail")).toBeVisible();
 const moves=runtime.calls().filter(call=>call.command==="page_move");
 expect(moves).toHaveLength(2);
 expect(moves[0].args).toMatchObject({id:draftId,folderPath:"Reading"});
 expect(moves[1].args).toEqual(moves[0].args);
 const saved=await page.evaluate(async id=>window.__TAURI_INTERNALS__!.invoke("get_page",{id}),draftId);
 expect(saved).toMatchObject({id:draftId,status:"active",space:"Research",content:"Updated content survives reload and move.",storage_path:`Reading/${draftId}.md`});
 // The moved page is listed in its new folder and no longer in the old one.
 await openDocsFolder(page,"Reading");
 await expect(sidebar.getByRole("button",{name:"Open Folder draft after reload",exact:true})).toBeVisible();
 await openDocsFolder(page,"Work");
 await expect(sidebar.getByRole("button",{name:"Open Fixture architecture",exact:true})).toBeVisible();
 await expect(sidebar.getByRole("button",{name:"Open Folder draft after reload",exact:true})).toHaveCount(0);
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

test("old daemon keeps every page available and disables unsupported folder operations", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { locale: "en", rawActions: [], fixture: createReviewDecisionFixture("wiki-folders"), failures: [{command:"knowledge_folders_list",message:"Unknown command",times:100}] });
  await page.goto("/");await showWikiFolders(page);await expect(page.locator(".wiki-workspace")).toBeVisible();
  const sidebar = sectionList(page, "docs");
  await expect(sidebar.getByText("Folder browsing is unavailable. All pages are still accessible.")).toBeVisible();
  await sidebar.getByRole("button",{name:"New",exact:true}).click();
  await expect(page.getByRole("menuitem",{name:/^New folder/})).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(sidebar.getByRole("button",{name:"Work",exact:true})).toHaveCount(0);
  await expect(sidebar.getByRole("button",{name:/Open Independent research/})).toBeVisible();
  await sidebar.getByRole("button",{name:/Open Fixture architecture/}).click();
  await page.getByRole("button",{name:"Page actions",exact:true}).click();
  await page.getByRole("menuitem",{name:"Move to folder",exact:true}).click();
  const dialog = page.getByRole("dialog",{name:"Move to folder",exact:true});
  await expect(dialog.getByRole("status")).toHaveText("Folder browsing is unavailable. All pages are still accessible.");
  await expect(dialog.getByRole("button",{name:"Move",exact:true})).toBeDisabled();
  await dialog.getByRole("button",{name:"Cancel",exact:true}).click();
  await expect(page.locator(".cm-content[contenteditable=true]")).toContainText("Fixture architecture");
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});

test("actual file-save failure survives Main navigation and safely replays the same publish", async ({ page }) => {
  const runtime = await installTauriMock(page, { locale: "en", rawActions: [], fixture: createReviewDecisionFixture("wiki-folders"), pageScenario:{projectionPendingOnce:true} });
  await page.goto("/");await showWikiFolders(page);await expect(page.locator(".wiki-workspace")).toBeVisible();
  const sidebar=await openDocsFolder(page,"Work");
  await sidebar.getByRole("button",{name:"Open Fixture architecture",exact:true}).click();
  await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
  await page.locator(".note-tab-create").click();
  await page.getByRole("textbox",{name:"Title",exact:true}).fill("Saved even when folder is unavailable");
  await page.getByRole("textbox",{name:"Content",exact:true}).fill("This page is safely persisted before its file can be written.");
  await expect.poll(()=>runtime.calls().filter(call=>call.command==="create_page_draft").length).toBe(1);
  const created=runtime.calls().find(call=>call.command==="create_page_draft");
  expect(created?.args).toMatchObject({folderPath:"Work",title:"Saved even when folder is unavailable",content:"This page is safely persisted before its file can be written."});
  const draftId=(created?.args as {clientDraftId:string}).clientDraftId;
  await page.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
  await page.getByRole("tab",{name:"Saved even when folder is unavailable",exact:true}).click();
  await expect(page.getByText("Your page is saved, but its file could not be saved in the chosen folder.",{exact:true})).toBeVisible();
  const initial = runtime.calls().find(call=>call.command==="publish_page_draft");
  expect(initial?.args).toMatchObject({id:draftId,expectedVersion:1});
  const args = initial?.args as { id:string;expectedVersion:number };
  const saved = await page.evaluate(async id => {
  const internals = window.__TAURI_INTERNALS__;
  if (!internals) throw new Error("Expected the installed Tauri mock before reading the saved Page");
  return internals.invoke("get_page",{id});
 },args.id);
  expect(saved).toMatchObject({id:args.id,status:"active",storage_path:null,content:"This page is safely persisted before its file can be written."});
  expect(saved).not.toHaveProperty("projection_error");
  await page.getByRole("button",{name:"Retry saving the file",exact:true}).click();
  await expect(page.getByText("Your page is saved, but its file could not be saved in the chosen folder.",{exact:true})).toHaveCount(0);
  const publishes = runtime.calls().filter(call=>call.command==="publish_page_draft");
  expect(publishes).toHaveLength(2); expect(publishes[1].args).toEqual(initial?.args);
  expect(await page.evaluate(async id => {
  const internals = window.__TAURI_INTERNALS__;
  if (!internals) throw new Error("Expected the installed Tauri mock before reading the saved Page");
  return internals.invoke("get_page",{id});
 },args.id)).toMatchObject({id:args.id,version:args.expectedVersion+1,storage_path:`Work/${args.id}.md`});
});

test("a folder card opens its own pages and an empty folder says so", async ({ page }) => {
  await page.setViewportSize({width:1280,height:900});
  await installTauriMock(page,{locale:"en",rawActions:[],fixture:createReviewDecisionFixture("wiki-folders")});
  await page.goto("/");const directory=await showWikiFolders(page);
  const work=directory.getByRole("button",{name:"Work",exact:true});await work.focus();await work.press("Enter");
  await expect(directory.getByRole("button",{name:/Fixture architecture$/})).toBeVisible();
  await directory.getByRole("button",{name:"Research",exact:true}).click();
  await expect(directory.getByRole("button",{name:/Why fixtures stay deterministic$/})).toBeVisible();
  // Nothing in the list expands or collapses (UX-FIND-4).
  await expect(directory.locator("[aria-expanded]:not([aria-haspopup])")).toHaveCount(0);
  await directory.getByRole("button",{name:/Why fixtures stay deterministic$/}).click();
  await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
  await expect(page.locator(".cm-content")).toContainText("Decision: keep review fixtures deterministic.");
  await openDocsFolder(page,"Empty");
  await expect(directory.getByText("This folder has no pages yet.",{exact:true})).toBeVisible();
});
