// SPDX-License-Identifier: AGPL-3.0-only
import {test,expect,type Page} from '@playwright/test';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {openWikiNote} from './helpers/wikiWorkspace';
async function boot(page: Page){await page.setViewportSize({width:1440,height:900});const errors=collectBrowserErrors(page);const control=await installTauriMock(page,{locale:'en',rawActions:[]});await page.goto('/');await openWikiNote(page,'Fixture architecture');return {errors,control};}
test('Info is selected from the page toolbar without a nested inspector tab bar',async({page},info)=>{
 const {errors}=await boot(page);const infoTool=page.getByRole('button',{name:'Info',exact:true});const mapTool=page.getByRole('button',{name:'Mind map',exact:true});
 await expect(infoTool).toBeVisible();await expect(mapTool).toBeVisible();await infoTool.click();
 await expect(infoTool).toHaveAttribute('aria-pressed','true');await expect(page.locator('.note-inspector-tabs')).toHaveCount(0);
 await expect(page.getByRole('group',{name:'Connections',exact:true})).toBeVisible();await expect(page.getByRole('button',{name:'Open in graph',exact:true})).toBeVisible();await expect(page.locator('.knowledge-context-section')).toHaveCount(0);await expect(page.locator('.note-info-summary')).toBeVisible();
 await page.screenshot({path:info.outputPath('unified-info.png')});expect(errors.pageErrors).toEqual([]);
});
test('expanded inspector and its page-specific state restore after another Wiki page',async({page},info)=>{
 const {errors,control}=await boot(page);const infoTool=page.getByRole('button',{name:'Info',exact:true});const mapTool=page.getByRole('button',{name:'Mind map',exact:true});
 await mapTool.click();await expect(page.locator('.page-canvas-surface')).toBeVisible();await page.getByRole('button',{name:'Expand workspace',exact:true}).click();
 const pane=page.locator('.page-info-drawer-expanded-workspace');await expect(pane).toBeVisible();await infoTool.click();await expect(pane).toBeVisible();await expect(page.locator('.page-detail-document')).toBeHidden();await expect(page.getByRole('group',{name:'Connections',exact:true})).toBeVisible();
 await openWikiNote(page,'Why fixtures stay deterministic');await expect(page.getByRole('button',{name:'Info',exact:true})).toHaveAttribute('aria-expanded','false');await expect(pane).toHaveCount(0);
 await openWikiNote(page,'Fixture architecture');await expect(infoTool).toHaveAttribute('aria-pressed','true');await expect(pane).toBeVisible();
 await mapTool.click();await expect(page.getByRole('region',{name:'Canvas for Fixture architecture',exact:true})).toBeVisible();await page.screenshot({path:info.outputPath('restored-map.png')});
 await page.getByRole('button',{name:'Return to sidebar',exact:true}).click();await expect(pane).toHaveCount(0);await expect(page.locator('.page-info-drawer-group-overlay-panel')).toBeVisible();await expect(page.locator('.page-detail-document')).toBeVisible();
 expect(control.calls().filter(c=>/^(update_page|create_page_map_node|patch_page_map_node|delete_page_map_node)$/.test(c.command))).toHaveLength(0);expect(errors.pageErrors).toEqual([]);
});
