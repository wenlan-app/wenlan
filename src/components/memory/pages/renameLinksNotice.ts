// SPDX-License-Identifier: AGPL-3.0-only
import type { QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { toast } from "sonner";
import type { RenamePageResponse } from "../../../lib/tauri";
import { recordRenamedLinks } from "./renameLinkRewrite";

/** The link counts a rename, or a restore under a new title, reports. */
type LinkCounts = Pick<RenamePageResponse, "id" | "title" | "links_updated" | "links_pending_review" | "links_not_updated">;

/** What a rename did to other pages' links, in one line; null when it touched none. */
export function renameLinksMessage(t: TFunction, renamed: LinkCounts): string | null {
  const parts: string[] = [];
  const updated = renamed.links_updated ?? 0;
  const pending = renamed.links_pending_review ?? 0;
  const notUpdated = renamed.links_not_updated ?? 0;
  if (updated > 0) parts.push(t("pageDetail.renameLinksUpdated", { count: updated }));
  if (pending > 0) parts.push(t("pageDetail.renameLinksPendingReview", { count: pending }));
  if (notUpdated > 0) parts.push(t("pageDetail.renameLinksNotUpdated", { count: notUpdated }));
  return parts.length > 0 ? parts.join(" ") : null;
}

/** After a rename: refresh pages whose links the daemon rewrote, so an open
 *  editor sees the newer version, and tell the person what changed. The
 *  rename is recorded first, so an editor holding unsaved text on one of those
 *  pages takes the link rewrite into its draft instead of a conflict. */
export function finishRenameLinks(
  t: TFunction,
  queryClient: QueryClient,
  renamed: LinkCounts,
  previousTitle: string,
): void {
  if ((renamed.links_updated ?? 0) > 0 || (renamed.links_pending_review ?? 0) > 0) {
    recordRenamedLinks(previousTitle, renamed.title);
    for (const queryKey of [["page"], ["page-revisions"], ["pending-revisions"]]) {
      void queryClient.invalidateQueries({ queryKey });
    }
  }
  const message = renameLinksMessage(t, renamed);
  if (message) toast(message);
}
