// SPDX-License-Identifier: AGPL-3.0-only
import {expect,test,type Page,type Locator} from '@playwright/test';
import {resources} from '../src/i18n/resources';
import {createReviewDecisionFixture} from './fixtures/reviewDecisions';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {openWikiNote} from './helpers/wikiWorkspace';
const frame=(p:Page,id='primary')=>p.locator(`[data-note-group-id="${id}"]`);
async function geometry(group:Locator){return group.evaluate(el=>{
 const rect=(s:string)=>{const r=el.querySelector(s)!.getBoundingClientRect();return{x:r.x,y:r.y,right:r.right,bottom:r.bottom,width:r.width,height:r.height};};
 const targets=['.note-inspector-toggle','.note-tabs [role="tab"][aria-selected="true"]','.note-tab-create','.note-tab-close','.note-inspector-tabs [role="tab"]:first-child','.note-inspector-tabs [role="tab"]:last-child','.page-info-drawer-workspace-toggle','.page-info-drawer-close'];
 return{group: {width:el.getBoundingClientRect().width,x:el.getBoundingClientRect().x,right:el.getBoundingClientRect().right}, compact:!!el.querySelector('.page-info-drawer-group-full'), expanded:!!el.querySelector('.page-info-drawer-expanded-group'), canvas:!!el.querySelector('.note-inspector-panel--canvas'), panel:rect('.page-info-drawer'), content:rect('.note-group-content'), inspectorContent:rect('.page-info-drawer-content'), contentPadTop:Number.parseFloat(getComputedStyle(el.querySelector('.page-info-drawer-content')!).paddingTop||'0'), groupOverlay:!!el.querySelector('.page-info-drawer-group-overlay-panel'), panelBorder:getComputedStyle(el.querySelector('.page-info-drawer')!).borderLeftWidth, headerBorder:getComputedStyle(el.querySelector('.page-info-drawer-header')!).borderLeftWidth, noteHeader:rect('.note-group-header'),inspectorHeader:rect('.page-info-drawer-header'),noteTabs:rect('.note-group-tabs'),noteText:rect('.note-tabs [role="tab"][aria-selected="true"] span'),infoText:rect('.note-inspector-tabs [role="tab"]:first-child span'),overflow:document.documentElement.scrollWidth>innerWidth,targets:targets.filter(s=>el.querySelector(s)).map(s=>{const node=el.querySelector(s)!;const r=node.getBoundingClientRect();return{selector:s,width:r.width,height:r.height,hit:node.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2))};})};
 });}
