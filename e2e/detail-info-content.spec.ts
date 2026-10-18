// SPDX-License-Identifier: AGPL-3.0-only
import {test,expect} from '@playwright/test';
import {installTauriMock,collectBrowserErrors} from './tauriMock';
import {createSpacesNavigationFixture} from './fixtures/spacesNavigation';
import {resources} from '../src/i18n/resources';
import {writeFileSync} from 'node:fs';

const content='週末散步前先準備水瓶與雨具。雨天改走展覽與書店路線，保留回程的彈性。';
for(const locale of ['en','zh-Hant','zh-Hans'] as const)for(const theme of ['light','dark'])for(const width of [1440,375]){
 test(`Info content ${locale} ${theme} ${width}`,async({page},info)=>{
  const t=resources[locale].translation,errors=collectBrowserErrors(page),base=createSpacesNavigationFixture();
  const fixture={...base,memories:base.memories.map((m,i)=>i===0?{...m,content,confirmed:false,stability:'new' as const,structured_fields:JSON.stringify({claim:content,context:'出門前查看天氣；保留一條室內備案。'}),last_modified:Math.floor(Date.now()/1000)-7200}:m),entityDetails:base.entityDetails.map(d=>({...d,entity:{...d.entity,updated_at:Math.floor(Date.now()/1000)-7200}}))};
  await page.setViewportSize({width,height:900});
  const runtime=await installTauriMock(page,{fixture,locale,rawActions:[],localStorage:{'wenlan-theme':theme,'wenlan-navigation-v1':JSON.stringify({version:1,visible:['pages','spaces','graph','sources','memories','entities'],sidebar:{visible:true,mode:'icons',width:240}})}});
  await page.goto('/');await expect(page.locator('.memory-shell')).toBeVisible();
  await page.evaluate(()=>{const bridge=window.__TAURI_INTERNALS__!,invoke=bridge.invoke;bridge.invoke=async(command,args,options)=>{
   if(command==='get_enrichment_status')return {source_id:'memory-0',summary:'raw',steps:[{step:'summarize',status:'pending'}]};
   if(command==='get_memory_revisions')return {current_source_id:'memory-0',chain_depth:1,entries:[{source_id:'memory-0',title:'週末散步準備',content_preview:'週末散步前先準備水瓶與雨具。雨天改走展覽與書店路線，保留回程的彈性。',delta_summary:'加入雨天備案。',depth:0,last_modified:Date.now()/1000-7200,source_agent:'codex'},{source_id:'memory-1',title:'先前的準備方式',content_preview:'帶水瓶，沿河散步。',delta_summary:'保留原始路線。',depth:1,last_modified:Date.now()/1000-86400,source_agent:'codex'}]};
   return invoke(command,args,options);
  };});
  const navigate=async(name:string)=>{
   const nav=page.getByRole('navigation',{name:t.sidebar.navigation});
   if(!await nav.isVisible())await page.getByRole('button',{name:/^(Show sidebar|顯示側邊欄|显示侧边栏)$/}).click();
   await nav.getByRole('button',{name,exact:true}).click();
  };
  await navigate(t.sidebar.memories);
  await page.getByRole('article',{name:'Fixture architecture',exact:true}).getByRole('button',{name:t.memoryList.openMemory,exact:true}).click();
  const editor=page.locator('.memory-document-editor .cm-content');await expect(editor).toContainText(content);
  const openInfo=async()=>{const toggle=page.getByRole('button',{name:t.knowledgeContext.memoryPanel,exact:true});if(await toggle.getAttribute('aria-expanded')!=='true')await toggle.click();};
  await openInfo();const pane=page.locator('.page-info-drawer:visible');await expect(pane).toBeVisible();
  await expect(pane.getByText(content,{exact:true})).toHaveCount(0);
  await expect(pane.getByText('出門前查看天氣；保留一條室內備案。',{exact:true})).toBeVisible();
  await expect(pane.getByText('raw',{exact:true})).toHaveCount(0);await expect(pane.getByText(/depth \d/)).toHaveCount(0);
  const confirm=pane.getByRole('button',{name:t.infoActions.confirmContent,exact:true});await expect(confirm).toBeVisible();
  const details=pane.getByRole('heading',{name:t.detailInfo.details,exact:true});await expect(details).toBeVisible();
  expect((await confirm.boundingBox())!.y).toBeLessThan((await details.boundingBox())!.y);
  const memoryWidth=(await pane.boundingBox())!.width;
  const memoryValueX=(await pane.locator('.memory-detail-info-value').first().boundingBox())!.x;
  expect(await pane.locator('.memory-detail-field-label').evaluate(e=>getComputedStyle(e).textTransform)).toBe('none');
  expect(await pane.locator('.memory-detail-field-list').evaluate(e=>getComputedStyle(e).borderTopWidth)).toBe('0px');
  await page.evaluate(()=>document.fonts.ready);await page.mouse.move(0,0);await page.screenshot({path:info.outputPath('01-memory.png')});
  await pane.getByRole('button',{name:t.memoryDetail.showAll.replace('{{count}}','2'),exact:true}).click();
  await pane.getByRole('button',{name:/先前的準備方式/}).scrollIntoViewIfNeeded();
  await expect(pane.getByText('帶水瓶，沿河散步。',{exact:true})).toBeVisible();
  await page.screenshot({path:info.outputPath('02-memory-history.png'),animations:'disabled'});
  const metadataTime=await pane.locator('.memory-detail-info-value time').textContent();
  if(width===1440){
   const trigger=page.getByRole('button',{name:t.memoryDetail.actions,exact:true});await trigger.click();const menu=page.getByRole('menu',{name:t.memoryDetail.actions,exact:true});await expect(menu).toBeVisible();await page.mouse.move(0,0);
   const pointer=await menu.locator('[role^=menuitem]').first().evaluate(e=>({outline:getComputedStyle(e).outlineStyle,bg:getComputedStyle(e).backgroundColor}));expect(pointer.outline).toBe('none');expect(pointer.bg).toBe('rgba(0, 0, 0, 0)');
   await page.keyboard.press('Escape');await expect(trigger).toBeFocused();await expect(pane).toBeVisible();await trigger.press('ArrowUp');await expect(menu.locator('[role^=menuitem]').last()).toBeFocused();await page.keyboard.press('Escape');
   await trigger.click();await page.locator('.memory-detail-reading').click({position:{x:4,y:4}});await expect(menu).toHaveCount(0);
  }else{await pane.getByRole('button',{name:t.common.close,exact:true}).click();}

  await navigate(t.sidebar.entities);await page.getByRole('button',{name:'Ada Lovelace',exact:true}).click();await openInfo();
  await expect(pane.getByRole('heading',{name:t.detailInfo.details,exact:true})).toBeVisible();
  await expect(pane.getByText(t.detailInfo.relatedMemoriesHint,{exact:true})).toBeVisible();
  expect((await pane.boundingBox())!.width).toBeCloseTo(memoryWidth,0);
  const topicValueX=(await pane.locator('.memory-detail-metadata-value').first().boundingBox())!.x;
  expect(Math.abs(topicValueX-memoryValueX)).toBeLessThanOrEqual(3);
  await expect(pane.locator('.memory-detail-metadata-value').filter({hasText:metadataTime!})).toBeVisible();
  expect(await pane.locator('.entity-relation-list').evaluate(e=>getComputedStyle(e).borderTopWidth)).toBe('0px');
  await page.evaluate(()=>document.fonts.ready);await page.mouse.move(0,0);await page.screenshot({path:info.outputPath('03-topic.png')});

  expect(await page.evaluate(()=>document.documentElement.scrollWidth-innerWidth)).toBeLessThanOrEqual(1);
  expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
  expect(runtime.calls().filter(c=>c.command==='update_memory_cmd')).toHaveLength(0);
  writeFileSync(info.outputPath('evidence.json'),JSON.stringify({locale,theme,width,memoryWidth,metadataTime,errors},null,2));
 });
}

