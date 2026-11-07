// SPDX-License-Identifier: AGPL-3.0-only
import { invokeDaemonCommand as invoke } from "./tauri";

export type SourceLifecycleState = "active" | "excluded" | "retracted" | "removed";
export interface SourceLifecycleRecord { space_id: string; source: string; source_id: string; revision: number; state: SourceLifecycleState; reason: string; actor: string; updated_at: number }
export interface SourceLifecycleImpact { page_id: string; page_version: number; source_revision: number; source_ids: string[]; occurrences: number[]; reason: string }
export interface SourceLifecycleResponse { record: SourceLifecycleRecord; history: SourceLifecycleRecord[]; impacts: SourceLifecycleImpact[]; preview_impacts?: SourceLifecycleImpact[] }
export interface SourceLifecycleRequest { space: string; source: string; source_id: string; expected_revision: number; state: SourceLifecycleState; reason: string }

export const sourceLifecycleKey = (space: string | undefined, source: string, sourceId: string) => ["source-lifecycle", space ?? null, source, sourceId] as const;
export const getSourceLifecycle = (space: string, source: string, sourceId: string) => invoke<SourceLifecycleResponse>("get_source_lifecycle", { space, source, sourceId });
export const setSourceLifecycle = (request: SourceLifecycleRequest) => invoke<SourceLifecycleResponse>("set_source_lifecycle", { request });
export interface ResolvedSourceReference { space_id: string; source: "file" | "webpage"; source_id: string; source_agent: string | null; title: string }
export const resolveSourceReference = (space: string, locator: string) => invoke<ResolvedSourceReference>("resolve_source_reference", { space, locator });
