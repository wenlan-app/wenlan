// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Page } from "../../../lib/tauri";
import { collectPageInventory, filterPageInventory, inventoryPageFilename, pageFolderPath, pageMatchesInventoryScope } from "./pageInventory";
const note = (overrides: Partial<Page> = {}): Page => ({ id:"a",title:"Note",summary:null,content:"",entity_id:null,domain:null,source_memory_ids:[],version:1,status:"active",created_at:"2026-07-16",last_modified:"2026-07-16",last_compiled:"2026-07-16",...overrides });
describe("real folder inventory", () => {
  it("uses real nested paths and direct children, never Space ownership", () => {
    const page = note({ storage_path:"Work/Research/read.md",space:"Personal" });
    expect(inventoryPageFilename(page)).toBe("Work/Research/read.md");
    expect(pageFolderPath(page)).toBe("Work/Research");
    expect(pageMatchesInventoryScope(page,"folder:Work")).toBe(false);
    expect(pageMatchesInventoryScope(page,"folder:Work/Research")).toBe(true);
    expect(pageMatchesInventoryScope(note({storage_path:"root.md"}),"folder:")).toBe(true);
  });
  it.each([null,undefined,"../plan.md","/plan.md","Work//plan.md","Work/.hidden.md","C:plan.md","Work\\plan.md","plan.md\n"])("rejects unsafe path %s without losing the note", storage_path => {
    const page = note({storage_path}); expect(inventoryPageFilename(page)).toBeNull(); expect(pageMatchesInventoryScope(page,"all")).toBe(true); expect(pageMatchesInventoryScope(page,"folder:")).toBe(false);
  });
  it("keeps drafts in their persisted destination and legacy notes only in all", () => {
    expect(pageFolderPath(note({status:"draft",folder_path:"Work"}))).toBe("Work");
    expect(pageMatchesInventoryScope(note({status:"draft",folder_path:"Work"}),"folder:Work")).toBe(true);
    expect(pageMatchesInventoryScope(note(),"folder:")).toBe(false);
    expect(pageMatchesInventoryScope(note({creation_kind:"entity"}),"all")).toBe(false);
  });
  it("keeps a note distilled about an entity, which carries entity_id", () => {
    const aboutEntity = note({ entity_id:"ent_libsql", creation_kind:"distilled", storage_path:"libsql-database-architecture.md" });
    expect(pageMatchesInventoryScope(aboutEntity,"folder:")).toBe(true);
    expect(collectPageInventory([aboutEntity],[])).toEqual([aboutEntity]);
    expect(collectPageInventory([note({ id:"shadow", entity_id:"ent_libsql", creation_kind:"entity" })],[])).toEqual([]);
  });
  it("deduplicates status transitions with active precedence and global search", () => {
    const active=note({storage_path:"Work/note.md"});
    expect(collectPageInventory([active],[note({status:"draft"})])).toEqual([active]);
    expect(filterPageInventory([active],"WORK/", "en")).toEqual([active]);
  });
});