test("Empty Memory and Topic keep the shared Info hierarchy",async({page},info)=>{
 const base=createSpacesNavigationFixture();
 const fixture={...base,entityDetails:base.entityDetails.map(detail=>({...detail,relations:[]}))};
 await page.setViewportSize({width:1440,height:900});
 await installTauriMock(page,{fixture,locale:'en',rawActions:[],localStorage:{'wenlan-theme':'light','wenlan-navigation-v1':JSON.stringify({version:1,visible:['pages','spaces','graph','sources','memories','entities'],sidebar:{visible:true,mode:'icons',width:240}})}});
 await page.goto('/');
 await expect(page.locator('.memory-shell')).toBeVisible();
 await page.evaluate(()=>{const bridge=window.__TAURI_INTERNALS__!,invoke=bridge.invoke;bridge.invoke=async(command,args,options)=>command==='get_knowledge_graph_cmd'?{entities:[],relations:[],memory_links:[],memories:[],pages:[],page_links:[]}:invoke(command,args,options);});
 const nav=page.getByRole('navigation',{name:'Primary navigation'});
 await nav.getByRole('button',{name:'Memories',exact:true}).click();
 await page.getByRole('article',{name:'Fixture architecture',exact:true}).getByRole('button',{name:'Open memory',exact:true}).click();
 const pane=page.locator('.page-info-drawer:visible');
 await expect(pane.getByRole('heading',{name:'Connections',exact:true})).toBeVisible();
 await expect(pane.getByText('No direct connections in the currently visible knowledge.',{exact:true})).toBeVisible();
 await page.screenshot({path:info.outputPath('empty-memory.png'),animations:'disabled'});
 await nav.getByRole('button',{name:'Topics',exact:true}).click();
 await page.getByRole('button',{name:'Ada Lovelace',exact:true}).click();
 await expect(pane.getByRole('heading',{name:'Details',exact:true})).toBeVisible();
 await expect(pane.getByText('No connections recorded yet.',{exact:true})).toBeVisible();
 await page.screenshot({path:info.outputPath('empty-topic.png'),animations:'disabled'});
});

