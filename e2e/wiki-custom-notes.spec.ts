// SPDX-License-Identifier: AGPL-3.0-only
import {test,expect,type Page} from '@playwright/test';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {openWikiNote} from './helpers/wikiWorkspace';
const titles=['Fixture architecture','Why fixtures stay deterministic','Weekly fixture recap'];
const ids=['page-architecture','page-errors','page-keyboard'];
const orderKey='wenlan:wiki-inventory:v1';
async function boot(page:Page){await page.setViewportSize({width:1280,height:840});const errors=collectBrowserErrors(page);const runtime=await installTauriMock(page,{locale:'en',rawActions:[],preserveLocalStorage:true,localStorage:{'wenlan-theme':'dark','wenlan:recent-pages:v1':JSON.stringify({version:1,entries:ids.map((id,i)=>({id,title:titles[i],visitedAt:Date.now()-i*1000}))})}});await page.goto('/');return {runtime,errors};}
async function storedOrder(page:Page){return page.evaluate(key=>JSON.parse(localStorage.getItem(key)??'{}').customOrder,orderKey);}
const rows=(page:Page)=>page.locator('.wiki-recent-note');
test('custom keeps the selected lower page in place, supports drag and persists after reload',async({page},info)=>{
 const {runtime,errors}=await boot(page);const mode=page.locator('.wiki-workspace-mode select');await mode.selectOption('custom');await expect(rows(page)).toHaveText(titles);
 await rows(page).nth(1).click();await expect(page.locator('.cm-content')).toContainText('Decision: keep review fixtures deterministic.');await expect(rows(page).nth(1)).toHaveAttribute('aria-current','page');await expect(rows(page)).toHaveText(titles);
 const source=page.locator('.wiki-recent-note-row').nth(2),grip=source.locator('.wiki-recent-note-grip');const b=(await grip.boundingBox())!,target=(await page.locator('.wiki-recent-note-row').first().boundingBox())!;const sb=(await source.boundingBox())!;
 await page.mouse.move(b.x+b.width/2,b.y+b.height/2);await page.mouse.down();await page.mouse.move(b.x+b.width/2,target.y+5,{steps:8});
 await expect(source).toHaveClass(/is-dragging/);expect((await source.boundingBox())!.y).toBeLessThan(sb.y-20);expect(await storedOrder(page)).toEqual(ids);await page.screenshot({path:info.outputPath('drag.png')});await page.mouse.up();
 await expect(rows(page)).toHaveText([titles[2],titles[0],titles[1]]);await expect.poll(()=>storedOrder(page)).toEqual([ids[2],ids[0],ids[1]]);await expect(rows(page).last()).toHaveAttribute('aria-current','page');
 await page.reload();await expect(mode).toHaveValue('custom');await expect(rows(page)).toHaveText([titles[2],titles[0],titles[1]]);
 const firstGrip=page.locator('.wiki-recent-note-grip').first();await firstGrip.focus();await page.keyboard.press('ArrowDown');await expect(rows(page)).toHaveText([titles[0],titles[2],titles[1]]);await expect(page.locator('.note-tabs-list')).toHaveCount(0);
 await expect(page.getByRole('button',{name:'Drag to reorder Weekly fixture recap',exact:true})).toBeFocused();
 await page.keyboard.press('ArrowUp');await expect(rows(page)).toHaveText([titles[2],titles[0],titles[1]]);await page.keyboard.press('ArrowDown');await expect(rows(page)).toHaveText([titles[0],titles[2],titles[1]]);
 await openWikiNote(page,'History semantics');await mode.selectOption('custom');await expect(rows(page)).toHaveText([titles[0],titles[2],titles[1],'History semantics']);
 await page.mouse.move(500,700);await page.screenshot({path:info.outputPath('settled.png')});
 expect(runtime.calls().filter(c=>['update_page','page_move','knowledge_folder_create'].includes(c.command))).toHaveLength(0);expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
test('custom cancels drag without saving or opening pages',async({page})=>{
 await boot(page);await page.locator('.wiki-workspace-mode select').selectOption('custom');
 for(const cancel of ['Escape','outside','pointercancel','blur','resize','scroll']){
 const grip=page.locator('.wiki-recent-note-grip').first();const b=(await grip.boundingBox())!;const start=await storedOrder(page);
 await grip.evaluate(el=>el.addEventListener('pointerdown',e=>el.setAttribute('data-test-pointer-id',String((e as PointerEvent).pointerId)),{once:true}));
 await page.mouse.move(b.x+b.width/2,b.y+b.height/2);await page.mouse.down();await page.mouse.move(b.x+b.width/2,b.y+60,{steps:4});await expect(page.locator('.is-dragging')).toHaveCount(1);
 if(cancel==='Escape')await page.keyboard.press('Escape');else if(cancel==='outside')await page.mouse.move(1100,b.y+60);else if(cancel==='resize')await page.setViewportSize({width:1270,height:840});else await page.evaluate(({kind,pointerId})=>{if(kind==='pointercancel'){window.dispatchEvent(new PointerEvent(kind,{pointerId}));}else window.dispatchEvent(new Event(kind));},{kind:cancel,pointerId:Number(await grip.getAttribute('data-test-pointer-id'))});
 await page.mouse.up();await expect(page.locator('.is-dragging')).toHaveCount(0);expect(await storedOrder(page),cancel).toEqual(start);await expect(rows(page)).toHaveText(titles);await expect(page.locator('.note-tabs-list')).toHaveCount(0);
 }
});
