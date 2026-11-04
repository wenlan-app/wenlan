import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { addZoteroSource, listSpaces, listZoteroCollections, syncRegisteredSource, type SyncStats } from "../../../lib/tauri";
import SourceDialog from "./SourceDialog";

export default function ZoteroSourceDialog({ onClose, fixedSpace }: { onClose: () => void; fixedSpace?: string }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const collections = useQuery({ queryKey: ["zoteroCollections"], queryFn: listZoteroCollections, retry: false });
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces, enabled: fixedSpace === undefined, retry: false });
  const [collectionKey, setCollectionKey] = useState("");
  const [space, setSpace] = useState(fixedSpace ?? "");
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SyncStats | null>(null);
  useEffect(() => {
    if (!collectionKey && collections.data?.collections.length) setCollectionKey(collections.data.collections[0].key);
  }, [collectionKey, collections.data]);
  useEffect(() => {
    if (!fixedSpace && !space && spaces.data?.length) setSpace(spaces.data[0].name);
  }, [fixedSpace, space, spaces.data]);
  const connect = useMutation({
    mutationFn: async () => {
      setError(null); setResult(null);
      const registered = await addZoteroSource(collectionKey, collections.data?.server_id ?? null, space);
      await client.invalidateQueries({ queryKey: ["registeredSources"] });
      const stats = await syncRegisteredSource(registered.id);
      return stats;
    },
    onSuccess: async (stats) => {
      setResult(stats);
      await Promise.all([
        client.invalidateQueries({ queryKey: ["registeredSources"] }),
        client.invalidateQueries({ queryKey: ["space-sources"] }),
        client.invalidateQueries({ queryKey: ["indexedFiles"] }),
      ]);
    },
    onError: (cause) => setError(cause instanceof Error ? cause.message : String(cause)),
  });
  const ready = !!collectionKey && !!space && !connect.isPending;
  return <SourceDialog title={t("zoteroSource.title")} onClose={onClose}>
    <p>{t("zoteroSource.description")}</p>
    <div className="source-dialog-status" role={collections.isError ? "alert" : "status"}>
      {collections.isPending ? t("zoteroSource.checking") : collections.isError ? t("zoteroSource.offline") : t("zoteroSource.connected")}
      {collections.isError && <button type="button" className="page-editor-action" onClick={() => void collections.refetch()}>{t("sourceAccess.retry")}</button>}
    </div>
    {collections.isSuccess && <label>{t("zoteroSource.collection")}<select disabled={connect.isPending} value={collectionKey} onChange={e => { setCollectionKey(e.target.value); setResult(null); setError(null); }}>
      {collections.data.collections.map(item => <option key={item.key} value={item.key}>{item.name} ({item.item_count})</option>)}
    </select></label>}
    {!fixedSpace && <label>{t("zoteroSource.destination")}<select disabled={connect.isPending} value={space} onChange={e => { setSpace(e.target.value); setResult(null); setError(null); }}>
      {(spaces.data ?? []).map(item => <option key={item.id} value={item.name}>{item.name}</option>)}
    </select></label>}
    {collections.isSuccess && collections.data.collections.length === 0 && <p role="status">{t("zoteroSource.noCollections")}</p>}
    {spaces.isError && <p role="alert">{t("zoteroSource.spacesFailed")} <button type="button" className="page-editor-action" onClick={() => void spaces.refetch()}>{t("sourceAccess.retry")}</button></p>}
    {error && <p role="alert" className="source-dialog-error">{error}</p>}
    {result && <p role={result.errors ? "alert" : "status"}>
      {t("zoteroSource.summary", { ingested: result.ingested, skipped: result.skipped, errors: result.errors })}
      {result.error_detail && <span className="block whitespace-pre-wrap">{result.error_detail}</span>}
    </p>}
    {result?.skipped_files?.length ? <ul>{result.skipped_files.map((item, index) => <li key={`${item.path}:${index}`}>{item.path}: {item.reason}</li>)}</ul> : null}
    <div className="source-dialog-actions">
      <button type="button" className="page-editor-action" onClick={onClose}>{result || connect.isPending ? t("common.close") : t("sourceAccess.cancel")}</button>
      <button type="button" className="page-editor-action" disabled={!ready || collections.isError || (!fixedSpace && (spaces.isPending || spaces.isError)) || !collections.data?.collections.length} onClick={() => connect.mutate()}>
        {connect.isPending ? t("zoteroSource.importing") : result || error ? t("zoteroSource.importAgain") : t("zoteroSource.import")}
      </button>
    </div>
  </SourceDialog>;
}
