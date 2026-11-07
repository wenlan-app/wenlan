// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { resolve } from "node:path";
test("Zotero disconnect reviews scoped impact and sends its fence", async ({page}) => {
  const errors:string[]=[]; page.on("pageerror",error=>errors.push(error.message));
  await page.addInitScript(() => {
    const preview={identity:{registration_id:"reg-1",locator:"private-locator",server_id:"server",space_id:"space-id"},identity_digest:"digest",space_name:"Research",revision:2,canonical_item_ids:["private-paper"],affected_page_ids:["page-1","foreign-id"],impact_scope_known:true,pending_disconnect:false,disconnected:false};
    (window as any).__fixtureCalls=[];
    Object.assign(window,{__TAURI_INTERNALS__:{invoke:async(command:string,args:any)=>{
      (window as any).__fixtureCalls.push({command,args});
      if(command==="list_registered_sources") return [{id:"reg-1",source_type:"zotero",path:"zotero-local/collection",space:"Research",status:"Active",last_sync:null,file_count:1,memory_count:1}];
      if(command==="get_zotero_disconnect_preview") return preview;
      if(command==="get_page") return args.id === "page-1" ? {id:"page-1",title:"Evidence synthesis",space:"Research"} : {id:"foreign-id",title:"Foreign private title",space:"Other"};
      if(command==="remove_source") return null;
      if(command==="get_activity") return {state:"up_to_date",assets:[],everyday:{available:true},synthesis:{available:true}};
      throw new Error(`Unexpected command ${command}`);
    }}});
  });
  await page.goto("/preview/zotero-disconnect.html");
  await page.getByRole("button",{name:"More"}).click(); await page.getByRole("menuitem",{name:"Remove source"}).click();
  await expect(page.getByText("Evidence synthesis")).toBeVisible(); await expect(page.getByText("Page unavailable")).toBeVisible();
  const dialog=page.getByRole("dialog"); await expect(dialog).toContainText("Retained papers: 1"); await expect(dialog).toContainText("Affected research pages: 2"); await expect(dialog).not.toContainText(/private-locator|foreign-id|Foreign private title/);
  if(process.env.ZOTERO_DISCONNECT_EVIDENCE_DIR) await page.screenshot({path:resolve(process.env.ZOTERO_DISCONNECT_EVIDENCE_DIR,"confirmation.png")});
  await page.getByRole("button",{name:"Disconnect",exact:true}).click(); await expect(dialog).toHaveCount(0);
  const calls=await page.evaluate(()=>(window as any).__fixtureCalls); expect(calls.find((x:any)=>x.command==="remove_source")?.args).toMatchObject({id:"reg-1",expectedZoteroPreview:{identity_digest:"digest",revision:2,identity:{registration_id:"reg-1"}}}); expect(errors).toEqual([]);
});
test("interrupted disconnect can be finished from the recovered source row", async ({page}) => {
  const errors:string[]=[]; page.on("pageerror",error=>errors.push(error.message));
  await page.addInitScript(() => {
    let removed=false;
    const preview={identity:{registration_id:"reg-recovery",locator:"hidden",server_id:"server",space_id:"space-id"},identity_digest:"fence",space_name:"Research",revision:4,canonical_item_ids:[],affected_page_ids:[],impact_scope_known:false,pending_disconnect:true,disconnected:false};
    (window as any).__fixtureCalls=[];
    Object.assign(window,{__TAURI_INTERNALS__:{invoke:async(command:string,args:any)=>{
      (window as any).__fixtureCalls.push({command,args});
      if(command==="list_registered_sources") return removed ? [] : [{id:"reg-recovery",source_type:"zotero",path:"zotero/private-locator",space:"Research",status:"Active",last_sync:null,file_count:0,memory_count:0,last_sync_error_detail:"zotero_disconnect_pending"}];
      if(command==="get_zotero_disconnect_preview") return preview;
      if(command==="remove_source") {removed=true;return null;}
      if(command==="get_activity") return {state:"up_to_date",assets:[],everyday:{available:true},synthesis:{available:true}};
      throw new Error(`Unexpected command ${command}`);
    }}});
  });
  await page.goto("/preview/zotero-disconnect.html");
  await expect(page.getByText("Zotero · Research")).toBeVisible();
  await expect(page.getByText(/0 files|0 memories/)).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Sync",exact:true})).toHaveCount(0);
  if(process.env.ZOTERO_DISCONNECT_EVIDENCE_DIR) await page.screenshot({path:resolve(process.env.ZOTERO_DISCONNECT_EVIDENCE_DIR,"recovery-row.png")});
  await page.getByRole("button",{name:"Finish disconnect"}).click();
  await expect(page.getByText(/previous disconnect was interrupted/)).toBeVisible();
  await expect(page.getByText(/older connection cannot identify affected pages/)).toBeVisible();
  if(process.env.ZOTERO_DISCONNECT_EVIDENCE_DIR) await page.screenshot({path:resolve(process.env.ZOTERO_DISCONNECT_EVIDENCE_DIR,"recovery-confirmation.png")});
  await page.getByRole("button",{name:"Disconnect",exact:true}).click();
  await expect(page.getByRole("button",{name:"Finish disconnect"})).toHaveCount(0);
  const calls=await page.evaluate(()=>(window as any).__fixtureCalls); expect(calls.find((x:any)=>x.command==="remove_source")?.args).toMatchObject({id:"reg-recovery",expectedZoteroPreview:{identity_digest:"fence",revision:4,pending_disconnect:true}}); expect(calls.some((x:any)=>x.command==="sync_registered_source")).toBe(false); expect(errors).toEqual([]);
});
