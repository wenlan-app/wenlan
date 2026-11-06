// SPDX-License-Identifier: AGPL-3.0-only
import { invokeDaemonCommand as invoke } from "./tauri";

export interface ResearchSourceRef { source: string; source_id: string; title: string }
export interface OrganizeRules { space_id: string; revision: number; name: string; instructions: string; content_digest: string }
export interface OrganizeRunItem extends ResearchSourceRef { coverage: "pending" | "reading" | "complete" | "failed" | "unavailable"; content_digest: string; decision_revision: number; lifecycle_revision?: number; served_chars?: number; served_total_chars?: number | null; next_offset: number | null; error: string | null }
export interface OrganizeRun { id: string; space_id: string; revision: number; rules: OrganizeRules; items: OrganizeRunItem[]; target_page_id: string | null; target_page_version: number | null; target_page_incarnation?: string | null; result_page_version: number | null; pending_revision_id: string | null; state: "prepared" | "in_progress" | "partial" | "awaiting_review" | "completed" | "cancelled"; created_at: number; updated_at: number }
export interface ClassificationChange { source: string; source_id: string; source_content_digest: string; expected_decision_revision: number; add_tags: string[]; remove_tags: string[]; reason: string; before_tags?: string[]; title?: string }
export interface ClassificationPayload { action: "source_classification"; run_id: string; space_id: string; rules_digest: string; changes: ClassificationChange[] }
export const getResearchRules = (space: string) => invoke<OrganizeRules>("get_research_rules", { space });
export const saveResearchRules = (request: { space: string; expected_revision: number; name: string; instructions: string }) => invoke<OrganizeRules>("save_research_rules", { request });
export const listResearchRuns = (space: string) => invoke<{ runs: OrganizeRun[] }>("list_research_runs", { space });
export const createResearchRun = (request: { space: string; sources: ResearchSourceRef[]; instructions: string | null; target_page_id: string | null }) => invoke<OrganizeRun>("create_research_run", { request });
export const getResearchRun = (space: string, id: string) => invoke<OrganizeRun>("get_research_run", { space, id });
export const acceptClassification = (id: string, classificationChanges: ClassificationChange[], space?: string) => invoke<unknown>("accept_research_classification", { id, changes: classificationChanges, space });
export const mergeResearchTags = (request: { space: string; sources: ResearchSourceRef[]; from_tag: string; into_tag: string }) => invoke<{ updated: number }>("merge_research_tags", { request });

export const resumeResearchRun = (space: string, id: string, expectedRevision: number) => invoke<OrganizeRun>("update_research_run", { id, request: { space, expected_revision: expectedRevision, resume: true } });
