// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { openWikiNote } from "./helpers/wikiWorkspace";

for (const width of [1280,375]) test(`recent pages replace default hierarchy and open safely ${width}`,async({page})=>{
 await page.setViewportSize({width,height:900});
 const errors=collectBrowserErrors(page);
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[],fixture:createReviewDecisionFixture("wiki-folders")});
 await page.goto("/");
 const directory=page.locator(".wiki-workspace-directory");
 if(width===375)await page.locator(".wiki-workspace-folders-toggle").click();
 await expect(directory.getByRole("combobox")).toHaveValue("recent");
 await expect(directory).toContainText("Pages you open appear here");
 await expect(directory.locator(".notes-inventory-root")).toHaveCount(0);
 await openWikiNote(page,"Fixture architecture");
 await expect(page.locator(".page-detail")).toBeVisible();
 await openWikiNote(page,"Why fixtures stay deterministic");
 await expect(page.locator(".cm-content")).toContainText("Decision: keep review fixtures deterministic");
 if(width===375)await page.locator(".wiki-workspace-folders-toggle").click();
 await directory.getByRole("combobox").selectOption("recent");
 const rows=directory.locator(".wiki-recent-note");
 await expect(rows).toHaveCount(2);
 await expect.poll(()=>directory.locator(".wiki-recent-notes").evaluate(el=>el.scrollWidth-el.clientWidth)).toBeLessThanOrEqual(1);
 await expect(rows.first()).toHaveText("Why fixtures stay deterministic");
 await rows.filter({hasText:"Fixture architecture"}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await expect(page.locator(".cm-content")).toContainText("Fixture architecture");
 if(width===375)await expect(directory).toBeHidden();
 expect(runtime.calls().filter(c=>["update_page","create_page_draft","page_move","knowledge_folder_create"].includes(c.command))).toHaveLength(0);
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

for (const mode of ["recent", "custom"]) test(`${mode} navigation honors failed save and New does not inherit an optional folder`,async({page})=>{
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[],fixture:createReviewDecisionFixture("wiki-folders"),delays:{update_page:150}});
 await page.goto("/");await openWikiNote(page,"Why fixtures stay deterministic");await openWikiNote(page,"Fixture architecture");
 const dir=page.locator(".wiki-workspace-directory");await dir.getByRole("combobox").selectOption(mode);
 const editor=page.getByRole("textbox",{name:"Page editor",exact:true});
 await editor.evaluate(el=>{el.setAttribute("data-proof","retained");const view=(el as any).cmTile.root.view;view.dispatch({selection:{anchor:view.state.doc.length}});view.focus();});
 await page.keyboard.insertText("\nProtected recent-navigation edit.");runtime.failNext("update_page","save refused");
 await dir.getByRole("button",{name:"Why fixtures stay deterministic",exact:true}).click();
 await expect(page.locator(".page-detail").getByRole("alert")).toBeVisible();await expect(editor).toHaveAttribute("data-proof","retained");
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Retry",exact:true}).click();await expect(page.locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 await dir.getByRole("button",{name:"Why fixtures stay deterministic",exact:true}).click();await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 await page.locator(".note-tab-create").click();await page.getByRole("textbox",{name:"Title",exact:true}).fill("Unfiled recent page");await page.getByRole("textbox",{name:"Content",exact:true}).fill("A new page needs no folder decision.");
 await expect.poll(()=>runtime.calls().filter(c=>c.command==="create_page_draft").length).toBe(1);
 const savedDrafts=await page.evaluate(async()=>await window.__TAURI_INTERNALS__!.invoke("list_pages",{status:"draft",limit:100,offset:0})) as Array<{id:string;title:string;content:string;folder_path?:string|null}>;
 const savedDraft=savedDrafts.find(draft=>draft.title==="Unfiled recent page");
 expect(savedDraft).toMatchObject({title:"Unfiled recent page",content:"A new page needs no folder decision.",folder_path:null});
 const createArgs=runtime.calls().find(c=>c.command==="create_page_draft")?.args as Record<string, unknown>;
 expect(createArgs).toMatchObject({title:"Unfiled recent page",content:"A new page needs no folder decision."});
 expect(createArgs).not.toHaveProperty("folderPath");
 await expect(page.getByRole("alert")).toHaveCount(0);
 await expect(dir.getByRole("combobox")).toHaveValue(mode);
});

test("recent panel and Search align in either rail mode",async({page})=>{
 await page.setViewportSize({width:1440,height:900});await installTauriMock(page,{locale:"en",rawActions:[]});await page.goto("/");
 for(const width of [240,64]){
  const separator=page.getByRole("separator",{name:/sidebar/i});
  if(width===64){await separator.focus();for(let i=0;i<3;i++)await page.keyboard.press("ArrowLeft");}
  const search=page.locator(".notes-sidebar-search button");
  const mode=page.locator(".wiki-workspace-mode select");
  const a=await search.boundingBox(),b=await mode.boundingBox();
  expect(a).not.toBeNull();expect(b).not.toBeNull();expect(Math.abs(a!.y+a!.height/2-b!.y-b!.height/2)).toBeLessThanOrEqual(1);
 }
});
