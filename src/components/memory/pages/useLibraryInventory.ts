// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import {
  EXPLICIT_BROWSE_QUERY_POLICY,
  listAllActivePagesExplicitBrowse,
  listAllDraftPagesExplicitBrowse,
} from "./listAllPages";
import { collectPageInventory } from "./pageInventory";

/**
 * The library's notes as the Wiki counts them: active pages plus drafts.
 *
 * `ready` is true only once both lists loaded, and `empty` additionally needs
 * zero notes, so a slow or failed query never reads as a new library. Every
 * surface that branches on "empty library" (the reading-start panel and the
 * single-column workspace) reads it here, so they share one query cache entry
 * and cannot disagree.
 */
export function useLibraryInventory() {
  const activePagesQuery = useQuery({
    queryKey: ["pages", "active"],
    queryFn: listAllActivePagesExplicitBrowse,
    ...EXPLICIT_BROWSE_QUERY_POLICY,
  });
  const draftPagesQuery = useQuery({
    queryKey: ["pages", "draft"],
    queryFn: listAllDraftPagesExplicitBrowse,
    ...EXPLICIT_BROWSE_QUERY_POLICY,
  });
  const pages = collectPageInventory(activePagesQuery.data ?? [], draftPagesQuery.data ?? []);
  const ready = activePagesQuery.isSuccess && draftPagesQuery.isSuccess;
  return { pages, ready, empty: ready && pages.length === 0 };
}
