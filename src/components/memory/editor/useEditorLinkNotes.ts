// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { loadAllLinkCandidates, linkNotesFor } from "../page/linkCandidates";
/** Reuse page resolution/ambiguity rules; load only after a writer types [[. */
export function useEditorLinkNotes(space: string | null, id: string, requested: boolean) {
  const { data } = useQuery({queryKey:["pages","link-notes",space],queryFn:()=>loadAllLinkCandidates(space),enabled:requested,staleTime:30_000,retry:false});
  return useMemo(()=>linkNotesFor({page:{id,creation_kind:"authored"},ownSpace:space,spaces:data??[]})
    // These surfaces insert links, but do not own a page-basis mutation.
    .map(note=>({...note,canUseAsSource:false})),[data,id,space]);
}
