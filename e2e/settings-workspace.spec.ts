// SPDX-License-Identifier: AGPL-3.0-only
import {expect,test,type Page} from "@playwright/test";
import {installTauriMock,collectBrowserErrors} from "./tauriMock";

async function enterSettings(page:Page) {
 await expect(page.locator(".memory-workspace-header")).toBeVisible();
 const show=page.getByRole("button",{name:"Show sidebar",exact:true});
 if(await show.isVisible()) await show.click();
 await page.locator(".identity-menu-trigger").click();
 await page.getByRole("menuitem",{name:"Settings",exact:true}).click();
 await expect(page.locator("main h1")).toHaveText("General");
}
async function showSettingsSidebar(page:Page) {
 await expect(page.locator(".memory-workspace-header")).toBeVisible();
 const show=page.getByRole("button",{name:"Show sidebar",exact:true});
 if(await show.isVisible()) await show.click();
 return page.locator(".settings-sidebar");
}
for(const layout of ["labels","icons","hidden","narrow"] as const) test(`Settings Home and five groups: ${layout}`,async({page})=>{
 await page.setViewportSize({width:layout==="narrow"?375:1280,height:900});
 const errors=collectBrowserErrors(page);
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[],localStorage:{"wenlan-navigation-v1":JSON.stringify({version:2,visible:["pages","spaces","graph","sources"],sidebar:{visible:layout!=="hidden",mode:layout==="icons"?"icons":"labels"}})}});
 await page.goto("/");await enterSettings(page);
 for(const group of ["General","Intelligence","Diagnostics","Connections","Sources"]){
  const sidebar=await showSettingsSidebar(page);
  await expect(sidebar.getByRole("button",{name:"Back to home",exact:true})).toBeVisible();
  await expect(sidebar.getByRole("button",{name:"Search",exact:true})).toHaveCount(0);
  await sidebar.getByRole("button",{name:group,exact:true}).click();
  const heading=page.getByRole("heading",{name:group,exact:true,level:1});
  await expect(heading).toBeVisible();
  expect(await heading.evaluate(el=>el.getBoundingClientRect().height)).toBeGreaterThan(16);
 }
 if(layout==="hidden") await page.getByRole("button",{name:"Hide sidebar",exact:true}).click();
 const home=(layout==="hidden"||layout==="narrow"?page.locator(".memory-workspace-header"):page.locator(".settings-sidebar")).getByRole("button",{name:"Back to home",exact:true});
 await home.focus();await page.evaluate(async()=>{await window.__TAURI_INTERNALS__?.invoke("plugin:event|emit",{event:"toggle-spotlight",payload:null});});
 const dialog=page.getByRole("dialog",{name:"Search",exact:true});await expect(dialog).toBeVisible();await page.keyboard.press("Escape");
 await expect(home).toBeFocused();await home.press("Enter");
 await expect(page.locator(".wiki-workspace")).toBeVisible();await expect(page.locator(".settings-sidebar")).toHaveCount(0);
 await expect(page.locator('[data-sidebar-overlay="true"]')).toHaveCount(0);
 await page.locator(".workspace-history-navigation").getByRole("button",{name:"Back",exact:true}).click();
 await expect(page.locator("main h1")).toHaveText("Sources");
 if(layout==="narrow"){
  const sidebar=await showSettingsSidebar(page);await sidebar.getByRole("button",{name:"Back to home",exact:true}).click();
  await expect(page.locator(".wiki-workspace")).toBeVisible();await expect(page.locator('[data-sidebar-overlay="true"]')).toHaveCount(0);
 }
 expect(runtime.calls().filter(c=>/^(update_profile|set_config|update_page|set_provider)/.test(c.command))).toHaveLength(0);
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});

for (const width of [1440, 375]) test(`Settings controls fit their panels at ${width}`, async ({ page }, info) => {
 await page.setViewportSize({width,height:900});
 const errors=collectBrowserErrors(page);
 const runtime=await installTauriMock(page,{locale:"en",rawActions:[]});
 await page.addInitScript(()=>{
  const internals=window.__TAURI_INTERNALS__;
  if(!internals)throw new Error("Tauri mock must be installed before boot");
  const previous=internals.invoke;
  internals.invoke=async(command,args)=>{
   if(command==="get_profile")return {id:"layout-profile",name:"Example",display_name:"Example",email:null,bio:null,avatar_path:null,created_at:0,updated_at:0};
   if(command==="detect_mcp_clients_cmd")return [];
   return previous(command,args);
  };
 });
 await page.goto("/");await enterSettings(page);
 // The export action was clipped even at desktop width. Check the actual
 // clipping ancestor, not merely that the button exists or intersects the viewport.
 const exportButton=page.getByRole("button",{name:"Choose folder",exact:true});
 await page.evaluate(async()=>{await document.fonts.ready;});
 expect(await exportButton.evaluate(el=>{
  const rect=el.querySelector("span")!.getBoundingClientRect();let p=el.parentElement;
  while(p && p.tagName!=="MAIN"){
   if(["hidden","clip"].includes(getComputedStyle(p).overflowX)){
    const bounds=p.getBoundingClientRect();if(rect.left<bounds.left-1||rect.right>bounds.right+1)return false;
   }
   p=p.parentElement;
  }return true;
 })).toBe(true);
 for(const group of ["General","Intelligence","Diagnostics","Connections","Sources"]){
  const sidebar=await showSettingsSidebar(page);await sidebar.getByRole("button",{name:group,exact:true}).click();
  await expect(page.locator("main h1")).toHaveText(group);
  await page.evaluate(async()=>{await document.fonts.ready;await Promise.all(document.getAnimations().filter(a=>Number.isFinite(a.effect?.getComputedTiming().endTime)).map(a=>a.finished.catch(()=>undefined)));});
  const issues=await page.locator("main").evaluate(root=>[...root.querySelectorAll<HTMLElement>("button,input,select")].flatMap(el=>{
   const r=el.getBoundingClientRect();if(!r.width||!r.height)return [];
   let p=el.parentElement;
   while(p && p!==root){
    if(["hidden","clip"].includes(getComputedStyle(p).overflowX)){
     const b=p.getBoundingClientRect();if(r.left<b.left-1||r.right>b.right+1)return [el.textContent||el.getAttribute("aria-label")||el.tagName];
    }p=p.parentElement;
   }return [];
  }));
  expect(issues,`${group}: clipped controls`).toEqual([]);
  await page.screenshot({ path: info.outputPath(`settings-${group.toLowerCase()}-${width}.png`) });
  if(group==="Intelligence"&&width===375){
   for(const row of await page.locator('.settings-provider-row > button').all())expect((await row.boundingBox())!.width).toBeGreaterThan(180);
  }
 }
 expect(runtime.calls().filter(c=>/^(update_profile|set_config|set_provider|export_pages)/.test(c.command))).toHaveLength(0);
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
