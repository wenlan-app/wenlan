// SPDX-License-Identifier: AGPL-3.0-only
// Synthetic protocol fixture: browser interaction proof, not native repair evidence.
import type { Page } from '@playwright/test';
import type { RepairLintReport, RepairManifest, RepairPlanEntry } from '../../src/lib/repairTypes';
import { createSpacesNavigationFixture } from './spacesNavigation';

const digest = (n: number) => n.toString(16).padStart(64, '0');
export function knowledgeReport(): RepairLintReport {
  return {
    report_schema_version: 1, check_catalog_version: 1, profile: 'general', scope: { kind: 'global' },
    capability_context: 'daemon_operator_endpoint_unauthenticated_unverified',
    snapshots: { db: { mode: 'transactional_read_only', analysis_digest: 'a'.repeat(16), post_run_digest: 'a'.repeat(16) }, pages: { mode: 'best_effort', before_scan_digest: 'b'.repeat(16), after_scan_digest: 'b'.repeat(16) } },
    config_fingerprint: 'c'.repeat(16), producer_receipt: { runtime_commit: '58bd55dc88b03d85482b85a31bf43e0b42b07c9d' },
    checks: ['pages.links.orphan_labels', 'pages.duplicate_active_titles'].map((check_id) => ({ check_id, outcome: 'finding', gate_effect: 'actionable', severity: 'warning', applicability: 'applicable', precondition: 'ready', coverage: { method: 'full_enumeration', authorized_denominator: 4, evaluated: 4, evidence_cap: 100, truncated: false, evidence_returned: 4 }, metrics: [], summary_code: 'finding_detected', evidence: [], duration_ms: 1 })),
    totals: { checks: 2, passed: 0, findings: 2, actionable_findings: 2, advisory_findings: 0, incomplete: 0 }, complete: true,
  };
}
export function knowledgeManifest(index: number): RepairManifest {
  const target: RepairManifest['target'] = { kind: 'page_link', source_page_id: index === 1 ? 'page-architecture' : 'page-keyboard', label_key: index === 1 ? 'Packing list' : 'Rainy-day route', scope: { kind: 'registered', space: 'Wenlan' } };
  const report = knowledgeReport();
  return {
    manifest_schema_version: 6, manifest_id: `repair_00000000-0000-4000-8000-${String(index).padStart(12, '0')}`, prepared_at: 1790000000,
    source: { report_schema_version: 1, check_catalog_version: 1, lint_scope: { kind: 'global' }, report_scope: { kind: 'global' }, check_id: 'pages.links.orphan_labels', deterministic_evidence: [{ kind: 'reason_code', reason_code: 'orphan_link_label' }], general_snapshots: report.snapshots, general_producer_receipt: report.producer_receipt },
    target, expected_state: { version: 1, canonical_receipt: digest(20 + index) }, writer: 'bind_page_link',
    mutation: { kind: 'bind_page_link', before_target_page_id: null, after_target_page_id: index === 1 ? 'page-browser' : 'page-errors' },
    allowed_effects: { owner: target, fields: ['target_page_id'] }, rollback: { format_version: 1, relative_path: `repair/${index}.json`, digest: digest(30 + index) },
    post_assertions: { target_check_id: 'pages.links.orphan_labels', target_evidence_id: digest(40 + index), general_baseline: [], deep_baseline: [], verification_policy: { kind: 'general_only' }, require_complete_general: true, reject_new_actionable: true, reject_new_incomplete: true, allowed_non_target_check_deltas: [] }, manifest_digest: digest(index),
  };
}
export function knowledgeEntries(): RepairPlanEntry[] {
  return [1, 2].map((n): RepairPlanEntry => ({ check_id: 'pages.links.orphan_labels', occurrence_digest: digest(100 + n), affected_records: [{ kind: 'page', durable_id: n === 1 ? 'page-architecture' : 'page-keyboard' }], resolution: { disposition: 'ready', manifest: knowledgeManifest(n) } })).concat([
    { check_id: 'pages.duplicate_active_titles', occurrence_digest: digest(103), affected_records: [{ kind: 'page', durable_id: 'page-cjk' }], resolution: { disposition: 'review', review_item: { review_id: 'knowledge-review-1', check_id: 'pages.duplicate_active_titles', issue: 'Two pages have the same title.', choices: ['rename', 'defer'], suggested_research_queries: [] } } },
    { check_id: 'pages.links.orphan_labels', occurrence_digest: digest(104), affected_records: [{ kind: 'page', durable_id: 'page-history' }], resolution: { disposition: 'blocked', blocked: { reason_code: 'target_missing', detail: 'No page matches this link.', next_action: 'Create the page or update the link in the original page.' } } },
  ]);
}
export function knowledgeLibrary() {
  const base = createSpacesNavigationFixture();
  const titles = ['Weekend plan', 'Packing list', 'Rainy-day route', 'Reading notes', 'Design decisions', 'City walk'];
  return { ...base, pages: base.pages.map((p, i) => ({ ...p, title: titles[i] ?? p.title, content: '# ' + (titles[i] ?? p.title) + '\n\nA fictional page for Knowledge Check verification.' })), refinements: [{ id: 'knowledge-review-1', action: 'lint_repair_review' as const, source_ids: ['page-cjk'], payload: { action: 'lint_repair_review' as const, check_id: 'pages.duplicate_active_titles', occurrence_digest: digest(103), owner_binding_digest: digest(105), issue: 'Two pages have the same title.', choices: [], suggested_research_queries: [] }, confidence: 1, created_at: '2026-10-09T00:00:00Z' }], distillReview: { ...base.distillReview, pending: [], stale_pages: [], orphan_topics: [] } };
}
export async function installKnowledgeProtocol(page: Page, mode: 'normal' | 'incomplete' | 'unknown' | 'verify-failed' | 'empty' | 'unstable' = 'normal') {
  await page.addInitScript(({ initialReport, entries, mode }) => {
    const original = window.__wenlanTauriInvoke;
    const w = window as typeof window & { __knowledgeCalls: Array<{ command: string; args: any }> };
    w.__knowledgeCalls = [];
    const read = () => JSON.parse(localStorage.getItem('qa-knowledge-protocol') || '{}') as Record<string, { apply: any; verified?: any }>;
    const write = (value: unknown) => localStorage.setItem('qa-knowledge-protocol', JSON.stringify(value));
    const idDigest = (n: number) => n.toString(16).padStart(64, '0');
    const manifestations = entries.flatMap((entry) => entry.resolution.disposition === 'ready' ? [entry.resolution.manifest] : []);
    const planId = 'knowledge-plan-1'; const planDigest = idDigest(900);
    window.__wenlanTauriInvoke = async (command, args) => {
      if (!command.startsWith('repair_')) return original(command, args);
      w.__knowledgeCalls.push({ command, args });
      const request = (args as any)?.request;
      const record = read();
      if (command === 'repair_lint') {
        if ((args as any)?.query?.profile !== 'general') throw new Error('Deep is deferred');
        const report = structuredClone(initialReport);
        if (mode === 'incomplete') { report.complete = false; report.totals.incomplete = 1; report.checks[0].outcome = 'failed_to_run'; }
        if (mode === 'empty' || mode === 'unstable') { report.totals.findings = 0; report.totals.actionable_findings = 0; report.totals.passed = 2; report.checks.forEach(c => {c.outcome = 'pass'; c.summary_code = 'check_passed';}); }
        if (mode === 'unstable') report.snapshots.pages.after_scan_digest = 'd'.repeat(16);
        return report;
      }
      if (command === 'repair_plan') return { plan_id: planId, plan_digest: planDigest, scope: { kind: 'global' }, entry_count: mode === 'empty' ? 0 : entries.length, deterministic_complete: true, semantic_complete: false };
      if (command === 'repair_plan_entries') return { plan_id: planId, plan_digest: planDigest, scope: { kind: 'global' }, offset: request.offset, next_offset: null, total_entries: mode === 'empty' ? 0 : entries.length, entries: mode === 'empty' ? [] : entries };
      if (command === 'repair_recovery') return null; // Existing review panel has no prior operation in this synthetic fixture.
      if (command === 'repair_validate_manifest') return true;
      const manifest = manifestations.find(m => m.manifest_id === request?.manifest_id || m.manifest_id === request?.apply?.manifest_id);
      if (!manifest) throw new Error('Unknown manifest');
      const id = manifest.manifest_id;
      if (command === 'repair_apply') {
        if (record[id]) throw new Error('Duplicate apply forbidden');
        const apply = { receipt_schema_version: 5, manifest_id: id, manifest_digest: manifest.manifest_digest, applied_at: Date.now(), before_target_receipt: idDigest(901), after_target_receipt: idDigest(902), non_target_before: idDigest(903), non_target_after: idDigest(903), actual_effects: manifest.allowed_effects, writer: manifest.writer, receipt_digest: idDigest(910 + Object.keys(record).length) };
        record[id] = { apply }; write(record);
        if (mode === 'unknown' && Object.keys(record).length === 1) throw new Error('Reply lost after commit');
        return apply;
      }
      if (command === 'repair_verify') {
        if (mode === 'verify-failed') throw new Error('Verification incomplete');
        const verified = { receipt_schema_version: 5, manifest_id: id, manifest_digest: manifest.manifest_digest, apply_receipt_digest: record[id].apply.receipt_digest, verified_at: Date.now(), general_snapshots: initialReport.snapshots, receipt_digest: idDigest(950 + Object.keys(record).length) };
        record[id].verified = verified; write(record); return verified;
      }
      if (command === 'repair_operation_status') return { manifest_id: id, manifest_digest: manifest.manifest_digest, state: record[id]?.verified ? { phase: 'verified', apply_receipt: record[id].apply, verification_receipt: record[id].verified } : record[id] ? { phase: 'applied_unverified', apply_receipt: record[id].apply } : { phase: 'prepared' } };
      if (command === 'repair_resume_runtime') return null;
      if (command === 'repair_cancel') return { manifest_id: id, manifest_digest: manifest.manifest_digest, state: { phase: 'cancelled', cancelled_at: Date.now() } };
      throw new Error(`Unexpected repair command: ${command}`);
    };
  }, { initialReport: knowledgeReport(), entries: knowledgeEntries(), mode });
}
