// SPDX-License-Identifier: AGPL-3.0-only
import {expect,test,type Page} from '@playwright/test';
import {createReviewDecisionFixture} from './fixtures/reviewDecisions';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {openWikiNote} from './helpers/wikiWorkspace';
import {resources} from '../src/i18n/resources';
const frame=(page:Page,id='primary')=>page.locator(`[data-note-group-id="${id}"]`);
const inspector=(page:Page)=>page.locator('.page-info-drawer');
const scenes=[
 {width:1680,theme:'dark',locale:'en',touch:false},
 {width:1440,theme:'dark',locale:'en',touch:false},
 {width:1280,theme:'dark',locale:'en',touch:false},
 {width:1280,theme:'light',locale:'zh-Hant',touch:false},
 {width:375,theme:'light',locale:'en',touch:false},
 {width:375,theme:'dark',locale:'zh-Hans',touch:false},
 {width:2560,theme:'light',locale:'en',touch:false},
 {width:375,theme:'dark',locale:'zh-Hant',touch:true},
] as const;
for(const scene of scenes)test.describe(`${scene.width} ${scene.theme} ${scene.locale}${scene.touch?' touch':''}`,()=>{
 test.use({viewport:{width:scene.width,height:960},hasTouch:scene.touch});
 test('one shared inspector follows the active Wiki page and preserves its controls',async({page},info)=>{
  const errors=collectBrowserErrors(page);
  // Seed the split using the normal desktop navigation, then verify the resulting narrow layout.
  if(scene.width<640) await page.setViewportSize({width:1280,height:960});
  const copy=resources[scene.locale].translation;
  await installTauriMock(page,{fixture:createReviewDecisionFixture('wiki-folders'),locale:scene.locale,rawActions:[],localStorage:{'wenlan-theme':scene.theme}});
  await page.goto('/');await openWikiNote(page,'Fixture architecture');await openWikiNote(page,'Why fixtures stay deterministic');
  const left=frame(page),right=frame(page,'secondary');
  await left.getByRole('tab',{name:'Why fixtures stay deterministic',exact:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:copy.pages.groups.moveToRight,exact:true}).click();
  if(scene.width<640) await page.setViewportSize({width:scene.width,height:960});
  await expect(left.locator('.page-detail')).toBeVisible();await expect(right.locator('.page-detail')).toBeVisible();
  await right.getByRole('tab',{selected:true}).click();
  const infoTool=page.getByRole('button',{name:copy.pageInspector.info,exact:true});
  const mapTool=page.getByRole('button',{name:copy.pageCanvas.tabCanvas,exact:true});
  await expect(infoTool).toBeVisible();await expect(mapTool).toBeVisible();
  await infoTool.click();await expect(inspector(page)).toHaveCount(1);await expect(infoTool).toHaveAttribute('aria-pressed','true');
  const panelBox=(await inspector(page).boundingBox())!;
  const shellBox=(await page.locator('.memory-shell').boundingBox())!;
  expect(panelBox.x).toBeGreaterThanOrEqual(shellBox.x-1);
  expect(panelBox.x+panelBox.width).toBeLessThanOrEqual(shellBox.x+shellBox.width+1);
  expect(panelBox.width).toBeGreaterThanOrEqual(320);
  await page.screenshot({path:info.outputPath('shared-info.png'),fullPage:true});

  // The other group has its own page state; focusing its page does not leave
  // the right group's inspector open in a second pane.
  await left.getByRole('tab',{name:'Fixture architecture',exact:true}).click();
  await expect(inspector(page)).toHaveCount(0);
  await expect(infoTool).toHaveAttribute('aria-expanded','false');
  await right.getByRole('tab',{name:'Why fixtures stay deterministic',exact:true}).click();
  await expect(infoTool).toHaveAttribute('aria-pressed','true');
  await mapTool.click();
  await expect(page.getByRole('region',{name:copy.pageCanvas.regionLabel.replace('{{title}}','Why fixtures stay deterministic'),exact:true})).toBeVisible();
  await expect(inspector(page)).toHaveCount(1);
  await expect(page.getByRole('button',{name:'PDF',exact:true})).toHaveCount(0);
  await page.screenshot({path:info.outputPath('shared-map.png'),fullPage:true});

  if(scene.width>=1100){
   await page.getByRole('button',{name:copy.pageCanvas.expandWorkspace,exact:true}).click();
   await expect(page.locator('.page-info-drawer-expanded-workspace, .page-info-drawer-expanded-group')).toBeVisible();
   await page.screenshot({path:info.outputPath('expanded-map.png'),fullPage:true});
   await page.getByRole('button',{name:copy.pageCanvas.restoreSidebar,exact:true}).click();
  }
  await mapTool.click();await expect(inspector(page)).toHaveCount(0);
  await expect(mapTool).toHaveAttribute('aria-expanded','false');
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
 });
});
