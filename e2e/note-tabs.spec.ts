// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page, type Locator } from "@playwright/test";
import { showWikiFolders } from "./helpers/wikiWorkspace";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
async function start(page:Page) {
 await page.setViewportSize({width:1280,height:900});
 const errors=collectBrowserErrors(page);
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[],fixture:createReviewDecisionFixture("wiki-folders"),delays:{update_page:250}});
 await page.goto("/");await showWikiFolders(page);const tree=page.locator(".wiki-workspace-directory");
 await tree.getByRole("button",{name:"Work",exact:true}).click();
 await tree.getByRole("button",{name:"Open Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 return {runtime,errors,tree};
}
async function source(editor:Locator) { return editor.evaluate(el=>{const view=(el as any).cmTile?.root?.view;if(!view)throw new Error("Missing editor");return view.state.doc.toString() as string;}); }
async function append(page:Page,text:string) { const editor=page.getByRole("textbox",{name:"Page editor",exact:true});await editor.evaluate(el=>{const view=(el as any).cmTile.root.view;view.dispatch({selection:{anchor:view.state.doc.length}});view.focus();});await page.keyboard.insertText(text);return editor; }
test("files open distinct tabs, deduplicate, and retain saved text when switching",async({page})=>{
 const {tree,runtime,errors}=await start(page);
 await append(page,"\nA saved tab edit.");
 await tree.getByRole("button",{name:"Research",exact:true}).click();
 await tree.getByRole("button",{name:"Open Why fixtures stay deterministic",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(2);
 expect(runtime.calls().filter(c=>c.command==="update_page")).toHaveLength(1);
 await tree.getByRole("button",{name:"Open Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await expect.poll(()=>source(page.getByRole("textbox",{name:"Page editor",exact:true}))).toContain("A saved tab edit.");
 await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(2);
 const tab=page.getByRole("tab",{name:"Fixture architecture",exact:true});await tab.focus();await tab.press("End");await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toBeFocused();
 await page.keyboard.press("Enter");await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Close Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(1);
 await page.getByRole("button",{name:"Close Why fixtures stay deterministic",exact:true}).click();
 await expect(page.getByRole("tablist")).toHaveCount(0);await expect(page.locator(".wiki-overview")).toBeVisible();
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
test("failed save prevents tab switch and closing, with the same live editor",async({page})=>{
 const {tree,runtime}=await start(page);
 await tree.getByRole("button",{name:"Research",exact:true}).click();await tree.getByRole("button",{name:"Open Why fixtures stay deterministic",exact:true}).click();
 await page.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await expect(page.locator(".cm-content")).toContainText("Fixture architecture");
 const editor=await append(page,"\nKeep this unsaved text.");await editor.evaluate(el=>el.setAttribute("data-session","retained"));
 runtime.failNext("update_page","save refused");await page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click();
 await expect(page.locator(".page-detail").getByRole("alert")).toBeVisible();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");await expect(editor).toHaveAttribute("data-session","retained");
 await page.getByRole("button",{name:"Close Fixture architecture",exact:true}).click();
 expect(runtime.calls().filter(c=>c.command==="update_page")).toHaveLength(1);
 await expect(editor).toHaveAttribute("data-session","retained");await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(2);
 await expect.poll(()=>source(editor)).toContain("Keep this unsaved text.");
 await page.getByRole("button",{name:"Retry",exact:true}).click();
 await expect(page.locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 expect(runtime.calls().filter(c=>c.command==="update_page")).toHaveLength(2);
 await page.getByRole("button",{name:"Close Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveCount(0);await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveAttribute("aria-selected","true");
});
test("draft tab keeps its identity and title through automatic finalization and switching",async({page})=>{
 const {tree,runtime}=await start(page);
 await tree.getByRole("button",{name:"Open Next experiment",exact:true}).click();
 await expect(page.getByRole("textbox",{name:"Title",exact:true})).toHaveValue("Next experiment");
 await page.getByRole("textbox",{name:"Title",exact:true}).fill("Edited draft tab");
 await page.getByRole("textbox",{name:"Content",exact:true}).fill("Draft stays available.");
 await page.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("tab",{name:"Edited draft tab",exact:true}).click();
 await expect(page.getByRole("heading",{level:1,name:"Edited draft tab",exact:true})).toBeVisible();
 const editor=page.getByRole("textbox",{name:"Page editor",exact:true});
 await expect.poll(()=>source(editor)).toContain("Draft stays available.");
 await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(2);
 const publishArgs=runtime.calls().find(call=>call.command==="publish_page_draft")?.args as {id:string;expectedVersion:number}|undefined;
 expect(publishArgs).toMatchObject({id:"page-writing-draft"});
 const saved=await page.evaluate(async id=>{
  const internals=window.__TAURI_INTERNALS__;
  if(!internals)throw new Error("Expected the installed Tauri mock before reading the saved Page");
  return internals.invoke("get_page",{id});
 },publishArgs!.id) as {id:string;version:number;status:string;title:string;content:string};
 expect(saved).toMatchObject({id:publishArgs!.id,status:"active",title:"Edited draft tab",content:"Draft stays available."});
 expect(saved.version).toBe(publishArgs!.expectedVersion+1);
 await expect(page.getByRole("tab",{name:"Edited draft tab",exact:true})).toHaveAttribute("aria-selected","true");
});

test("closing a pending destination does not reopen it after the current save finishes",async({page})=>{
 const {tree}=await start(page);
 await tree.getByRole("button",{name:"Research",exact:true}).click();await tree.getByRole("button",{name:"Open Why fixtures stay deterministic",exact:true}).click();
 await page.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 const editor=await append(page,"\nSaved while another tab closes.");
 await page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click();
 await page.getByRole("button",{name:"Close Why fixtures stay deterministic",exact:true}).click();
 await expect(page.locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 await expect(page.getByRole("tab",{name:"Why fixtures stay deterministic",exact:true})).toHaveCount(0);
 await expect.poll(()=>source(editor)).toContain("Saved while another tab closes.");
});


test("reading area never duplicates the tree before opening or after closing the last page",async({page})=>{
 await page.setViewportSize({width:1280,height:900});
 const errors=collectBrowserErrors(page);
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[],fixture:createReviewDecisionFixture("wiki-folders"),localStorage:{"wenlan-wiki-view-mode":"cards"}});
 await page.goto("/");await showWikiFolders(page);
 const reading=page.locator(".wiki-workspace-reading"), tree=page.locator(".wiki-workspace-directory");
 await expect(tree.getByRole("button",{name:"Work",exact:true})).toBeVisible();
 await expect(reading.getByRole("table")).toHaveCount(0);
 await expect(reading.locator(".asset-card, .wiki-folder-lenses")).toHaveCount(0);
 await expect(reading.getByRole("heading",{name:"Open a page",exact:true})).toBeVisible();
 await tree.getByRole("button",{name:"Work",exact:true}).click();
 await expect(tree.getByRole("button",{name:"Open Fixture architecture",exact:true})).toBeVisible();
 await expect(reading.getByText("Fixture architecture",{exact:true})).toHaveCount(1);await expect(reading.getByTestId("wiki-new-pages").getByText("Fixture architecture",{exact:true})).toBeVisible();
 expect(runtime.calls().filter(c=>c.command==="get_page_explicit_browse")).toHaveLength(0);
 await tree.getByRole("button",{name:"Open Fixture architecture",exact:true}).click();
 await expect(reading.locator(".page-detail")).toBeVisible();
 await expect(page.getByRole("tab",{name:"Fixture architecture",exact:true})).toHaveAttribute("aria-selected","true");
 await page.getByRole("button",{name:"Close Fixture architecture",exact:true}).click();
 await expect(reading.getByRole("heading",{name:"Open a page",exact:true})).toBeVisible();
 await expect(reading.getByRole("table")).toHaveCount(0);
 await expect(reading.locator(".asset-card, .wiki-folder-lenses")).toHaveCount(0);
 await expect(reading.getByText("Fixture architecture",{exact:true})).toHaveCount(1);await expect(reading.getByTestId("wiki-new-pages").getByText("Fixture architecture",{exact:true})).toBeVisible();
 await expect(page.locator(".note-tab-create")).toBeVisible();
 await expect(page.locator(".note-tab-create")).toBeFocused();
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});


test("tab Plus waits for a successful save and preserves the selected folder", async ({ page }) => {
 const {runtime,errors} = await start(page);
 const editor=await append(page,"\nPreserve before creating a page.");
 await editor.evaluate(el=>el.setAttribute("data-session","plus-retained"));
 runtime.failNext("update_page","save refused");
 await page.getByRole("button",{name:"New page",exact:true}).click();
 await expect(page.locator(".page-detail").getByRole("alert")).toBeVisible();
 await expect(editor).toHaveAttribute("data-session","plus-retained");
 await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(1);
 await expect.poll(()=>source(editor)).toContain("Preserve before creating a page.");
 await page.getByRole("button",{name:"Retry",exact:true}).click();
 await expect(page.locator('.page-detail .sr-only[role="status"]')).toHaveText("Saved");
 await page.getByRole("button",{name:"New page",exact:true}).click();
 await expect(page.getByRole("textbox",{name:"Title",exact:true})).toBeFocused();
 await expect(page.getByRole("tablist").getByRole("tab")).toHaveCount(2);
 await page.getByRole("textbox",{name:"Title",exact:true}).fill("New tab page");
 await page.getByRole("textbox",{name:"Content",exact:true}).fill("New page in Work");
 await page.getByRole("tab",{name:"Fixture architecture",exact:true}).click();
 expect(runtime.calls().find(c=>c.command==="create_page_draft")?.args).toEqual(expect.objectContaining({folderPath:"Work"}));
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
