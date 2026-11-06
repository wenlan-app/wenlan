import { invokeDaemonCommand, type PageCitation } from "./tauri";

export interface PageExportSource { locator: string; source_kind: string; source_id: string; title: string; source_agent: string | null; url: string | null; text_digest: string | null; historical: boolean }
export interface PageExportBibliography { source_id: string; title: string; bibtex: string | null; availability: "available" | "missing" | "generation_mismatch" }
export interface PageExportBundle {
  id: string; digest: string; kind: "saved" | "pending_draft"; page_id: string; page_incarnation: string; space_id: string | null;
  version: number; export_revision: number; source_revision: number; title: string; content: string; last_modified: string; captured_at: number;
  citations: PageCitation[] | null; sources: PageExportSource[]; bibliography: PageExportBibliography[]; references_available: boolean; pending_revision_id: string | null;
}
export interface PageExportVersion {
  bundle_id: string | null; kind: "saved" | "pending_draft" | "legacy_unavailable"; version: number; export_revision: number | null;
  title: string | null; saved_at: string; captured_at: number | null; references_available: boolean; bibliography_available: boolean; unavailable_reason: string | null;
}
export const capturePageExport = (pageId: string, space?: string) => invokeDaemonCommand<PageExportBundle>("capture_page_export", { pageId, space });
export const listPageExportVersions = (pageId: string, space?: string) => invokeDaemonCommand<{ versions: PageExportVersion[] }>("list_page_export_versions", { pageId, space });
export const getPageExportBundle = (pageId: string, bundleId: string, space?: string) => invokeDaemonCommand<PageExportBundle>("get_page_export_bundle", { pageId, bundleId, space });
