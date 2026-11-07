// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPage, getZoteroDisconnectPreview, removeSource, type ZoteroDisconnectPreview } from "../../../lib/tauri";
import SourceDialog from "./SourceDialog";

export default function ZoteroDisconnectDialog({ id, onClose }: { id: string; onClose: () => void }) {
  const {t} = useTranslation();
  const client = useQueryClient();
  const [failed, setFailed] = useState(false);
  const preview = useQuery({ queryKey:["zotero-disconnect-preview",id], queryFn:() => getZoteroDisconnectPreview(id), retry:false, gcTime:0, staleTime:0, refetchOnMount:"always", refetchOnWindowFocus:false });
  const data = preview.data;
  const valid = !!data && data.identity?.registration_id === id && !!data.space_name && !!data.identity_digest && Number.isSafeInteger(data.revision) && typeof data.impact_scope_known === "boolean" && Array.isArray(data.canonical_item_ids) && Array.isArray(data.affected_page_ids) && data.affected_page_ids.every(value => typeof value === "string");
  const ready = valid && preview.isSuccess && !preview.isFetching;
  const pages = useQueries({ queries: (ready && data?.impact_scope_known ? data.affected_page_ids : []).map(pageId => ({queryKey:["zotero-disconnect-page",data?.space_name,pageId],queryFn:() => getPage(pageId),retry:false,staleTime:0,refetchOnMount:"always" as const})) });
  const mutation = useMutation({ mutationFn:(expected:ZoteroDisconnectPreview) => removeSource(id, expected), onSuccess: async () => { await client.invalidateQueries({queryKey:["registeredSources"]}); onClose(); }, onError: async () => { setFailed(true); await preview.refetch(); } });
  const close = () => { if (!mutation.isPending) onClose(); };
  return <SourceDialog title={ready ? t("zoteroDisconnect.title", {space:data.space_name}) : t("zoteroDisconnect.titleLoading")} onClose={close}>
    <p>{t("zoteroDisconnect.description")}</p>
    {(preview.isPending || preview.isFetching) && <p role="status">{t("zoteroDisconnect.loading")}</p>}
    {(preview.isError || preview.isSuccess && !valid) && <p role="alert">{t("zoteroDisconnect.failed")} <button type="button" className="page-editor-action" onClick={() => void preview.refetch()}>{t("zoteroDisconnect.retry")}</button></p>}
    {ready && <>{data.impact_scope_known ? <><p>{t("zoteroDisconnect.papers",{count:data.canonical_item_ids.length})}</p><p>{t("zoteroDisconnect.pages",{count:data.affected_page_ids.length})}</p>
      <ul>{data.affected_page_ids.map((id,index) => { const query=pages[index]; const title=query?.isSuccess && !query.isFetching && query.data?.space === data.space_name && query.data.title && query.data.title !== id ? query.data.title : t(query?.isFetching ? "zoteroDisconnect.pageLoading" : "zoteroDisconnect.unavailable"); return <li key={id}>{title}</li>; })}</ul></> : <p>{t("zoteroDisconnect.impactUnknown")}</p>}
      {data.pending_disconnect && <p role="status">{t("zoteroDisconnect.pending")}</p>}{data.disconnected && <p role="status">{t("zoteroDisconnect.disconnected")}</p>}
    </>}
    {failed && <p role="alert">{t("zoteroDisconnect.saveFailed")}</p>}
    <div className="source-dialog-actions"><button type="button" className="page-editor-action" onClick={close} disabled={mutation.isPending}>{t("zoteroDisconnect.cancel")}</button><button type="button" className="page-editor-action" disabled={!ready || mutation.isPending} onClick={() => { if (ready) { setFailed(false); mutation.mutate(data); } }}>{t(mutation.isPending ? "zoteroDisconnect.disconnecting" : "zoteroDisconnect.disconnect")}</button></div>
  </SourceDialog>;
}
