// SPDX-License-Identifier: AGPL-3.0-only
import type { QueryClient } from "@tanstack/react-query";
import { deletePage, discardPageDraft, getPage, type Page } from "../../../lib/tauri";

/**
 * Delete a page from a list row (folder tree or Recently used). A published
 * page goes through delete_page; a draft is discarded at its latest version,
 * since delete_page rejects drafts.
 */
export async function deleteWikiPage(
  queryClient: QueryClient,
  page: Pick<Page, "id" | "status">,
): Promise<void> {
  const { id } = page;
  if (page.status === "draft") {
    // An open editor can autosave between the read and the discard, so a
    // version conflict reads the version again once.
    for (let attempt = 0; ; attempt++) {
      const latest = await getPage(id);
      if (!latest) break;
      try { await discardPageDraft({ id, expectedVersion: latest.version }); break; }
      catch (cause) {
        const code = cause && typeof cause === "object" && "code" in cause ? (cause as { code?: unknown }).code : null;
        if (code === "page_draft_not_found") break;
        if (code !== "draft_version_conflict" || attempt > 0) throw cause;
      }
    }
  } else await deletePage(id);
  void queryClient.invalidateQueries({ queryKey: ["page", id], refetchType: "none" });
  void queryClient.invalidateQueries({ queryKey: ["pages"] });
  void queryClient.invalidateQueries({ queryKey: ["knowledge-graph"] });
  void queryClient.invalidateQueries({ queryKey: ["page-links", id], refetchType: "none" });
  void queryClient.invalidateQueries({ queryKey: ["page-revisions", id], refetchType: "none" });
  void queryClient.invalidateQueries({ queryKey: ["page-sources", id], refetchType: "none" });
}
