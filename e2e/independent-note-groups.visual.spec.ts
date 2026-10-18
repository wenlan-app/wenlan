// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from '@playwright/test';
import { openWikiNote } from './helpers/wikiWorkspace';
import { createReviewDecisionFixture } from './fixtures/reviewDecisions';
import { installTauriMock, collectBrowserErrors } from './tauriMock';
import { resources } from '../src/i18n/resources';
for(const locale of ['en'] as const) for(const theme of ['light']) for(const width of [1440,375]) {
 test(`independent groups ${locale} ${theme} ${width}`,async({page},info)=>{
  const copy=resources[locale].translation;
  await page.setViewportSize({width,height:960});
  const errors=collectBrowserErrors(page);
  await installTauriMock(page,{locale,rawActions:[],fixture:createReviewDecisionFixture('wiki-folders'),localStorage:{'wenlan-theme':theme}});
  await page.goto('/');
  await openWikiNote(page,'Fixture architecture');
  await openWikiNote(page,'Why fixtures stay deterministic');
  const left=page.locator('[data-note-group-id="primary"]'),right=page.locator('[data-note-group-id="secondary"]');
  await left.getByRole('tab',{selected:true}).click({button:'right'});
  await page.getByRole('menuitem',{name:copy.pages.groups.moveToRight,exact:true}).click();
  await expect(right.locator('.page-detail')).toBeVisible();
  await expect(left.locator('.page-detail')).toBeVisible();
  expect(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)).toBe(true);
  const boxes=await Promise.all([left.boundingBox(),right.boundingBox()]);
  if(width===375) expect(boxes[1]!.y).toBeGreaterThanOrEqual(boxes[0]!.y+boxes[0]!.height-1);
  else expect(boxes[1]!.x).toBeGreaterThanOrEqual(boxes[0]!.x+boxes[0]!.width-1);
  await page.screenshot({path:info.outputPath('01-notes.png'),fullPage:true});
  await left.getByRole('button',{name:copy.pageInspector.open,exact:true}).click();
  await right.getByRole('button',{name:copy.pageInspector.open,exact:true}).click();
  await right.getByRole('tab',{name:copy.pageCanvas.tabCanvas,exact:true}).click();
  for(const frame of [left,right]) {
   const bounds=await frame.boundingBox(),panel=await frame.getByRole('complementary').boundingBox();
   expect(panel!.x).toBeGreaterThanOrEqual(bounds!.x-1);
   expect(panel!.x+panel!.width).toBeLessThanOrEqual(bounds!.x+bounds!.width+1);
   expect(panel!.y).toBeGreaterThanOrEqual(bounds!.y-1);
   expect(panel!.y+panel!.height).toBeLessThanOrEqual(bounds!.y+bounds!.height+1);
  }
  await page.screenshot({path:info.outputPath('02-own-inspectors.png'),fullPage:true});
  await right.getByRole('button',{name:copy.pageCanvas.expandWorkspace,exact:true}).click();
  await expect(left.getByRole('tab',{name:copy.pageInspector.info,exact:true})).toBeVisible();
  await expect(right.locator('.page-info-drawer-expanded-group')).toBeVisible();
  await page.screenshot({path:info.outputPath('03-own-workspace.png'),fullPage:true});
  expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
 });
}
