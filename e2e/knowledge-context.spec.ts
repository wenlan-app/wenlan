// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { openMemoryContext, openPageTool } from "./helpers/pageTools";
const copies = [
  {locale:"en",actions:"Page actions",open:"Open Fixture architecture",info:"Page info",pane:"Page views",graph:"Connections",sources:"Source memories",context:"Info",used:"Used as a source in",recent:"Recent memories",close:"Close"},
] as const;
for (const copy of copies) for (const width of [1280,375]) {
  test(`context roundtrip ${copy.locale} ${width}`,async({page},testInfo)=>{
    const errors=collectBrowserErrors(page); await page.setViewportSize({width,height:900});
    await installTauriMock(page,{locale:copy.locale,rawActions:[]});await page.goto("/");
    await openWikiNote(page, "Fixture architecture");
    await openPageTool(page,copy.info,copy.actions);
    const wiki=page.getByRole("complementary",{name:copy.pane});await expect(wiki).toBeVisible();
    await wiki.locator(".note-info-source-list").getByRole("button",{name:/Fixture architecture$/}).press("Enter");
    await expect(page.locator(".memory-detail-reading")).toContainText("Typed fixtures");
    await expect(page.locator(".memory-detail-reading summary")).toHaveCount(0);
    await expect(page.getByRole("button",{name:copy.recent,exact:true})).toBeVisible();
    const memoryRole = width>=1100 ? "complementary" : "dialog";
    const memoryPanel=page.getByRole(memoryRole,{name:copy.context,exact:true});
    if(width>=1100) await expect(memoryPanel).toBeVisible();
    else await expect(memoryPanel).toHaveCount(0);
    await openMemoryContext(page,copy.context);
    const memory=page.getByRole(memoryRole,{name:copy.context});
    await expect(memory.getByRole("region",{name:copy.used})).toBeVisible();
    await page.screenshot({path:testInfo.outputPath("memory-context.png")});
    await memory.getByRole("region",{name:copy.used}).getByRole("button",{name:"Fixture architecture",exact:true}).press("Enter");
    await expect(page.locator(".page-detail")).toContainText("Fixture architecture");
    await expect(page.locator(".memory-detail-reading")).toHaveCount(0);
    expect(await page.evaluate(()=>document.documentElement.scrollWidth-innerWidth)).toBeLessThanOrEqual(1);
    expect(errors.pageErrors).toEqual([]);
  });
}
test("memory inventory navigation waits for autosave and refuses to leave when saving fails",async({page})=>{
  await page.setViewportSize({width:1280,height:900});
  const runtime=await installTauriMock(page,{locale:"en",rawActions:[]});
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  await openPageTool(page,"Page info");
  await page.locator(".note-info-source-list").getByRole("button",{name:/Fixture architecture$/}).click();
  const editor=page.getByRole("textbox",{name:"Memory reading",exact:true});
  const draft="Unfinished memory draft that must survive a failed save.";
  runtime.failNext("update_memory_cmd","synthetic autosave failure");
  await editor.fill(draft);

  const recent=page.getByRole("button",{name:"Recent memories",exact:true});
  await recent.click();
  const inventory=page.locator("#workspace-context-browser").getByRole("region",{name:"Recent memories",exact:true});
  const other=inventory.getByRole("button",{name:"Open memory: Fixture memory 1",exact:true});
  await other.press("Enter");

  await expect(editor).toHaveText(draft);
  await expect(page.getByRole("alert")).toContainText("Changes could not be saved. Your draft is still here.");
  expect(runtime.calls().filter(call=>call.command==="update_memory_cmd")).toHaveLength(1);
  await expect(page.getByRole("main",{name:"Memory dossier",exact:true})).toBeVisible();
  await recent.click();
  await expect(page.locator("#workspace-context-browser").getByRole("region",{name:"Recent memories",exact:true})
    .getByRole("button",{name:"Open memory: Fixture architecture",exact:true})).toHaveAttribute("aria-current","page");
  await recent.click();

  await page.getByRole("button",{name:"Retry saving",exact:true}).click();
  await expect.poll(()=>runtime.calls().filter(call=>call.command==="update_memory_cmd")).toHaveLength(2);
  await expect(page.getByRole("alert")).toHaveCount(0);

  await recent.click();
  const reopenedInventory=page.locator("#workspace-context-browser").getByRole("region",{name:"Recent memories",exact:true});
  const savedTarget=reopenedInventory.getByRole("button",{name:"Open memory: Fixture memory 1",exact:true});
  await savedTarget.press("Enter");
  await recent.click();
  await expect(page.locator("#workspace-context-browser").getByRole("region",{name:"Recent memories",exact:true})
    .getByRole("button",{name:"Open memory: Fixture memory 1",exact:true})).toHaveAttribute("aria-current","page");
  expect(runtime.calls().filter(call=>call.command==="update_memory_cmd")).toHaveLength(2);
});


test("page action Escape leaves editing intact and narrow context tabs reach the graph", async ({page}) => {
  await page.setViewportSize({width:375,height:812});
  await installTauriMock(page,{locale:"en",rawActions:[]}); await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  const editor=page.getByRole("textbox",{name:"Page editor",exact:true});
  await expect(editor).toBeVisible();
  await page.getByRole("button",{name:"Page actions",exact:true}).click();
  await page.getByRole("menu",{name:"Page actions",exact:true}).locator('[role="menuitem"]:not(:disabled)').first().press("Escape");
  await expect(page.getByRole("menu",{name:"Page actions",exact:true})).toHaveCount(0);
  await expect(editor).toBeVisible();
  await expect(page.getByRole("button",{name:"Page actions",exact:true})).toBeFocused();
  await openPageTool(page,"Page info");
  const infoTab=page.getByRole("tab",{name:"Info",exact:true});
  await infoTab.focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.getByRole("tab",{name:"Mind map",exact:true})).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("Enter");
  await expect(page.getByRole("group",{name:"Connections",exact:true})).toBeVisible();
  const firstConnection=page.getByRole("group",{name:"Connections",exact:true}).getByRole("button").first();
  let reachedGraph=false;
  for(let step=1;step<=8;step++){
    await page.keyboard.press("Tab");
    const focus=await page.evaluate(()=>{
      const header=document.querySelector(".page-info-drawer-group-overlay-panel .page-info-drawer-header");
      const pane=document.querySelector(".page-info-drawer-group-overlay-panel .page-info-drawer-content");
      const active=document.activeElement;
      return{inside:!!active&&(header?.contains(active)||pane?.contains(active)),name:active?.getAttribute("aria-label")||active?.textContent?.trim()||active?.tagName};
    });
    expect(focus.inside,`Tab ${step} stays within this inspector; focused ${focus.name}`).toBe(true);
    if(await firstConnection.evaluate(el=>el===document.activeElement)){reachedGraph=true;break;}
  }
  await expect(firstConnection).toBeFocused();
  expect(reachedGraph,"keyboard Tab reaches the first connection within eight stops").toBe(true);
});
