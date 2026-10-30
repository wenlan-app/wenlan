// SPDX-License-Identifier: AGPL-3.0-only
import type { QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { toast } from "sonner";
import { confirmAction } from "../../../lib/confirmAction";
import { archivePage, type Page } from "../../../lib/tauri";

/** Same rule as the daemon: only an active ordinary page can be archived. */
export function canArchivePage(page: Pick<Page, "status" | "creation_kind"> | null | undefined): boolean {
  return !!page && page.status === "active" && page.creation_kind !== "entity";
}

/**
 * Refresh everything that lists or links a page whose status changed (archived
 * or restored). Unlike a delete, the page still exists: an open tab refetches
 * it and turns read only, or back to editable.
 */
export async function refreshPageStatus(queryClient: QueryClient, id: string): Promise<void> {
  await Promise.all([
    ["page", id], ["pages"], ["recent-pages"], ["searchPages"], ["search"],
    ["knowledge-graph"], ["constellation-cartography"], ["space-pages"],
    ["spaces-page-counts"], ["sidebar-space-page-counts"], ["page-links"], ["page-revisions", id],
  ].map(queryKey => queryClient.invalidateQueries({ queryKey })));
}

/**
 * Archive a page from a list row (folder tree or Recently used). The page
 * keeps its id, body and history; it leaves the lists and can be restored
 * from Archived pages. The caller owns the confirmation.
 */
export async function archiveWikiPage(
  queryClient: QueryClient,
  page: Pick<Page, "id" | "status" | "creation_kind">,
): Promise<void> {
  if (!canArchivePage(page)) throw new Error("only active ordinary pages can be archived");
  const { id } = page;
  await archivePage(id);
  // A tab that mounts before the refetch answers must already read archived,
  // so it never opens an editor on a page the daemon will refuse to save.
  queryClient.setQueryData<Page | null>(["page", id], current =>
    current && current.id === id ? { ...current, status: "archived" } : current);
  void refreshPageStatus(queryClient, id).catch(() => undefined);
}

export interface ListArchiveOptions {
  readonly t: TFunction;
  /** Resolves true when an open editor took the request (see WikiWorkspace). */
  readonly onArchiveOpenPage?: (pageId: string) => Promise<boolean>;
  readonly onArchived?: (pageId: string) => void;
}

// One list archive at a time across both lists: each menu closes before its
// confirmation, so the guard cannot live in the menu's own state.
let listArchivePending = false;

/**
 * The Archive page item of a list row's menu. A page that is open in an
 * editor is archived by that editor (it saves first and owns the action lock);
 * any other page is confirmed and archived here.
 */
export async function archivePageFromList(
  queryClient: QueryClient,
  page: Pick<Page, "id" | "status" | "creation_kind">,
  { t, onArchiveOpenPage, onArchived }: ListArchiveOptions,
): Promise<boolean> {
  if (listArchivePending || !canArchivePage(page)) return false;
  listArchivePending = true;
  try {
    if (await onArchiveOpenPage?.(page.id)) return false;
    if (!await confirmAction(t("pageDetail.archiveConfirm"), {
      confirm: t("pageDetail.archivePage"),
      cancel: t("common.cancel"),
    })) return false;
    await archiveWikiPage(queryClient, page);
    onArchived?.(page.id);
    return true;
  } catch {
    toast(t("pageDetail.archiveError"));
    return false;
  } finally { listArchivePending = false; }
}
