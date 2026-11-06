// SPDX-License-Identifier: AGPL-3.0-only
import {test,expect,type Page} from '@playwright/test';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {openWikiNote} from './helpers/wikiWorkspace';
export async function infoFixtures(page:Page){
 await page.addInitScript(()=>{const a=window.__TAURI_INTERNALS__!;const invoke=a.invoke;a.invoke=async(c,args)=>{
  if(c==='get_page_links')return {outbound:[{label:'Why fixtures stay deterministic',target_page_id:'page-errors'}],inbound:[{source_page_id:'page-keyboard',label:'Weekly fixture recap'},{source_page_id:'page-keyboard',label:'Weekly fixture recap'}]};
  return invoke(c,args);
 };});
}
async function boot(page:Page,width=1440){await page.setViewportSize({width,height:900});const errors=collectBrowserErrors(page);const controller=await installTauriMock(page,{locale:'en',rawActions:[]});await infoFixtures(page);await page.goto('/');await openWikiNote(page,'Fixture architecture');await expect(page.getByRole('textbox',{name:'Page editor',exact:true})).toBeVisible();return {errors,controller};}
for(const tool of ['Info','Mind map'])test(`keeps ${tool} state on its Wiki page without leaking to another page`,async({page},info)=>{
 const {errors}=await boot(page);const toggle=page.getByRole('button',{name:tool,exact:true});await toggle.click();
 await expect(toggle).toHaveAttribute('aria-pressed','true');
 if(tool==='Mind map')await expect(page.getByRole('region',{name:'Canvas for Fixture architecture',exact:true})).toBeVisible();
 await openWikiNote(page,'Why fixtures stay deterministic');
 await expect(page.getByRole('button',{name:'Info',exact:true})).toHaveAttribute('aria-expanded','false');
 await expect(page.locator('.page-info-drawer')).toHaveCount(0);
 await openWikiNote(page,'Fixture architecture');
 await expect(toggle).toHaveAttribute('aria-pressed','true');
 if(tool==='Mind map')await expect(page.getByRole('region',{name:'Canvas for Fixture architecture',exact:true})).toBeVisible();
 await page.screenshot({path:info.outputPath('restored.png')});expect(errors.pageErrors).toEqual([]);
});
for(const width of [1440,375])test(`an Info connection opens an unseen Wiki page with its inspector closed ${width}`,async({page},info)=>{
 const {errors}=await boot(page,width);await page.getByRole('button',{name:'Info',exact:true}).click();const pane=page.locator('.page-info-drawer');await expect(pane.getByRole('button',{name:'Weekly fixture recap',exact:true})).toHaveCount(1);await pane.getByRole('button',{name:'Why fixtures stay deterministic',exact:true}).click();await expect(page.locator('.note-tabs [role=tab][aria-selected=true]')).toContainText('Why fixtures stay deterministic');await expect(page.getByRole('button',{name:'Info',exact:true})).toHaveAttribute('aria-expanded','false');await expect(page.locator('.page-info-drawer')).toHaveCount(0);await page.screenshot({path:info.outputPath('linked-page-default.png')});expect(errors.pageErrors).toEqual([]);
});
test('failed save retains active page and Info pane',async({page},info)=>{
 const {errors,controller}=await boot(page);controller.failNext('update_page','Synthetic save failure',20);const editor=page.getByRole('textbox',{name:'Page editor',exact:true});await editor.click();await page.keyboard.press('End');await page.keyboard.type(' protected edit');await page.getByRole('button',{name:'Info',exact:true}).click();await page.locator('.page-info-drawer').getByRole('button',{name:'Why fixtures stay deterministic',exact:true}).click();await expect(editor).toContainText('protected edit');await expect(page.getByRole('button',{name:'Info',exact:true})).toHaveAttribute('aria-pressed','true');await expect(page.locator('.note-tabs [role=tab][aria-selected=true]')).toContainText('Fixture architecture');await page.screenshot({path:info.outputPath('blocked.png')});expect(errors.pageErrors).toEqual([]);
});
test('Info graph link opens an unseen target with its inspector closed on narrow screen',async({page},info)=>{
 const {errors}=await boot(page,375);await page.getByRole('button',{name:'Info',exact:true}).click();await page.getByRole('group',{name:'Connections',exact:true}).getByRole('button',{name:'Open page: Ada Lovelace',exact:true}).click();await expect(page.locator('.note-tabs [role=tab][aria-selected=true]')).toContainText('Ada Lovelace');await expect(page.getByRole('button',{name:'Info',exact:true})).toHaveAttribute('aria-expanded','false');await expect(page.locator('.page-info-drawer')).toHaveCount(0);await page.screenshot({path:info.outputPath('connections-link-page-default.png')});expect(errors.pageErrors).toEqual([]);
});
