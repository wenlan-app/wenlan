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
 const a=await point(from);await p.mouse.move(a.x,a.y);await p.mouse.down();await p.mouse.move(a.x+8,a.y,{steps:2});await p.mouse.move(to.x,to.y,{steps:12});
}
async function drag(p:Page,from:Locator,to:{x:number,y:number}){await hold(p,from,to);await p.mouse.up();}
async function menu(p:Page,n:number,id="primary"){
 await tab(p,n,id).click({button:"right"});
 await p.getByRole("menuitem",{name:id==="primary"?"Move to right group":"Move to central group",exact:true}).click();
}
async function assertClean(errors:ReturnType<typeof collectBrowserErrors>){expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);}


for (const theme of ['light','dark']) test(`gentle region preview and generous split target ${theme}`,async({page},info)=>{
 const {errors}=await start(page,1440,theme);
 const content=(await group(page).locator('.note-group-content').boundingBox())!;
 // This point is deliberately farther than the old 140px edge zone.
 await hold(page,tab(page,3),{x:content.x+content.width-200,y:content.y+150});
 const preview=page.locator('.note-tab-split-preview[data-kind="split"]');await expect(preview).toBeVisible();
 await expect(page.locator('.note-tab-drop-line')).toHaveCount(0);await expect(page.locator('.note-tab-drop-marker')).toHaveCount(0);
 await expect(tab(page,3).locator('..')).toHaveAttribute('data-dragging','true');
 const frame=(await group(page).boundingBox())!,p=(await preview.boundingBox())!;
 expect(p.x).toBeCloseTo(frame.x+frame.width/2+8,0);expect(p.width).toBeCloseTo(frame.width/2-16,0);expect(p.y).toBeCloseTo(frame.y+8,0);
 await page.screenshot({path:info.outputPath('split-preview.png')});await page.mouse.up();
 await expect(tab(page,3,'secondary')).toBeVisible();await expect(page.locator('body')).not.toHaveClass(/note-tab-dragging/);
 const right=(await group(page,'secondary').locator('.note-group-content').boundingBox())!;
 await hold(page,tab(page,1),{x:right.x+right.width/2,y:right.y+180});
 await expect(page.locator('.note-tab-split-preview[data-kind="transfer"]')).toBeVisible();
 await page.screenshot({path:info.outputPath('transfer-preview.png')});await page.mouse.up();
 await expect.poll(()=>order(page,'secondary')).toEqual(['Drag page 3','Drag page 1']);
 const central=(await group(page).locator('.note-group-content').boundingBox())!;
 await drag(page,tab(page,3,'secondary'),{x:central.x+central.width/2,y:central.y+180});
 await expect.poll(()=>order(page)).toEqual(['Drag page 2','Drag page 3']);
 await page.screenshot({path:info.outputPath('after-transfer.png')});await assertClean(errors);
});

test('reorder marker is short and neutral; release, Escape and blur clean pointer styling',async({page},info)=>{
 const {errors}=await start(page);
 await hold(page,tab(page,3),await point(tab(page,1),'before'));
 const marker=page.locator('.note-tab-drop-marker');await expect(marker).toBeVisible();const b=(await marker.boundingBox())!;
 expect(b.width).toBe(2);expect(b.height).toBe(28);await expect(page.locator('.note-tab-drop-line')).toHaveCount(0);
 await page.screenshot({path:info.outputPath('reorder-marker.png')});await page.keyboard.press('Escape');await page.mouse.up();
 await expect(page.locator('[data-dragging]')).toHaveCount(0);await expect(page.locator('body')).not.toHaveClass(/note-tab-dragging/);
 await hold(page,tab(page,3),await point(tab(page,1),'before'));await page.evaluate(()=>window.dispatchEvent(new Event('blur')));await page.mouse.up();
 await expect(page.locator('[data-dragging]')).toHaveCount(0);await expect(page.locator('body')).not.toHaveClass(/note-tab-dragging/);
 expect(await order(page)).toEqual(['Drag page 1','Drag page 2','Drag page 3']);await assertClean(errors);
});

test('body transfer obeys save protection',async({page},info)=>{
 const {errors,runtime}=await start(page,2560);await menu(page,3);
 await group(page,'secondary').getByRole('button',{name:'Open page sidebar',exact:true}).click();
 const inspector=group(page,'secondary').locator('.page-info-drawer-group-overlay-panel');await expect(inspector).toBeVisible();
 const editor=group(page).getByRole('textbox',{name:'Page editor',exact:true});runtime.failNext('update_page','save refused',3);
 await editor.evaluate(el=>{const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});await page.keyboard.insertText(' Unsaved stays.');
 const body=(await group(page,'secondary').locator('.note-group-content').boundingBox())!;
 await drag(page,tab(page,2),{x:body.x+body.width/2,y:body.y+160});
 await expect(group(page).getByRole('alert')).toBeVisible();await expect(tab(page,2)).toBeVisible();await expect(editor).toContainText('Unsaved stays.');
 await page.screenshot({path:info.outputPath('save-refused.png')});await assertClean(errors);
});

test('compact split preview matches stacked layout and reduced motion',async({page},info)=>{
 const {errors}=await start(page,375,'dark',2);await page.emulateMedia({reducedMotion:'reduce'});
 const frame=(await group(page).boundingBox())!,body=(await group(page).locator('.note-group-content').boundingBox())!;
 await hold(page,tab(page,2),{x:body.x+body.width-30,y:body.y+120});
 const preview=page.locator('.note-tab-split-preview');await expect(preview).toBeVisible();const p=(await preview.boundingBox())!;
 expect(p.x).toBeCloseTo(frame.x+8,0);expect(p.y).toBeCloseTo(frame.y+frame.height/2+8,0);
 await page.screenshot({path:info.outputPath('compact-preview.png')});await page.mouse.up();await expect(tab(page,2,'secondary')).toBeVisible();
 await assertClean(errors);
});
