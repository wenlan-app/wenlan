import {test,expect,type Page} from '@playwright/test';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {openWikiNote} from './helpers/wikiWorkspace';
const g=(p:Page,id='primary')=>p.locator(`[data-note-group-id="${id}"]`);
async function start(page:Page){ const errors=collectBrowserErrors(page);const runtime=await installTauriMock(page,{locale:'en',rawActions:[]});await page.setViewportSize({width:1440,height:960});await page.goto('/');await openWikiNote(page,'Fixture architecture');return {errors,runtime}; }
async function create(page:Page,id='primary'){await g(page,id).locator('.note-tab-create').click();await expect(g(page,id).getByRole('textbox',{name:'Title',exact:true})).toBeVisible();}
test('new page autosaves while typing, finalizes on switch and stays editable',async({page},info)=>{
 const {errors,runtime}=await start(page);await create(page);const f=g(page);await expect(f.getByRole('button',{name:'Publish',exact:true})).toHaveCount(0);await expect(f.getByRole('combobox')).toHaveCount(0);
 await f.getByRole('textbox',{name:'Title',exact:true}).fill('A normal page');await f.getByRole('textbox',{name:'Content',exact:true}).fill('Keep this first paragraph.');
 await expect.poll(()=>runtime.calls().filter(c=>c.command==='create_page_draft').length).toBe(1);expect(runtime.calls().filter(c=>c.command==='publish_page_draft')).toHaveLength(0);
 await expect(f.getByRole('textbox',{name:'Content',exact:true})).toBeFocused();await page.screenshot({path:info.outputPath('writing.png')});
 await f.getByRole('tab',{name:'Fixture architecture',exact:true}).click();await expect(f.getByRole('heading',{name:'Fixture architecture',exact:true})).toBeVisible();
 await f.getByRole('tab',{name:'A normal page',exact:true}).click();await expect(f.locator('.page-detail')).toContainText('Keep this first paragraph.');await expect(f.locator('.page-draft-editor')).toHaveCount(0);
 await f.getByRole('textbox',{name:'Page editor',exact:true}).fill('Keep this first paragraph. More writing.');await expect.poll(()=>runtime.calls().filter(c=>c.command==='update_page').length).toBeGreaterThan(0);await page.screenshot({path:info.outputPath('saved-editable.png')});
 expect(runtime.calls().filter(c=>c.command==='publish_page_draft')).toHaveLength(1);expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
test('title-only and body-only pages finalize across both groups and close',async({page},info)=>{
 const {errors,runtime}=await start(page);await create(page);await g(page).getByRole('textbox',{name:'Title',exact:true}).fill('Title only');
 await g(page).getByRole('tab',{name:'Title only',exact:true}).click({button:'right'});await page.getByRole('menuitem',{name:'Move to right group',exact:true}).click();const right=g(page,'secondary');await expect(right.locator('.page-detail')).toContainText('Title only');
 await create(page,'secondary');await right.getByRole('textbox',{name:'Content',exact:true}).fill('First line becomes the name\n\nPreserve the rest.');await right.getByRole('tab',{name:'Title only',exact:true}).click();
 await right.getByRole('tab',{name:'First line becomes the name',exact:true}).click();await expect(right.locator('.page-detail')).toContainText('Preserve the rest.');
 await right.getByRole('tab',{name:'First line becomes the name',exact:true}).click({button:'right'});await page.getByRole('menuitem',{name:'Move to central group',exact:true}).click();await expect(g(page).locator('.page-detail')).toContainText('Preserve the rest.');await page.screenshot({path:info.outputPath('two-groups.png')});
 await create(page);await g(page).getByRole('textbox',{name:'Title',exact:true}).fill('Close saves me');await g(page).locator('.note-tab[data-active="true"] .note-tab-close').click();await expect(g(page).getByRole('tab',{name:'Close saves me',exact:true})).toHaveCount(0);
 expect(runtime.calls().filter(c=>c.command==='publish_page_draft')).toHaveLength(3);expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
});
test('failed automatic save keeps text and retry returns an editable page',async({page},info)=>{
 const {errors,runtime}=await start(page);await create(page);await g(page).getByRole('textbox',{name:'Title',exact:true}).fill('Recovered page');await g(page).getByRole('textbox',{name:'Content',exact:true}).fill('Never lose this text.');runtime.failNext('publish_page_draft','simulated interruption');
 await g(page).getByRole('tab',{name:'Fixture architecture',exact:true}).click();await expect(g(page).getByRole('alert')).toContainText('Your text is still here');await expect(g(page).getByRole('textbox',{name:'Content',exact:true})).toHaveValue('Never lose this text.');await page.screenshot({path:info.outputPath('save-error.png')});
 await g(page).getByRole('button',{name:'Retry save',exact:true}).click();await expect(g(page).locator('.page-detail')).toContainText('Never lose this text.');await expect(g(page).getByRole('textbox',{name:'Page editor',exact:true})).toBeEditable();expect(errors.pageErrors).toEqual([]);
});
test('explicit title conflict preserves page; rename resolves; blank tabs do not persist',async({page},info)=>{
 const {errors,runtime}=await start(page);await create(page);await g(page).locator('.note-tab[data-active="true"] .note-tab-close').click();expect(runtime.calls().filter(c=>c.command==='create_page_draft')).toHaveLength(0);
 await create(page);await g(page).getByRole('textbox',{name:'Title',exact:true}).fill('Conflict page');await g(page).getByRole('textbox',{name:'Content',exact:true}).fill('Original unchanged.');await g(page).getByRole('tab',{name:'Fixture architecture',exact:true}).click();await create(page);await g(page).getByRole('textbox',{name:'Title',exact:true}).fill('Conflict page');await g(page).getByRole('textbox',{name:'Content',exact:true}).fill('Different article stays safe.');await g(page).locator('.note-tab-create').click();await expect(g(page).getByRole('alert')).toContainText('already');await expect(g(page).getByRole('textbox',{name:'Content',exact:true})).toHaveValue('Different article stays safe.');
 await g(page).getByRole('button',{name:'Rename page',exact:true}).click();await g(page).getByRole('textbox',{name:'Title',exact:true}).fill('A separate architecture page');await g(page).locator('.note-tab-create').click();await expect(g(page).getByRole('textbox',{name:'Title',exact:true})).toHaveValue('');await g(page).getByRole('tab',{name:'A separate architecture page',exact:true}).click();await expect(g(page).locator('.page-detail')).toContainText('Different article stays safe.');await page.screenshot({path:info.outputPath('conflict-recovered.png')});expect(errors.pageErrors).toEqual([]);
});