// The common reading frame keeps a leading origin while its measure responds to Info.
for (const scenario of [
 { width:1440, theme:'light', mode:'labels', long:false },
 { width:1440, theme:'dark', mode:'icons', long:true },
 { width:1100, theme:'light', mode:'labels', long:false },
 { width:375, theme:'light', mode:'icons', long:false },
] as const) {
 test(`Memory reading origin ${scenario.width} ${scenario.theme}`, async({page},info)=>{
  const {width,theme,mode}=scenario, base=createSpacesNavigationFixture();
  const text=scenario.long?Array(12).fill(content).join('\n\n'):content;
  const fixture={...base,memories:base.memories.map((m,i)=>i===0?{...m,content:text}:m)};
  const errors=collectBrowserErrors(page);
  await page.setViewportSize({width,height:900});
  const runtime=await installTauriMock(page,{fixture,locale:'en',rawActions:[],localStorage:{'wenlan-theme':theme,'wenlan-navigation-v1':JSON.stringify({version:1,visible:['pages','spaces','graph','sources','memories','entities'],sidebar:{visible:true,mode,width:240}})}});
  await page.goto('/');
  await expect(page.locator('.memory-shell')).toBeVisible();
  const nav=page.getByRole('navigation',{name:'Primary navigation'});
  if(!await nav.isVisible())await page.getByRole('button',{name:'Show sidebar',exact:true}).click();
  await nav.getByRole('button',{name:'Memories',exact:true}).click();
  await page.getByRole('article',{name:'Fixture architecture',exact:true}).getByRole('button',{name:'Open memory',exact:true}).click();
  const editor=page.locator('.memory-document-editor .cm-content');
  await expect(editor).toContainText(content);
  const pane=page.locator('.page-info-drawer:visible');
  const measure=async(label:string)=>{
   await page.evaluate(()=>document.fonts.ready);
   const main=(await page.locator('main.memory-detail-dossier').boundingBox())!;
   await expect(async()=>{
    const body=(await page.locator('.memory-detail-reading').boundingBox())!;
    const frame=page.locator('.memory-document-frame');
    const frameBox=(await frame.boundingBox())!;
    const framePaddingLeft=Number.parseFloat(await frame.evaluate(e=>getComputedStyle(e).paddingLeft));
    expect(Math.abs(body.x-frameBox.x-framePaddingLeft)).toBeLessThanOrEqual(2);
    expect(body.width).toBeLessThanOrEqual(561);
    expect(body.x-main.x).toBeGreaterThanOrEqual(width<=640?15:31);
   }).toPass();
   const metrics=await editor.evaluate(e=>({font:getComputedStyle(e).fontSize,lineHeight:getComputedStyle(e).lineHeight,textAlign:getComputedStyle(e).textAlign}));
   expect(metrics.font).toBe('15px');expect(metrics.lineHeight).toBe('26.25px');expect(metrics.textAlign).not.toBe('center');
   expect(await page.evaluate(()=>document.documentElement.scrollWidth-innerWidth)).toBeLessThanOrEqual(1);
   await page.mouse.move(0,0);
   await page.screenshot({path:info.outputPath(`${label}.png`),animations:'disabled'});
   return {label,body:await editor.boundingBox(),frame:await page.locator('.memory-document-frame').boundingBox(),metrics};
  };
  const measurements=[];
  if(width>=1100){
   await expect(pane).toBeVisible();
   measurements.push(await measure('both-sidebars'));
   // Below 1160 the labelled rail folds to icons while Info is open; its toggle expands it first.
   if(mode==='labels'&&width<240+320+600){
    await expect(page.locator('.memory-shell')).toHaveAttribute('data-sidebar-layout','icons');
    await page.getByRole('button',{name:'Show sidebar',exact:true}).click();
    await expect(page.locator('.memory-shell')).toHaveAttribute('data-sidebar-layout','labels');
   }
   await page.getByRole('button',{name:'Hide sidebar',exact:true}).click();
   measurements.push(await measure('left-closed'));
   await pane.getByRole('button',{name:'Close',exact:true}).click();
   measurements.push(await measure('both-closed'));
  }else{await expect(pane).toHaveCount(0);measurements.push(await measure('narrow'));}
  expect(errors.pageErrors).toEqual([]);expect(errors.consoleErrors).toEqual([]);
  expect(runtime.calls().filter(c=>c.command==='update_memory_cmd')).toHaveLength(0);
  writeFileSync(info.outputPath('reading-evidence.json'),JSON.stringify({scenario,measurements,errors},null,2));
 });
}
