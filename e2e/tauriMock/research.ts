// SPDX-License-Identifier: AGPL-3.0-only
// Fixture-only research state. No daemon, filesystem, Zotero, or PDF access.
import type { Page, Space, RefinementProposalSummary, IndexedFileInfo } from "../../src/lib/tauri";
import type { OrganizeRules, OrganizeRun } from "../../src/lib/researchApi";
import type { PageExportBundle } from "../../src/lib/pageExportApi";
import type { SourceLifecycleRecord } from "../../src/lib/sourceLifecycleApi";
import { isResearchDisputePayload, type ResearchDisputeView, type DisputeDecision } from "../../src/lib/disputeApi";
const commands = new Set(["get_research_dispute", "decide_research_dispute", "get_research_rules", "save_research_rules", "list_research_runs", "create_research_run", "get_research_run", "update_research_run", "accept_research_classification", "merge_research_tags", "get_source_lifecycle", "set_source_lifecycle", "resolve_source_reference", "get_zotero_disconnect_preview", "list_zotero_items", "capture_page_export", "list_page_export_versions", "get_page_export_bundle"]);
const object = (value: unknown): Record<string, unknown> => {
    if (!value || typeof value !== "object" || Array.isArray(value))
        throw new Error("Malformed research arguments");
    return value as Record<string, unknown>;
};
const text = (value: unknown): string => { if (typeof value !== "string" || !value.trim())
    throw new Error("Expected nonempty research argument"); return value; };
const revision = (value: unknown): number => { if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0)
    throw new Error("Invalid revision"); return value; };
// Fixture identity, deliberately not a cryptographic receipt of source reading.
const fixtureDigest = (value: unknown) => { let hash = 2166136261; for (const char of JSON.stringify(value))
    hash = Math.imul(hash ^ char.charCodeAt(0), 16777619); return (hash >>> 0).toString(16).padStart(8, "0").repeat(8); };