async function aligned(group:Locator){const g=await geometry(group);expect(Math.abs(g.noteHeader.y-g.inspectorHeader.y)).toBeLessThanOrEqual(1);expect(g.inspectorHeader.height, "compact mind-map controls use a second row while view tabs retain the page baseline").toBe(g.canvas && g.panel.width <= 279 ? 96 : 52);expect(Math.abs(g.content.y-g.inspectorContent.y)).toBeLessThanOrEqual(1);expect(g.inspectorHeader.bottom, 'inspector header clears its content padding').toBeLessThanOrEqual(g.inspectorContent.y+g.contentPadTop+1);expect(g.noteTabs.right).toBeLessThanOrEqual(g.inspectorHeader.x+1);expect(Math.abs((g.noteText.y+g.noteText.height/2)-(g.infoText.y+g.infoText.height/2))).toBeLessThanOrEqual(2);expect(g.overflow).toBe(false);expect(g.compact, "never automatically replaces page with inspector").toBe(false);if(g.expanded){expect(Math.abs(g.panel.x-g.group.x)).toBeLessThanOrEqual(1);expect(Math.abs(g.panel.width-g.group.width)).toBeLessThanOrEqual(1);}else{expect(Math.abs(g.panel.x-g.inspectorHeader.x), 'sidebar header and content left edge').toBeLessThanOrEqual(1);expect(Math.abs(g.panel.width-g.inspectorHeader.width), 'sidebar header and content width').toBeLessThanOrEqual(1);expect(g.content.right, 'page reflows before inspector').toBeLessThanOrEqual(g.panel.x+1);expect(g.panel.width).toBeGreaterThanOrEqual(140);expect(g.content.width, 'page stays visibly separate').toBeGreaterThanOrEqual(200);expect(g.groupOverlay?g.panelBorder:g.headerBorder).toBe('1px');}for(const target of g.targets){expect(target.hit,target.selector).toBe(true);expect(target.width,target.selector).toBeGreaterThanOrEqual(24);}}
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
 test('page and inspector share aligned bounds and remain independently usable',async({page},info)=>{
 const t=resources[scene.locale].translation,errors=collectBrowserErrors(page);
 await installTauriMock(page,{fixture:createReviewDecisionFixture('wiki-folders'),locale:scene.locale,rawActions:[],localStorage:{'wenlan-theme':scene.theme}});
 await page.goto('/');await openWikiNote(page,'Fixture architecture');await openWikiNote(page,'Why fixtures stay deterministic');
 const left=frame(page),right=frame(page,'secondary');
 await left.getByRole('tab',{name:'Why fixtures stay deterministic',exact:true}).click({button:'right'});await page.getByRole('menuitem',{name:t.pages.groups.moveToRight,exact:true}).click();
 for(const group of [left,right])await group.locator('.note-inspector-toggle').click();
 await expect(right.locator('.page-info-drawer')).toBeVisible();
 await page.screenshot({path:info.outputPath('both-info.png')});
 await info.attach('both-info-geometry',{body:JSON.stringify(await Promise.all([geometry(left),geometry(right)])),contentType:'application/json'});
 await expect(left.locator('.note-inspector-toggle')).toBeVisible();
 await expect(left.locator('.note-inspector-toggle')).toHaveAttribute('aria-expanded','true');
 // Opening, expanding and toggling Info reflows the groups and can fold the rail, so the geometry polls until it settles.
 await expect(async()=>{await aligned(left);await aligned(right);}).toPass({timeout:10_000});
 await right.locator('.note-inspector-tabs [role="tab"]').nth(1).click();
 await expect(right.locator('.page-canvas')).toBeVisible();
 await right.locator('.page-info-drawer-workspace-toggle').click();
 await expect(right.locator('.page-info-drawer-expanded-group')).toBeVisible();
 await expect(async()=>{await aligned(left);await aligned(right);}).toPass({timeout:10_000});
 await expect(left.locator('.note-inspector-tabs [role="tab"]').first()).toHaveAttribute('aria-selected','true');
 await page.screenshot({path:info.outputPath('right-expanded.png')});
 await right.locator('.page-info-drawer-workspace-toggle').click();await expect(async()=>{await aligned(right);}).toPass({timeout:10_000});
 await right.locator('.note-inspector-toggle').click();
 await expect(right.locator('.page-info-drawer')).toHaveCount(0);
 await expect(right.locator('.note-inspector-toggle')).toHaveAttribute('aria-expanded','false');
 await expect(left.locator('.page-info-drawer')).toBeVisible();
 await right.locator('.note-inspector-toggle').click();
 await expect(right.locator('.note-inspector-toggle')).toHaveAttribute('aria-expanded','true');
 await right.locator('.note-inspector-tabs [role="tab"]').nth(1).click();
 await expect(async()=>{await aligned(left);await aligned(right);}).toPass({timeout:10_000});
 await page.screenshot({path:info.outputPath('toggle-restored.png')});
 if(scene.width===1680){
  for(const width of [375,2560,1680]){
   await page.setViewportSize({width,height:960});
   await expect.poll(async()=>{const g=await geometry(right);return !g.compact&&g.panel.width>=140&&g.content.width>=200&&Math.abs(g.panel.x-g.inspectorHeader.x)<=1&&Math.abs(g.content.right-g.panel.x)<=1;}).toBe(true);
   // The resize poll above only covers the right group; the left one can still be settling, so its hit test polls too.
   await expect(async()=>{await aligned(left);await aligned(right);}).toPass({timeout:10_000});
   await expect(left.locator('.note-inspector-tabs [role="tab"]').first()).toHaveAttribute('aria-selected','true');
   await expect(right.locator('.note-inspector-tabs [role="tab"]').last()).toHaveAttribute('aria-selected','true');
   await page.screenshot({path:info.outputPath(`resized-${width}.png`)});
  }
 }
 if(!scene.touch){
  const tab=right.getByRole('tab',{name:'Why fixtures stay deterministic',exact:true});const from=(await tab.boundingBox())!,target=(await left.locator('.page-info-drawer').boundingBox())!;
  await page.mouse.move(from.x+from.width/2,from.y+from.height/2);await page.mouse.down();await page.mouse.move(target.x+target.width-15,target.y+150,{steps:14});
  await expect(page.locator('.note-tab-split-preview[data-kind="transfer"]')).toBeVisible();await page.keyboard.press('Escape');await page.mouse.up();
  await expect(tab).toBeVisible();await expect(page.locator('[data-note-group-id]')).toHaveCount(2);
 }
 await right.locator('.page-info-drawer-close').click();await expect(right.locator('.page-info-drawer')).toHaveCount(0);await expect(right.locator('.note-inspector-toggle')).toBeVisible();
 await expect(left.locator('.page-info-drawer')).toBeVisible();await left.locator('.page-info-drawer-close').click();
 await expect(left.locator('.note-inspector-toggle')).toBeVisible();await expect(left.getByRole('tab',{name:'Fixture architecture',exact:true})).toBeVisible();await expect(right.getByRole('tab',{name:'Why fixtures stay deterministic',exact:true})).toBeVisible();
 expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
 });
});
