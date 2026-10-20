// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page, type Locator } from "@playwright/test";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
const group=(p:Page,id="primary")=>p.locator(`[data-note-group-id="${id}"]`);
const tab=(p:Page,n:number,id="primary")=>group(p,id).getByRole("tab",{name:`Drag page ${n}`,exact:true});
const order=(p:Page,id="primary")=>group(p,id).locator('.note-tabs [role="tab"]').allTextContents();
async function start(p:Page,width=1440,theme="light",count=3){
 await p.setViewportSize({width,height:960});
 const errors=collectBrowserErrors(p),base=createReviewDecisionFixture("wiki-folders");
 const fixture={...base,pages:Array.from({length:12},(_,i)=>({...base.pages[0],id:`drag-${i+1}`,title:`Drag page ${i+1}`,content:`# Drag page ${i+1}\n\n`+Array.from({length:55},(_,j)=>`Paragraph ${j+1}. A synthetic page for reading position and moving safely.`).join("\n\n"),storage_path:`drag-${i+1}.md`,folder_path:""}))};
 const runtime=await installTauriMock(p,{fixture,locale:"en",rawActions:[],delays:{update_page:200},localStorage:{"wenlan-theme":theme}});
 await p.goto("/");for(let i=1;i<=count;i++)await openWikiNote(p,`Drag page ${i}`);
 await expect(group(p).getByRole("textbox",{name:"Page editor",exact:true})).toBeVisible();
 return{errors,runtime};
}
async function point(el:Locator,side:"before"|"after"|"middle"="middle"){
 const b=(await el.boundingBox())!;return{x:b.x+(side==="before"?3:side==="after"?b.width-3:b.width/2),y:b.y+b.height/2};
}
async function hold(p:Page,from:Locator,to:{x:number,y:number}){
 await from.scrollIntoViewIfNeeded();
 expect(await from.evaluate(el=>{const r=el.getBoundingClientRect();return el.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2));})).toBe(true);
 const a=await point(from);await p.mouse.move(a.x,a.y);await p.mouse.down();await p.mouse.move(a.x+8,a.y,{steps:2});await p.mouse.move(to.x,to.y,{steps:12});
}
async function drag(p:Page,from:Locator,to:{x:number,y:number}){await hold(p,from,to);await p.mouse.up();}
async function menu(p:Page,n:number,id="primary"){
 await tab(p,n,id).click({button:"right"});
 await p.getByRole("menuitem",{name:id==="primary"?"Move to right group":"Move to central group",exact:true}).click();
}
async function assertClean(errors:ReturnType<typeof collectBrowserErrors>){expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);}



for (const target of ['inspector', 'header']) test(`right ${target} creates a new group with inspector open`,async({page},info)=>{
 const {errors}=await start(page,1440);
 await group(page).getByRole('button',{name:'Open page sidebar',exact:true}).click();
 const drawer=group(page).getByRole('complementary');await expect(drawer).toBeVisible();
 const frame=(await group(page).boundingBox())!;
 const to={x:frame.x+frame.width-30,y:frame.y+(target==='header'?25:220)};
 await hold(page,tab(page,1),to);
 await page.screenshot({path:info.outputPath('before-release.png')});
 await expect(page.locator('.note-tab-split-preview[data-kind="split"]')).toBeVisible();
 await page.mouse.up();
 await expect(tab(page,1,'secondary')).toBeVisible();await expect(group(page,'secondary').getByRole('textbox',{name:'Page editor',exact:true})).toContainText('Drag page 1');
 await expect(tab(page,3)).toBeVisible();await expect(group(page).getByRole('tab',{name:'Info',exact:true})).toBeVisible();
 await expect(page.locator('[data-note-group-id]')).toHaveCount(2);
 await page.screenshot({path:info.outputPath('created-group.png')});await assertClean(errors);
});

