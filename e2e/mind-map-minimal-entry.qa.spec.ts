import {test,expect,type Page,type Locator} from "@playwright/test";
import {installTauriMock,collectBrowserErrors} from "./tauriMock";
import {openWikiNote} from "./helpers/wikiWorkspace";
import {resources} from "../src/i18n/resources";
const group=(p:Page,id="primary")=>p.locator(`[data-note-group-id="${id}"]`);
async function start(page:Page,locale:"en"|"zh-Hant"|"zh-Hans"="en",width=1440,mode="normal"){
 const errors=collectBrowserErrors(page);await page.setViewportSize({width,height:960});
 const runtime=await installTauriMock(page,{locale,rawActions:[],localStorage:{"wenlan-theme":width===375?"dark":"light"}});
 if(mode!=="normal")await page.addInitScript(mode=>{
  const orig=window.__wenlanTauriInvoke;let created=false;
  window.__wenlanTauriInvoke=async(command,args)=>{const result=await orig(command,args);if(command==="create_page_map_node")created=true;if(command==="get_page_map"&&result&&typeof result==="object"){
   if(mode==="empty"&&!created)return {...result,nodes:[],revision:0};
   if(mode==="unsupported")return {...result,independent_ideas:false};
  }return result;};
 },mode);
 await page.goto("/");await openWikiNote(page,"Fixture architecture");
 const tr=resources[locale].translation;
 await group(page).getByRole("button",{name:tr.pageInspector.open,exact:true}).click();
 await group(page).getByRole("tab",{name:tr.pageCanvas.tabCanvas,exact:true}).click();
 const surface=group(page).locator(".page-canvas-surface");await expect(surface).toBeVisible();
 await expect(group(page).locator(".page-canvas-new-node")).toHaveCount(0);
 await expect(group(page).getByRole("button",{name:/^(Apply to page|Improve|Generate canvas|Preview changes)$/})).toHaveCount(0);
 return {runtime,errors,tr};
}
async function openNewNode(frame:Locator){
 const surface=frame.locator(".page-canvas-surface");const box=(await surface.boundingBox())!;
 await surface.click({button:"right",position:{x:box.width/2,y:box.height/2}});
 const menu=frame.getByRole("menu",{name:"Canvas actions"});
 await expect(menu).toBeVisible();
 await expect(menu.getByRole("menuitem",{name:"New node",exact:true})).toBeVisible();
 await menu.getByRole("menuitem",{name:"New node",exact:true}).click();
}
function noArticleWrites(runtime:Awaited<ReturnType<typeof installTauriMock>>){expect(runtime.calls().filter(c=>["update_page","improve_page_map"].includes(c.command))).toEqual([]);}
test("pane menu and N create an independent node without article writes",async({page},info) => {
 const {runtime,errors}=await start(page);const f=group(page);await openNewNode(f);
 const input=f.getByRole("textbox",{name:"Name this node",exact:true});await expect(input).toBeFocused();
 await input.fill("Do not submit");await page.keyboard.press("Escape");await expect(input).toHaveCount(0);
 await f.locator(".page-canvas-surface").focus();await page.keyboard.press("n");await expect(input).toBeFocused();
 await input.fill("A separate idea");await page.keyboard.press("Enter");await expect(input).toHaveCount(0);
 await expect(f.locator(".react-flow__node").filter({hasText:"A separate idea"})).toBeVisible();
 expect(runtime.calls().filter(c=>c.command==="create_page_map_node")).toHaveLength(1);noArticleWrites(runtime);
 await page.screenshot({path:info.outputPath("clean-map.png")});expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
test("empty map menu entry retains uncertain-create retry identity",async({page},info) => {
 const {runtime,errors}=await start(page,"en",1280,"empty");const f=group(page);await page.screenshot({path:info.outputPath("empty-map.png")});
 await openNewNode(f);const input=f.getByRole("textbox",{name:"Node name",exact:true});await expect(input).toBeFocused();
 await input.fill("My first idea");runtime.failNext("create_page_map_node","simulated offline");await page.keyboard.press("Enter");
 await expect(f.getByRole("alert")).toContainText("could not be saved");await expect(input).toHaveValue("My first idea");
 await page.screenshot({path:info.outputPath("retry-node.png")});await f.getByRole("button",{name:"Add node",exact:true}).click();
 await expect(input).toHaveCount(0);await expect(f.locator(".react-flow__node").filter({hasText:"My first idea"})).toBeVisible();
 const creates=runtime.calls().filter(c=>c.command==="create_page_map_node");expect(creates).toHaveLength(2);expect(creates[0].args).toEqual(creates[1].args);
 noArticleWrites(runtime);expect(errors.pageErrors).toEqual([]);
});
test("N is group-local and does not capture page editing",async({page},info) => {
 const {runtime,errors}=await start(page,"en",1920);await openWikiNote(page,"Why fixtures stay deterministic");
 await group(page).getByRole("tab",{name:"Why fixtures stay deterministic",exact:true}).click({button:"right"});await page.getByRole("menuitem",{name:"Move to right group",exact:true}).click();
 const left=group(page),right=group(page,"secondary");
 for(const f of [left,right]){const open=f.getByRole("button",{name:"Open page sidebar",exact:true});if(await open.count())await open.click();await f.getByRole("tab",{name:"Mind map",exact:true}).click();await expect(f.locator(".page-canvas-surface")).toBeVisible();}
 await right.locator(".page-canvas-surface").focus();await page.keyboard.press("n");
 await expect(right.getByRole("textbox",{name:"Name this node",exact:true})).toBeVisible();await expect(left.getByRole("textbox",{name:"Name this node",exact:true})).toHaveCount(0);await page.keyboard.press("Escape");
 await left.getByRole("tab",{name:"Info",exact:true}).click();await expect(left.locator(".page-canvas-surface")).toHaveCount(0);await expect(right.locator(".page-canvas-surface")).toBeVisible();
 const editor=left.getByRole("textbox",{name:"Page editor",exact:true});await editor.click();await page.keyboard.press("n");
 await expect(page.getByRole("textbox",{name:"Name this node",exact:true})).toHaveCount(0);await page.keyboard.press("Backspace");
 await page.screenshot({path:info.outputPath("local-controls.png")});expect(runtime.calls().filter(c=>c.command==="create_page_map_node")).toHaveLength(0);
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
test("unsupported backend disables menu creation and N safely",async({page}) => {
 const {runtime,errors}=await start(page,"en",1280,"unsupported");const f=group(page);const surface=f.locator(".page-canvas-surface");
 const box=(await surface.boundingBox())!;await surface.click({button:"right",position:{x:box.width/2,y:box.height/2}});
 const menu=f.getByRole("menu",{name:"Canvas actions"});await expect(menu).toBeVisible();await expect(menu.getByRole("menuitem",{name:"New node",exact:true})).toBeDisabled();
 await page.keyboard.press("Escape");await surface.focus();await page.keyboard.press("n");
 await expect(f.getByRole("textbox",{name:"Name this node",exact:true})).toHaveCount(0);await expect(f.getByRole("textbox",{name:"Node name",exact:true})).toHaveCount(0);
 expect(runtime.calls().filter(c=>c.command==="create_page_map_node")).toHaveLength(0);noArticleWrites(runtime);expect(errors.pageErrors).toEqual([]);
});
