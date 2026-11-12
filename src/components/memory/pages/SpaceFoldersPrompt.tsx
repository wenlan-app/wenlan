// SPDX-License-Identifier: AGPL-3.0-only
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { getSpaceFolders, setSpaceFolders } from "../../../lib/tauri";
import { KNOWLEDGE_FOLDERS_QUERY_KEY } from "./useKnowledgeFolders";

export const SPACE_FOLDERS_QUERY_KEY = ["space-folders"] as const;

/** Asks once whether each Space's pages go in a folder of their own.
 *  Nothing moves until the person says yes. Shown only once the Wiki lists
 *  pages; an older daemon (or the fixture-only preview) that cannot answer
 *  shows nothing. */
export function SpaceFoldersPrompt({ noteCount }: { readonly noteCount: number | null }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data, isSuccess } = useQuery({ queryKey: SPACE_FOLDERS_QUERY_KEY, queryFn: getSpaceFolders, retry: false, staleTime: Infinity });
  const choose = useMutation({
    mutationFn: setSpaceFolders,
    onSuccess: (_, on) => {
      queryClient.setQueryData(SPACE_FOLDERS_QUERY_KEY, on);
      void queryClient.invalidateQueries({ queryKey: KNOWLEDGE_FOLDERS_QUERY_KEY });
    },
    onError: () => { toast.error(t("pages.folders.spaceFoldersFailed")); },
  });
  const unasked = isSuccess && data === null;
  if (!unasked || noteCount === null || noteCount === 0) return null;
  return (
    <div className="wiki-space-folders-prompt" role="group" aria-label={t("pages.folders.spaceFoldersAsk")}>
      <p className="wiki-space-folders-ask">{t("pages.folders.spaceFoldersAsk")}</p>
      <p className="wiki-space-folders-hint">{t("pages.folders.spaceFoldersHint")}</p>
      <div className="wiki-space-folders-actions">
        <button type="button" className="wiki-space-folders-yes" disabled={choose.isPending} onClick={() => choose.mutate(true)}>{t("pages.folders.spaceFoldersYes")}</button>
        <button type="button" className="wiki-space-folders-no" disabled={choose.isPending} onClick={() => choose.mutate(false)}>{t("pages.folders.spaceFoldersNo")}</button>
      </div>
    </div>
  );
}