for(const theme of ['light','dark']) test(`insertion line stays vertical and neutral ${theme}`, async({page},info)=>{
 const {errors}=await start(page,1440,theme);
 await hold(page,tab(page,3),await point(tab(page,1),'before'));
 const marker=page.locator('.note-tab-drop-marker');await expect(marker).toBeVisible();
 await page.screenshot({path:info.outputPath('insertion-line.png')});
 const rect=(await marker.boundingBox())!;expect(rect.width).toBeLessThanOrEqual(2);expect(rect.height).toBeGreaterThanOrEqual(24);
 await page.mouse.up();await expect.poll(()=>order(page)).toEqual(['Drag page 3','Drag page 1','Drag page 2']);await assertClean(errors);
});

test('existing opposite inspector accepts transfer; save refusal preserves source',async({page},info)=>{
 const {errors,runtime}=await start(page,2560,'dark');await menu(page,3);
 await group(page,'secondary').getByRole('button',{name:'Open page sidebar',exact:true}).click();
 const drawer=group(page,'secondary').getByRole('complementary');await expect(drawer).toBeVisible();
 await hold(page,tab(page,1),await point(drawer));
 await expect(page.locator('.note-tab-split-preview[data-kind="transfer"]')).toBeVisible();
 await page.mouse.up();await expect.poll(()=>order(page,'secondary')).toEqual(['Drag page 3','Drag page 1']);
 await expect(group(page,'secondary').getByRole('tab',{name:'Info',exact:true})).toBeVisible();
 const editor=group(page).getByRole('textbox',{name:'Page editor',exact:true});runtime.failNext('update_page','save refused',3);
 await editor.evaluate(el=>{const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});await page.keyboard.insertText(' Unsaved survives.');
 await drag(page,tab(page,2),await point(drawer));
 await expect(group(page).getByRole('alert')).toBeVisible();await expect(tab(page,2)).toBeVisible();await expect(editor).toContainText('Unsaved survives.');
 await expect(page.locator('[data-note-group-id]')).toHaveCount(2);
 await page.screenshot({path:info.outputPath('save-refused.png')});await assertClean(errors);
});

test('compact group overlay is a split target without trapping the source tab',async({page},info)=>{
 const {errors}=await start(page,375,'dark',2);
 await group(page).getByRole('button',{name:'Open page sidebar',exact:true}).click();
 await page.screenshot({path:info.outputPath('before-drag.png')});
 await info.attach('source-hit',{body:JSON.stringify(await tab(page,2).evaluate(el=>{const r=el.getBoundingClientRect();return{rect:r.toJSON(),hit:document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.outerHTML};})),contentType:'application/json'});
 const frame=(await group(page).boundingBox())!;
 await hold(page,tab(page,2),{x:frame.x+frame.width-22,y:frame.y+200});
 await expect(page.locator('.note-tab-split-preview[data-kind="split"]')).toBeVisible();
 await page.screenshot({path:info.outputPath('compact-preview.png')});await page.mouse.up();
 await expect(tab(page,2,'secondary')).toBeVisible();await expect(page.locator('[data-note-group-id]')).toHaveCount(2);
 await assertClean(errors);
});

test('expanded local inspector still exposes the right split target',async({page},info)=>{
 const {errors}=await start(page,1440);
 await group(page).getByRole('button',{name:'Open page sidebar',exact:true}).click();
 await group(page).getByRole('button',{name:'Expand workspace',exact:true}).click();
 await expect(group(page).locator('.page-info-drawer-expanded-group')).toBeVisible();
 await page.screenshot({path:info.outputPath('before-drag.png')});
 await info.attach('source-hit',{body:JSON.stringify(await tab(page,1).evaluate(el=>{const r=el.getBoundingClientRect();return{rect:r.toJSON(),hit:document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.outerHTML};})),contentType:'application/json'});
 const frame=(await group(page).boundingBox())!;
 await hold(page,tab(page,1),{x:frame.x+frame.width-30,y:frame.y+210});
 await expect(page.locator('.note-tab-split-preview[data-kind="split"]')).toBeVisible();
 await page.screenshot({path:info.outputPath('expanded-preview.png')});await page.mouse.up();
 await expect(tab(page,1,'secondary')).toBeVisible();await expect(page.locator('[data-note-group-id]')).toHaveCount(2);await assertClean(errors);
});
