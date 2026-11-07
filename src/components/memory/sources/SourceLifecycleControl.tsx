// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { DotsThree } from "@phosphor-icons/react";
import { getSourceLifecycle, setSourceLifecycle, sourceLifecycleKey, type SourceLifecycleResponse, type SourceLifecycleState } from "../../../lib/sourceLifecycleApi";
import { getPage } from "../../../lib/tauri";
import SourceDialog from "./SourceDialog";
import "./SourceLifecycleControl.css";

export function SourceLifecycleControl({ space, source, sourceId }: { space?: string; source: string; sourceId: string }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [menu, setMenu] = useState(false);
  const [open, setOpen] = useState(false);
  const [state, setState] = useState<SourceLifecycleState>("active");
  const [reason, setReason] = useState("");
  const [saved, setSaved] = useState<SourceLifecycleResponse | null>(null);
  const [failure, setFailure] = useState(false);
  const query = useQuery({ queryKey: sourceLifecycleKey(space, source, sourceId), queryFn: () => getSourceLifecycle(space!, source, sourceId), enabled: !!space && open && ["file", "webpage"].includes(source), retry: false });
  const impacts = saved ? saved.impacts : query.data?.preview_impacts ?? [];
  const titles = useQueries({ queries: impacts.map(impact => ({ queryKey: ["source-impact-page", space, impact.page_id], queryFn: () => getPage(impact.page_id), enabled: open, retry: false, staleTime: 0, refetchOnMount: "always" as const })) });
  useEffect(() => { if (open && query.data) setState(query.data.record.state); }, [open, query.data?.record.revision]);
  const save = useMutation({ mutationFn: () => setSourceLifecycle({ space: space!, source, source_id: sourceId, expected_revision: query.data!.record.revision, state, reason: reason.trim() }),
    onSuccess: async result => {
      setFailure(false); setSaved(result);
      client.setQueryData(sourceLifecycleKey(space, source, sourceId), result);
      await Promise.all([client.invalidateQueries({ queryKey: ["page"] }), client.invalidateQueries({ queryKey: ["page-sources"] }), client.invalidateQueries({ queryKey: ["pages"] }), client.invalidateQueries({ queryKey: ["refinement-proposals"] })]);
    },
    onError: async () => { setFailure(true); setSaved(null); await query.refetch(); },
  });
  if (!space || !["file", "webpage"].includes(source)) return null;
  return <div className="source-lifecycle-control">
    <button type="button" className="mem-icon-action" aria-label={t("sourceLifecycle.options")} title={t("sourceLifecycle.options")} aria-expanded={menu} onClick={() => setMenu(!menu)}><DotsThree size={20} aria-hidden="true" /></button>
    {menu && <div className="source-lifecycle-menu"><button type="button" className="page-editor-action" onClick={() => { setMenu(false); setOpen(true); setSaved(null); setFailure(false); setReason(""); }}>{t("sourceLifecycle.changeStatus")}</button></div>}
    {open && <SourceDialog title={t("sourceLifecycle.changeStatus")} onClose={() => setOpen(false)}>
      {query.isPending ? <p role="status">{t("sourceLifecycle.loading")}</p> : query.isError || !query.data ? <p role="alert">{t("sourceLifecycle.loadFailed")} <button type="button" className="page-editor-action" onClick={() => void query.refetch()}>{t("sourceAccess.retry")}</button></p> : <>
        <p>{t("sourceLifecycle.current")}: {t(`sourceLifecycle.${query.data.record.state}`)}</p>
        {query.data.record.reason && <p>{t("sourceLifecycle.currentReason")}: {query.data.record.reason}</p>}
        <label>{t("sourceLifecycle.newStatus")}<select value={state} onChange={event => { setState(event.target.value as SourceLifecycleState); setSaved(null); }} disabled={save.isPending}>
          {(["active", "excluded", "retracted", "removed"] as const).map(value => <option key={value} value={value}>{t(`sourceLifecycle.${value}`)}</option>)}
        </select></label>
        <label>{t("sourceLifecycle.reason")}<textarea value={reason} onChange={event => { setReason(event.target.value); setSaved(null); }} maxLength={4000} disabled={save.isPending} /></label>
        {failure && <p role="alert">{t("sourceLifecycle.saveFailed")}</p>}
        {saved && <p role="status">{t("sourceLifecycle.saved")}</p>}
        <div><p>{t("sourceLifecycle.affectedCount", { count: impacts.length })}</p><ul>{impacts.map((impact, index) => {
          const page = titles[index];
          const title = page?.isSuccess && !page.isFetching && page.data?.space === space && page.data.title ? page.data.title : t(page?.isFetching ? "sourceLifecycle.pageLoading" : "sourceLifecycle.pageUnavailable");
          return <li key={impact.page_id}>{title}</li>;
        })}</ul></div>
        <div className="source-dialog-actions"><button type="button" className="page-editor-action" onClick={() => setOpen(false)}>{t("common.close")}</button><button type="button" className="page-editor-action" disabled={save.isPending || !reason.trim() || !!saved} onClick={() => save.mutate()}>{t("sourceLifecycle.save")}</button></div>
      </>}
    </SourceDialog>}
  </div>;
}
