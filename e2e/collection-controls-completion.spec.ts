// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

export async function addSourceFixtures(page:Page){
 await page.evaluate(()=>{const bridge=window.__TAURI_INTERNALS__!;const original=bridge.invoke;bridge.invoke=(command,args)=>{
  if(command==="list_registered_sources")return Promise.resolve([]) as ReturnType<typeof original>;
  if(command==="list_indexed_files")return Promise.resolve([{source:"file",source_id:"reading.md",title:"Reading pages",chunk_count:1,last_modified:1},{source:"webpage",source_id:"https://example.org/article",url:"https://example.org/article",title:"Research article",chunk_count:1,last_modified:1}]) as ReturnType<typeof original>;
  if(command==="get_chunks")return Promise.resolve([{id:"test-source",content:"Saved source preview content.",chunk_index:0,chunk_type:null,language:null}]) as ReturnType<typeof original>;
  return original(command,args);
 };});
}
for(const width of [1280,375])test(`Space detail shares Cards/List across Pages and Sources ${width}`,async({page})=>{
 await page.setViewportSize({width,height:900});const errors=collectBrowserErrors(page);await installTauriMock(page,{locale:"en",rawActions:[],localStorage:{"wenlan-spaces-view-mode":"cards"}});await page.goto("/");await addSourceFixtures(page);
 await openPrimaryDestination(page,"Spaces");await expect(page.getByRole("button",{name:"New",exact:true})).toBeVisible();
 const description=page.getByTestId("space-card-space-wenlan").locator(".asset-card-context");await description.scrollIntoViewIfNeeded();const rect=(await description.boundingBox())!;await page.mouse.click(rect.x+rect.width/2,rect.y+rect.height/2);await expect(page.getByRole("heading",{name:"Wenlan",level:1})).toBeVisible();
 await page.getByTestId("asset-lens-cards").click();const notes=page.locator(".space-dossier-page-list");await expect(notes).toHaveAttribute("data-lens","cards");const titles=await notes.getByRole("button").allTextContents();expect(titles.length).toBeGreaterThan(0);
 await page.getByRole("tab",{name:"Sources",exact:true}).click();await expect(page.locator(".space-project-sources")).toHaveAttribute("data-lens","cards");await page.locator(".space-project-sources .source-library-row-meta").first().click();await expect(page.getByRole(width>=1100?"complementary":"dialog",{name:"Reading pages",exact:true})).toContainText("Saved source preview content");await page.keyboard.press("Escape");
 await page.getByTestId("asset-lens-rows").click();await page.getByRole("tab",{name:"Pages",exact:true}).click();await expect(notes).toHaveAttribute("data-lens","rows");expect(await notes.getByRole("button").allTextContents()).toEqual(titles);
 await page.getByRole("button",{name:"Actions for Wenlan",exact:true}).click();const action=page.getByRole("menuitem",{name:"Review page changes",exact:true});await expect(action).toBeVisible();expect(await action.evaluate(el=>({wrap:getComputedStyle(el).whiteSpace,inside:el.getBoundingClientRect().left>=0 && el.getBoundingClientRect().right<=innerWidth}))).toEqual({wrap:"nowrap",inside:true});await page.keyboard.press("Escape");
 await notes.locator(".space-dossier-page-summary").first().click();await expect(page.locator(".page-detail")).toBeVisible();expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

for(const width of [1280,375])test(`Sources and Memories expose consistent collection lenses ${width}`,async({page})=>{
 await page.setViewportSize({width,height:900});const errors=collectBrowserErrors(page);await installTauriMock(page,{locale:"en",rawActions:[]});await page.goto("/");await addSourceFixtures(page);await openPrimaryDestination(page,"Sources");const library=page.locator(".source-library");
 await expect(library.getByRole("button",{name:"New",exact:true})).toBeVisible();await expect(library.locator("h1")).toHaveClass("sr-only");await expect(library.locator(".source-library-row")).toHaveCount(2);
 await library.getByTestId("asset-lens-cards").click();await expect(library.locator(".source-library-list")).toHaveAttribute("data-lens","cards");await library.getByRole("searchbox").fill("Reading");await expect(library.locator(".source-library-row")).toHaveCount(1);await library.getByTestId("asset-lens-rows").click();await expect(library.locator(".source-library-row")).toHaveCount(1);await library.getByRole("searchbox").fill("");
 await expect(library.getByRole("button",{name:"More source actions",exact:true})).toHaveCount(0);await library.getByRole("button",{name:"New",exact:true}).click();await expect(page.getByRole("dialog")).toBeVisible();await page.keyboard.press("Escape");
 await openPrimaryDestination(page,"Memories");await page.getByTestId("asset-lens-cards").click();await expect(page.locator(".memory-collection-card").first()).toBeVisible();const count=await page.locator(".memory-collection-card").count();await page.getByTestId("asset-lens-rows").click();await expect(page.locator(".memory-collection-card")).toHaveCount(0);await expect(page.locator(".memory-list-reading-row")).toHaveCount(count);expect(count).toBeGreaterThan(0);expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

test("Graph inspector uses the shared sidebar surface and a continuous divider",async({page})=>{
 await page.setViewportSize({width:1440,height:900});const errors=collectBrowserErrors(page);await installTauriMock(page,{locale:"en",rawActions:[]});await page.goto("/");await openPrimaryDestination(page,"Graph");await page.getByRole("combobox",{name:"Filter nodes"}).fill("Fixture architecture");await page.getByRole("option",{name:/Fixture architecture/}).click();const panel=page.locator(".page-info-drawer--inspector");await expect(panel).toBeVisible();
 const style=await panel.evaluate(el=>({bg:getComputedStyle(el).backgroundColor,top:el.getBoundingClientRect().top,bottom:el.getBoundingClientRect().bottom,leftBorder:getComputedStyle(el).borderLeftWidth}));const sidebar=await page.locator(".notes-workspace-sidebar").evaluate(el=>getComputedStyle(el).backgroundColor);expect(style.bg).toBe(sidebar);expect(style.top).toBeLessThanOrEqual(1);expect(style.bottom).toBe(900);expect(style.leftBorder).toBe("1px");await panel.getByRole("button",{name:"Open details",exact:true}).click();await expect(page.locator(".page-detail")).toBeVisible();expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