const now = 1783728000;
export class ResearchFixtureStore {
    private rules = new Map<string, OrganizeRules>();
    private runs = new Map<string, OrganizeRun>();
    private bundles = new Map<string, PageExportBundle>();
    private lifecycle = new Map<string, SourceLifecycleRecord[]>();
    private decisions = new Map<string, ResearchDisputeView>();
    supports(command: string) { return commands.has(command); }
    async invoke(command: string, input: unknown, spaces: readonly Space[], pages: readonly Page[], files: readonly IndexedFileInfo[], proposals: RefinementProposalSummary[]): Promise<unknown> {
        const a = object(input);
        const r = a.request === undefined ? a : object(a.request);
        const scope = (value: unknown) => { const name = text(value); const space = spaces.find(s => s.name === name); if (!space)
            throw new Error("Space not found"); return space; };
        const source = (space: string, kind: unknown, id: unknown) => { const file = files.find(f => f.source === kind && f.source_id === id && (f.space ?? f.domain) === space); if (!file)
            throw new Error("Source not found in Space"); return file; };
        const getRules = (space: Space): OrganizeRules => this.rules.get(space.id) ?? { space_id: space.id, revision: 0, name: "Research rules", instructions: "", content_digest: fixtureDigest("") };
        if (command === "get_zotero_disconnect_preview" || command === "list_zotero_items")
            throw new Error("Review fixture has no connected Zotero library");
        if (command.includes("page_export")) {
            if (a.space != null && typeof a.space !== "string")
                throw new Error("Invalid export Space");
            const requestedSpace = typeof a.space === "string" ? a.space.trim() : "";
            if (requestedSpace === "uncategorized" && spaces.some(space => space.name === "uncategorized"))
                throw new Error("Ambiguous uncategorized Space");
            const page = pages.find(p => p.id === a.pageId && (!requestedSpace || (requestedSpace === "uncategorized" ? p.space == null : requestedSpace === p.space)));
            if (!page)
                throw new Error("Page not found in Space");
            if (!["active", "archived"].includes(page.status) || Reflect.get(page,"kind") === "entity")
                throw new Error("Page is not exportable");
            if (page.space != null && !spaces.some(space => space.name === page.space))
                throw new Error("Page Space is not registered");
            // This store does not certify daemon truth cutover permissions.
            // A truth-bearing fixture signals cutover: refuse rather than guess.
            if (page.truth != null)
                throw new Error("Review fixture cannot certify truth cutover export permissions");
            if (command === "capture_page_export") {
                const fingerprint = fixtureDigest([page.id, page.version, page.content, page.citations]);
                const id = `fixture-export-${page.id}-${fingerprint}`;
                if (!this.bundles.has(id))
                    this.bundles.set(id, { id, digest: fingerprint, kind: "saved", page_id: page.id, page_incarnation: `fixture-${page.id}`, space_id: spaces.find(s => s.name === page.space)?.id ?? null, version: page.version, export_revision: this.bundles.size + 1, source_revision: 0, title: page.title, content: page.content, last_modified: page.last_modified ?? page.created_at, captured_at: now, citations: structuredClone(page.citations ?? null), sources: [], bibliography: [], references_available: !(page.source_memory_ids?.length || page.citations?.length), pending_revision_id: null });
                return structuredClone(this.bundles.get(id));
            }
            const bundles = [...this.bundles.values()].filter(b => b.page_id === page.id && b.space_id === (spaces.find(s => s.name === page.space)?.id ?? null));
            if (command === "list_page_export_versions")
                return { versions: bundles.map(b => ({ bundle_id: b.id, kind: b.kind, version: b.version, export_revision: b.export_revision, title: b.title, saved_at: b.last_modified, captured_at: b.captured_at, references_available: b.references_available, bibliography_available: false, unavailable_reason: b.references_available ? null : "Fixture source bibliography unavailable" })) };
            const bundle = bundles.find(b => b.id === a.bundleId);
            if (!bundle)
                throw new Error("Export bundle not found");
            return structuredClone(bundle);
        }
        const space = scope(r.space);
        if (command === "get_research_rules")
            return structuredClone(getRules(space));
        if (command === "save_research_rules") {
            const old = getRules(space);
            if (revision(r.expected_revision) !== old.revision)
                throw new Error("Research rules revision conflict");
            if (typeof r.instructions !== "string")
                throw new Error("Invalid instructions");
            const saved = { ...old, revision: old.revision + 1, name: text(r.name), instructions: r.instructions, content_digest: fixtureDigest([r.name, r.instructions]) };
            this.rules.set(space.id, saved);
            return structuredClone(saved);
        }
        if (command === "list_research_runs")
            return { runs: structuredClone([...this.runs.values()].filter(run => run.space_id === space.id)) };
        if (command === "create_research_run") {
            if (!Array.isArray(r.sources) || !r.sources.length)
                throw new Error("Research run needs sources");
            const target = r.target_page_id == null ? null : pages.find(p => p.id === r.target_page_id && p.space === space.name);
            if (r.target_page_id != null && !target)
                throw new Error("Target page not found in Space");
            const items = r.sources.map(value => { const ref = object(value); const file = source(space.name, ref.source, ref.source_id); return { source: file.source, source_id: file.source_id, title: file.title, coverage: "pending" as const, content_digest: "", decision_revision: 0, next_offset: null, error: null }; });
            const run: OrganizeRun = { id: `fixture-run-${this.runs.size + 1}`, space_id: space.id, revision: 1, rules: structuredClone(getRules(space)), items, target_page_id: target?.id ?? null, target_page_version: target?.version ?? null, result_page_version: null, pending_revision_id: null, state: "prepared", created_at: now, updated_at: now };
            this.runs.set(run.id, run);
            return structuredClone(run);
        }
        if (command === "get_research_run" || command === "update_research_run") {
            const run = this.runs.get(text(a.id));
            if (!run || run.space_id !== space.id)
                throw new Error("Research run not found in Space");
            if (command === "update_research_run") {
                if (revision(r.expected_revision) !== run.revision)
                    throw new Error("Research run revision conflict");
                if (r.resume !== true && r.cancel !== true)
                    throw new Error("Review has no source-read receipts; cannot assert coverage");
                if (r.resume === true && r.cancel === true)
                    throw new Error("Conflicting run action");
                run.state = r.cancel === true ? "cancelled" : "in_progress";
                run.revision++;
                run.updated_at++;
            }
            return structuredClone(run);
        }
        if (command === "resolve_source_reference" || command === "get_source_lifecycle" || command === "set_source_lifecycle") {
            const file = command === "resolve_source_reference" ? files.find(f => (f.space ?? f.domain) === space.name && (f.source_id === r.locator || `${f.source}::${f.source_id}` === r.locator)) : source(space.name, r.source, r.source_id ?? r.sourceId);
            if (!file)
                throw new Error("Source reference not found in Space");
            if (command === "resolve_source_reference")
                return { space_id: space.id, source: file.source, source_id: file.source_id, source_agent: file.source_agent ?? null, title: file.title };
            const key = JSON.stringify([space.id, file.source, file.source_id]);
            const history = this.lifecycle.get(key) ?? [];
            let record = history.at(-1) ?? { space_id: space.id, source: file.source, source_id: file.source_id, revision: 0, state: "active" as const, reason: "", actor: "fixture", updated_at: now };
            if (command === "set_source_lifecycle") {
                if (revision(r.expected_revision) !== record.revision)
                    throw new Error("Source lifecycle revision conflict");
                if (!["active", "excluded", "retracted", "removed"].includes(String(r.state)))
                    throw new Error("Invalid lifecycle state");
                record = { ...record, revision: record.revision + 1, state: r.state as SourceLifecycleRecord["state"], reason: text(r.reason), updated_at: now + history.length + 1 };
                history.push(record);
                this.lifecycle.set(key, history);
            }
            return structuredClone({ record, history, impacts: [] });
        }
        if (command === "get_research_dispute" || command === "decide_research_dispute") {
            const id = text(a.id);
            const proposal = proposals.find(p => p.id === id);
            const payload = proposal?.payload;
            if (!isResearchDisputePayload(payload) || payload.space !== space.name || payload.dispute.space_id !== space.id)
                throw new Error("Research dispute not found in Space");
            const view: ResearchDisputeView = this.decisions.get(id) ?? { space: space.name, id, status: "awaiting_review" as const, dispute: structuredClone(payload.dispute), payload_digest: payload.payload_digest, source_states: payload.dispute.evidence.map(e => ({ source: e.source, source_id: e.source_id, expected_digest: e.source_digest, current_digest: null, expected_pdf_digest: e.pdf_target?.pdf_digest ?? null, current_pdf_digest: null, state: "unavailable" as const })), decisions: [] };
            if (command === "decide_research_dispute") {
                if (a.expectedPayloadDigest !== view.payload_digest || view.status === "resolved")
                    throw new Error("Research dispute conflict");
                if (!["confirm_difference", "not_comparable", "correct_interpretation", "defer", "close"].includes(String(a.decision)))
                    throw new Error("Invalid dispute decision");
                if (["confirm_difference", "not_comparable", "correct_interpretation"].includes(String(a.decision)) && view.source_states.some(state => state.state !== "unchanged"))
                    throw new Error("Evidence decisions require unchanged sources");
                view.decisions.push({ decision: a.decision as DisputeDecision, reason: text(a.reason), actor: "fixture-human", decided_at: now + view.decisions.length, source_states: structuredClone(view.source_states) });
                view.status = a.decision === "defer" ? "awaiting_review" : "resolved";
                this.decisions.set(id, view);
            }
            return structuredClone(view);
        }
        // No synthetic classification adoption or tag persistence: Review has no
        // decision provenance for these sources. Never acknowledge a native write.
        if (command === "accept_research_classification")
            throw new Error("Classification proposal has no adoptable fixture decision provenance");
        if (command === "merge_research_tags") {
            if (!Array.isArray(r.sources) || !r.sources.length)
                throw new Error("Tag merge needs sources");
            for (const ref of r.sources) {
                const item = object(ref);
                source(space.name, item.source, item.source_id);
            }
            throw new Error("Review fixture has no persisted research tags to merge");
        }
        throw new Error(`Unsupported research fixture command: ${command}`);
    }
}
