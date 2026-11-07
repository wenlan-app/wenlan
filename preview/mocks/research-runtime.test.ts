// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
import { createSpacesNavigationFixture } from "../../e2e/fixtures/spacesNavigation";
import type { OrganizeRun, OrganizeRules } from "../../src/lib/researchApi";
import type { PageExportBundle } from "../../src/lib/pageExportApi";
const runtime = () => new TauriMockRuntime({ ...createSpacesNavigationFixture(), documents: [{ file: { source: "file", source_id: "paper-0", title: "Paper", space: "Wenlan", chunk_count: 1, last_modified: 1 }, content: "Indexed paper text" }] });
describe("Review research fixture state", () => {
    it("persists rules with revision conflicts and rejects unknown Spaces", async () => {
        const r = runtime();
        const rules = await r.invoke("get_research_rules", { space: "Research" }) as OrganizeRules;
        expect(rules.revision).toBe(0);
        await r.invoke("save_research_rules", { request: { space: "Research", expected_revision: 0, name: "Compare methods", instructions: "Keep limitations" } });
        expect(await r.invoke("get_research_rules", { space: "Research" })).toMatchObject({ revision: 1, instructions: "Keep limitations" });
        await expect(r.invoke("save_research_rules", { request: { space: "Research", expected_revision: 0, name: "stale", instructions: "" } })).rejects.toThrow("conflict");
        await expect(r.invoke("get_research_rules", { space: "Unknown" })).rejects.toThrow("Space not found");
    });
    it("tracks resumable runs without fabricating source read coverage", async () => {
        const r = runtime();
        const run = await r.invoke("create_research_run", { request: { space: "Wenlan", sources: [{ source: "file", source_id: "paper-0", title: "source" }], target_page_id: "page-architecture" } }) as OrganizeRun;
        expect(run.items[0].coverage).toBe("pending");
        expect(await r.invoke("update_research_run", { id: run.id, request: { space: "Wenlan", expected_revision: 1, resume: true } })).toMatchObject({ state: "in_progress", revision: 2 });
        await expect(r.invoke("update_research_run", { id: run.id, request: { space: "Wenlan", expected_revision: 1, cancel: true } })).rejects.toThrow("conflict");
        await expect(r.invoke("update_research_run", { id: run.id, request: { space: "Wenlan", expected_revision: 2, coverage: "complete" } })).rejects.toThrow("no source-read receipts");
        await expect(r.invoke("get_research_run", { space: "Research", id: run.id })).rejects.toThrow("not found in Space");
    });
    it("keeps export snapshots immutable and scoped", async () => {
        const r = runtime();
        const bundle = await r.invoke("capture_page_export", { pageId: "page-architecture", space: "Wenlan" }) as PageExportBundle;
        expect(bundle.references_available).toBe(false);
        bundle.content = "tampered";
        expect(await r.invoke("get_page_export_bundle", { pageId: bundle.page_id, bundleId: bundle.id, space: "Wenlan" })).not.toMatchObject({ content: "tampered" });
        expect(await r.invoke("list_page_export_versions", { pageId: bundle.page_id })).toMatchObject({ versions: [{ bundle_id: bundle.id }] });
        await expect(r.invoke("get_page_export_bundle", { pageId: bundle.page_id, bundleId: bundle.id, space: "Research" })).rejects.toThrow("Page not found");
    });
    it("records dispute decisions only at the matching scope and payload", async () => {
        const fixture = createSpacesNavigationFixture();
        const digest = "a".repeat(64);
        const dispute = { space_id: "space-wenlan", kind: "opposing_claims" as const, conditions: "Same task", reason: "Results differ", evidence: [{ source: "file" as const, source_id: "paper-a", source_digest: digest, quote: "First result" }, { source: "file" as const, source_id: "paper-b", source_digest: digest, quote: "Second result" }] };
        const r = new TauriMockRuntime({ ...fixture, refinements: [{ id: "dispute-1", action: "research_dispute", source_ids: ["paper-a", "paper-b"], confidence: 1, created_at: "2026-07-10", payload: { action: "research_dispute", space: "Wenlan", dispute, payload_digest: digest } }] });
        const args = { space: "Wenlan", id: "dispute-1", expectedPayloadDigest: digest, decision: "defer", reason: "Need source access" };
        await expect(r.invoke("decide_research_dispute", { ...args, expectedPayloadDigest: "b".repeat(64) })).rejects.toThrow("conflict");
        for (const decision of ["confirm_difference", "not_comparable", "correct_interpretation"]) {
            await expect(r.invoke("decide_research_dispute", {...args,decision})).rejects.toThrow("unchanged sources");
        }
        expect(await r.invoke("decide_research_dispute", args)).toMatchObject({ status: "awaiting_review", decisions: [{ decision: "defer" }], source_states: [{ state: "unavailable" }, { state: "unavailable" }] });
        expect(await r.invoke("decide_research_dispute", { ...args, decision: "close" })).toMatchObject({ status: "resolved" });
        await expect(r.invoke("decide_research_dispute", args)).rejects.toThrow("conflict");
        await expect(r.invoke("get_research_dispute", { space: "Research", id: "dispute-1" })).rejects.toThrow("not found in Space");
    });
    it.each([
        {status:"draft"}, {status:"deleted"}, {kind:"entity"}, {space:"Unregistered"},
        {truth:{subject:{},content:{}}},
    ])("refuses non-exportable fixture pages: %j",async patch=>{
        const fixture=createSpacesNavigationFixture();
        const page={...fixture.pages[0],...patch} as typeof fixture.pages[number];
        const r=new TauriMockRuntime({...fixture,pages:[page]});
        for(const command of ["capture_page_export","list_page_export_versions","get_page_export_bundle"])
            await expect(r.invoke(command,{pageId:page.id,bundleId:"missing"})).rejects.toThrow(/exportable|registered|cutover/);
    });
    it("exports archived concept and independent pages",async()=>{
        const fixture=createSpacesNavigationFixture();
        for(const patch of [{status:"archived"},{space:null,domain:null}]) {
            const page={...fixture.pages[0],...patch};
            const r=new TauriMockRuntime({...fixture,pages:[page]});
            expect(await r.invoke("capture_page_export",{pageId:page.id})).toMatchObject({page_id:page.id,kind:"saved"});
        }
    });
    it("resolves explicit uncategorized export scope to null Space only",async()=>{
        const fixture=createSpacesNavigationFixture();
        const page={...fixture.pages[0],space:null,domain:null};
        const r=new TauriMockRuntime({...fixture,pages:[page,fixture.pages[1]]});
        const bundle=await r.invoke("capture_page_export",{pageId:page.id,space:"uncategorized"}) as PageExportBundle;
        expect(bundle.space_id).toBeNull();
        expect(await r.invoke("list_page_export_versions",{pageId:page.id,space:"uncategorized"})).toMatchObject({versions:[{bundle_id:bundle.id}]});
        expect(await r.invoke("get_page_export_bundle",{pageId:page.id,bundleId:bundle.id,space:"uncategorized"})).toMatchObject({id:bundle.id});
        for(const command of ["capture_page_export","list_page_export_versions","get_page_export_bundle"])
            await expect(r.invoke(command,{pageId:fixture.pages[1].id,space:"uncategorized",bundleId:bundle.id})).rejects.toThrow("Page not found in Space");
    });
    it("rejects ambiguous registered uncategorized export scope",async()=>{
        const fixture=createSpacesNavigationFixture();
        const r=new TauriMockRuntime({...fixture,spaces:[...fixture.spaces,{...fixture.spaces[0],id:"space-ambiguous",name:"uncategorized"}],pages:[{...fixture.pages[0],space:"uncategorized"},{...fixture.pages[1],space:null}]});
        for(const command of ["capture_page_export","list_page_export_versions","get_page_export_bundle"])
            for(const page of fixture.pages.slice(0,2))
                await expect(r.invoke(command,{pageId:page.id,space:"uncategorized",bundleId:"missing"})).rejects.toThrow("Ambiguous uncategorized");
    });
    it("persists scoped lifecycle history with CAS and refuses nonexistent native work", async () => {
        const r = runtime();
        const request = { space: "Wenlan", source: "file", source_id: "paper-0", expected_revision: 0, state: "excluded", reason: "Outside scope" };
        expect(await r.invoke("set_source_lifecycle", { request })).toMatchObject({ record: { revision: 1, state: "excluded" }, history: [{ reason: "Outside scope" }] });
        await expect(r.invoke("set_source_lifecycle", { request })).rejects.toThrow("conflict");
        await expect(r.invoke("get_source_lifecycle", { space: "Research", source: "file", sourceId: "paper-0" })).rejects.toThrow("not found in Space");
        await expect(r.invoke("get_zotero_disconnect_preview", { id: "absent" })).rejects.toThrow("no connected Zotero");
        await expect(r.invoke("accept_research_classification", { space: "Wenlan", id: "absent", changes: [] })).rejects.toThrow("no adoptable");
        await expect(r.invoke("get_research_dispute", { space: "Wenlan", id: "absent" })).rejects.toThrow("not found in Space");
    });
});
