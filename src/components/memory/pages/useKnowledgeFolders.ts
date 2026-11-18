// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { knowledgeFoldersList } from "../../../lib/tauri";

export const KNOWLEDGE_FOLDERS_QUERY_KEY = ["knowledge-folders"] as const;
export function useKnowledgeFolders({ enabled = true }: { readonly enabled?: boolean } = {}) {
  return useQuery({ queryKey: KNOWLEDGE_FOLDERS_QUERY_KEY, queryFn: () => knowledgeFoldersList(), staleTime: 30_000, retry: false, enabled });
}
