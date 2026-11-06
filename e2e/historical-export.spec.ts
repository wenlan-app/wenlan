// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

test("export chooses a retained version and labels pending and unavailable entries", async ({page}) => {
  const errors = collectBrowserErrors(page);
  const fixture = createSpacesNavigationFixture();
  const note = { id:"export-note",title:"Export research",summary:null,content:"Current text",entity_id:null,domain:"Wenlan",space:"Wenlan",source_memory_ids:[],version:4,status:"active",creation_kind:"authored",review_status:"confirmed",created_at:"2026-11-03T12:00:00Z",last_compiled:"2026-11-03T12:00:00Z",last_modified:"2026-11-03T12:00:00Z",user_edited:false };
  await installTauriMock(page,{fixture:{...fixture,pages:[note]},locale:"en",rawActions:[]});
  await page.setViewportSize({width:1280,height:900}); await page.goto("/");
  await page.evaluate(() => {
    const bridge=window.__TAURI_INTERNALS__!, original=bridge.invoke;
    const base={id:"current",digest:"d1",kind:"saved",page_id:"export-note",page_incarnation:"inc",space_id:"space",version:4,export_revision:2,source_revision:1,title:"Current",content:"Current text",last_modified:"2026-11-03T12:00:00Z",captured_at:1793707200000,citations:[],sources:[],bibliography:[],references_available:true,pending_revision_id:null};
    const old={...base,id:"old",digest:"d2",version:2,title:"Old research",content:"Old claim [1]",citations:[{occurrence:1,marker:1,source_kind:"memory",locator:"old-locator",score:1,status:"verified",scope:"sentence"}],sources:[{locator:"old-locator",source_kind:"file",source_id:"paper",title:"Old paper",source_agent:"zotero",url:null,text_digest:null,historical:false}],bibliography:[{source_id:"paper",title:"Old paper",bibtex:"@article{old, title={Old paper}}",availability:"available"}]};
    const pending={...old,id:"pending",digest:"d3",kind:"pending_draft",pending_revision_id:"revision"};
    const version=(x: Pick<typeof base, "id" | "kind" | "version" | "export_revision" | "title" | "last_modified" | "captured_at">)=>({bundle_id:x.id,kind:x.kind,version:x.version,export_revision:x.export_revision,title:x.title,saved_at:x.last_modified,captured_at:x.captured_at,references_available:true,bibliography_available:true,unavailable_reason:null});
    const evidence={contents:""}; (window as unknown as {exportEvidence:typeof evidence}).exportEvidence=evidence;
    bridge.invoke=(command,args)=>{
      if(command==="capture_page_export") return Promise.resolve(base);
      if(command==="list_page_export_versions") return Promise.resolve({versions:[version(base),version(old),version(pending),{...version(old),bundle_id:null,kind:"legacy_unavailable",references_available:false}]});
      if(command==="get_page_export_bundle") { const params = args as { bundleId?: string } | undefined; return Promise.resolve(params?.bundleId==="old"?old:params?.bundleId==="pending"?pending:base); }
      if(command==="plugin:dialog|save") return Promise.resolve("/tmp/old.md");
      if(command==="plugin:fs|write_text_file") {evidence.contents=new TextDecoder().decode(args as unknown as Uint8Array); return Promise.resolve();}
      return original(command,args);
    };
  });
  await openPrimaryDestination(page,"Wiki"); await openWikiNote(page,note.title);
  await page.getByRole("button",{name:"Page actions"}).click(); await page.getByRole("menuitem",{name:"Export…"}).click();
  const select=page.getByLabel("Version"); await expect(select).toHaveValue("current");
  await expect(page.getByRole("option",{name:/Pending draft.*not adopted/})).toBeAttached();
  await expect(page.getByRole("option",{name:/References were not retained/})).toHaveAttribute("disabled", "");
  await select.selectOption("old"); await expect(page.getByRole("radio",{name:"BibTeX"})).toBeEnabled();
  await page.screenshot({path:"/Users/lucian/.codex/visualizations/2026/11/03/01a19fbf-c0b9-77f2-99af-fbf769921f49/research-v1/completion-v021/historical-export/selection.png"});
  await page.getByRole("button",{name:"Export",exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as unknown as {exportEvidence:{contents:string}}).exportEvidence.contents)).toContain("Old paper");
  expect(await page.evaluate(()=>(window as unknown as {exportEvidence:{contents:string}}).exportEvidence.contents)).toContain("Old claim");
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
