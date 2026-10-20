// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page, type Locator } from "@playwright/test";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
const group=(p:Page,id="primary")=>p.locator(`[data-note-group-id="${id}"]`);
const tab=(p:Page,n:number,id="primary")=>group(p,id).getByRole("tab",{name:`Drag page ${n}`,exact:true});
const order=(p:Page,id="primary")=>group(p,id).locator('.note-tabs [role="tab"]').allTextContents();
const editorSource=(editor:Locator)=>editor.evaluate(el=>(el as any).cmTile.root.view.state.doc.toString() as string);
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
async function split(p:Page,n:number){
 const b=(await group(p).locator('.note-group-content').boundingBox())!;
 await drag(p,tab(p,n),{x:b.x+b.width-15,y:b.y+100});
}
async function menu(p:Page,n:number,id="primary"){
 await tab(p,n,id).click({button:"right"});
 await p.getByRole("menuitem",{name:id==="primary"?"Move to right group":"Move to central group",exact:true}).click();
}
async function assertClean(errors:ReturnType<typeof collectBrowserErrors>){expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);}

test("reorder keeps active editor, edit state and reading position; click remains click",async({page},info)=>{
 const{errors}=await start(page);const editor=group(page).getByRole("textbox",{name:"Page editor",exact:true});
 await editor.evaluate(el=>el.setAttribute('data-instance','retained'));
 const scroller=group(page).locator('.note-group-content');await scroller.evaluate(el=>el.scrollTop=500);
 await drag(page,tab(page,3),await point(tab(page,1),"before"));
 await expect.poll(()=>order(page)).toEqual(["Drag page 3","Drag page 1","Drag page 2"]);
 await expect(editor).toHaveAttribute('data-instance','retained');await expect(tab(page,3)).toHaveAttribute('aria-selected','true');
 await expect.poll(()=>scroller.evaluate(el=>el.scrollTop)).toBe(500);
 await tab(page,1).click();await expect(tab(page,1)).toHaveAttribute('aria-selected','true');
 await expect(group(page).getByRole('button',{name:'Open beside',exact:true})).toHaveCount(0);
 await page.screenshot({path:info.outputPath('reordered.png')});await assertClean(errors);
});

test("split preview, destination insertion, reverse transfer and independent inspectors",async({page},info)=>{
 const{errors}=await start(page,2560);const left=group(page),scroll=left.locator('.note-group-content');
 await left.getByRole('button',{name:'Open page sidebar',exact:true}).click();
 await scroll.evaluate(el=>el.scrollTop=480);
 const b=(await scroll.boundingBox())!;
 await hold(page,tab(page,3),{x:b.x+b.width-12,y:b.y+110});
 await page.screenshot({path:info.outputPath('split-preview.png')});await page.mouse.up();
 await expect(tab(page,3,'secondary')).toHaveAttribute('aria-selected','true');
 await expect.poll(()=>group(page,'secondary').locator('.note-group-content').evaluate(el=>el.scrollTop)).toBe(480);
 await expect(left.getByRole('tab',{name:'Info',exact:true})).toBeVisible();
 await group(page,'secondary').getByRole('button',{name:'Open page sidebar',exact:true}).click();
 await group(page,'secondary').getByRole('tab',{name:'Mind map',exact:true}).click();
 await drag(page,tab(page,1),await point(tab(page,3,'secondary'),'before'));
 await expect.poll(()=>order(page,'secondary')).toEqual(['Drag page 1','Drag page 3']);
 await expect(group(page,'secondary').getByRole('tab',{name:'Mind map',exact:true})).toHaveAttribute('aria-selected','true');
 await expect(left.getByRole('tab',{name:'Info',exact:true})).toHaveAttribute('aria-selected','true');
 await hold(page,tab(page,3,'secondary'),await point(tab(page,2),'before'));
 await page.screenshot({path:info.outputPath('insertion-preview.png')});await page.mouse.up();
 await expect.poll(()=>order(page)).toEqual(['Drag page 3','Drag page 2']);
 await drag(page,tab(page,1,'secondary'),await point(tab(page,2),'after'));
 await expect(group(page,'secondary')).toHaveCount(0);
 await expect.poll(()=>order(page)).toEqual(['Drag page 3','Drag page 2','Drag page 1']);
 await page.screenshot({path:info.outputPath('moved-back.png')});await assertClean(errors);
});

test("Escape, invalid area and viewport changes cancel without navigation",async({page})=>{
 const{errors}=await start(page,1920);
 const original=await order(page);const target=await point(tab(page,1),'before');
 await hold(page,tab(page,3),target);await page.keyboard.press('Escape');await page.mouse.up();
 expect(await order(page)).toEqual(original);await expect(tab(page,3)).toHaveAttribute('aria-selected','true');
 await drag(page,tab(page,3),{x:10,y:350});expect(await order(page)).toEqual(original);
 await hold(page,tab(page,3),target);await page.setViewportSize({width:1850,height:960});await page.mouse.up();expect(await order(page)).toEqual(original);
 await tab(page,1).focus();await page.keyboard.press('Shift+F10');
 await expect(page.getByRole('menuitem',{name:'Move to right group',exact:true})).toBeFocused();
 await page.keyboard.press('Escape');await expect(tab(page,1)).toBeFocused();
 await assertClean(errors);
});

test("failed source save and failed target save preserve both editors and ownership",async({page},info)=>{
 const{runtime,errors}=await start(page,1440,'dark');
 const editor=group(page).getByRole('textbox',{name:'Page editor',exact:true});
 runtime.failNext('update_page','save refused',3);
 await editor.evaluate(el=>{const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});await page.keyboard.insertText('\nUnsaved source survives.');
 await split(page,3);
 await expect(group(page).getByRole('alert')).toBeVisible();await expect(group(page,'secondary')).toHaveCount(0);await expect(editor).toContainText('Unsaved source survives.');
 // Fresh page context below tests the other side of the same save transaction.
 await page.screenshot({path:info.outputPath('failed-source.png')});await assertClean(errors);
});

test("a failing destination cannot absorb the dragged source",async({page})=>{
 const{runtime,errors}=await start(page);await menu(page,3);await expect(tab(page,3,'secondary')).toBeVisible();
 const right=group(page,'secondary').getByRole('textbox',{name:'Page editor',exact:true});
 runtime.failNext('update_page','destination save refused',3);
 await right.evaluate(el=>{const v=(el as any).cmTile.root.view;v.dispatch({selection:{anchor:v.state.doc.length}});v.focus();});await page.keyboard.insertText('\nDestination draft survives.');
 await drag(page,tab(page,1),await point(tab(page,3,'secondary'),'before'));
 await expect(group(page,'secondary').getByRole('alert')).toBeVisible();await expect(tab(page,1)).toBeVisible();await expect(tab(page,1,'secondary')).toHaveCount(0);await expect(right).toContainText('Destination draft survives.');await assertClean(errors);
});

test("overflow auto-scroll permits distant reordering while fixed controls stay reachable",async({page},info)=>{
 const{errors}=await start(page,1280,'dark',12);const strip=group(page).locator('.note-tabs-list');
 const before=await strip.evaluate(el=>el.scrollLeft);expect(before).toBeGreaterThan(0);
 const box=(await strip.boundingBox())!;await hold(page,tab(page,12),{x:box.x+6,y:box.y+box.height/2});
 await expect.poll(()=>strip.evaluate(el=>el.scrollLeft),{timeout:10000}).toBe(0);
 await page.mouse.move(box.x+5,box.y+box.height/2);await page.mouse.up();
 await expect.poll(async()=> (await order(page))[0]).toBe('Drag page 12');
 const toggle=group(page).getByRole('button',{name:'Open page sidebar',exact:true});await expect(toggle).toBeInViewport();await expect(group(page).getByRole('button',{name:'New page',exact:true})).toBeInViewport();
 expect(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth)).toBe(false);
 await page.screenshot({path:info.outputPath('overflow.png')});await assertClean(errors);
});

test("narrow groups can move in both directions without a third group",async({page},info)=>{
 const{errors}=await start(page,375,'dark',2);await split(page,2);await expect(tab(page,2,'secondary')).toBeVisible();
 await group(page,'secondary').getByRole('button',{name:'Open page sidebar',exact:true}).click();
 await expect(group(page,'secondary').getByRole('tab',{name:'Info',exact:true})).toBeVisible();
 await group(page,'secondary').getByRole('button',{name:'Close',exact:true}).click();
 await drag(page,tab(page,2,'secondary'),await point(tab(page,1),'before'));
 await expect(group(page,'secondary')).toHaveCount(0);expect(await order(page)).toEqual(['Drag page 2','Drag page 1']);
 await page.screenshot({path:info.outputPath('narrow.png')});await assertClean(errors);
});

test("last central tab can return to an empty group and a draft finalizes with its identity on drag",async({page})=>{
 const{errors}=await start(page,1440,'light',1);await split(page,1);await expect(tab(page,1,'secondary')).toBeVisible();
 const empty=await group(page).locator('.note-tabs').boundingBox();
 await drag(page,tab(page,1,'secondary'),{x:empty!.x+empty!.width/2,y:empty!.y+empty!.height/2});
 await expect(group(page,'secondary')).toHaveCount(0);await expect(tab(page,1)).toBeVisible();
 await group(page).getByRole('button',{name:'New page',exact:true}).click();
 await group(page).getByRole('textbox',{name:'Title',exact:true}).fill('A draft moved safely');
 await group(page).getByRole('textbox',{name:'Content',exact:true}).fill('Saved content before moving a draft.');
 const draftTab=group(page).getByRole('tab',{name:'A draft moved safely',exact:true});
 const box=(await group(page).locator('.note-group-content').boundingBox())!;
 await drag(page,draftTab,{x:box.x+box.width-15,y:box.y+100});
 const movedEditor=group(page,'secondary').getByRole('textbox',{name:'Page editor',exact:true});
 await expect(movedEditor).toBeVisible();
 await expect(group(page,'secondary').locator('.page-detail').getByRole('status')).toHaveText('Saved');
 await expect.poll(()=>editorSource(movedEditor)).toContain('Saved content before moving a draft.');
 await expect(group(page).getByRole('tab',{name:'A draft moved safely',exact:true})).toHaveCount(0);
 await expect(page.getByRole('tab',{name:'A draft moved safely',exact:true})).toHaveCount(1);
 await assertClean(errors);
});
