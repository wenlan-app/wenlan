// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getAiMemoryImport, startAiMemoryImport, stopAiMemoryImport } from "../../../lib/tauri";
import SourceDialog from "./SourceDialog";

/** How each AI tool is named to the person. */
const TOOL_NAMES: Record<string, string> = { "claude-code": "Claude Code", codex: "Codex" };

/** What AI tools remember, in one place: the tools on this computer are
 *  followed one way once the person turns it on; tools whose memory lives in
 *  their cloud are pasted in once (the quick way). */
export default function AiMemoryDialog({ onClose, onPaste }: {
  onClose: () => void;
  /** The quick way: copy a prompt into the AI, paste its answer back. */
  onPaste?: () => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [failure, setFailure] = useState<string | null>(null);
  const fail = (error: unknown) => setFailure(error instanceof Error ? error.message : String(error));
  const status = useQuery({ queryKey: ["aiMemoryImport"], queryFn: getAiMemoryImport });
  const refresh = () => qc.invalidateQueries({ queryKey: ["aiMemoryImport"] });
  const start = useMutation({ mutationFn: startAiMemoryImport, onMutate: () => setFailure(null), onSuccess: refresh, onError: fail });
  const stop = useMutation({ mutationFn: stopAiMemoryImport, onMutate: () => setFailure(null), onSuccess: refresh, onError: fail });

  const found = status.data?.found ?? [];
  const following = status.data?.import != null;
  const busy = start.isPending || stop.isPending;
  return <SourceDialog title={t("sourceAccess.aiMemory")} onClose={onClose}>
    <section className="source-ai-memory" data-testid="ai-memory-here">
      <h3>{t("sourceAccess.aiMemoryHere")}</h3>
      {status.isPending ? null : found.length === 0 ? <p>{t("sourceAccess.aiMemoryNone")}</p> : <>
        <p>{found.map((f) => t("sourceAccess.aiMemoryToolCount", { tool: TOOL_NAMES[f.tool] ?? f.tool, count: f.memories })).join(t("sourceAccess.aiMemoryListJoin"))}</p>
        <p>{t("sourceAccess.aiMemoryFollows")}</p>
        {following
          ? <div className="source-ai-memory-row"><span role="status">{t(start.isPending ? "sourceAccess.aiMemoryReading" : "sourceAccess.aiMemoryFollowing")}</span>
            <button type="button" className="page-editor-action" disabled={busy} onClick={() => stop.mutate()}>{t("sourceAccess.aiMemoryStop")}</button></div>
          : !status.data?.ready
            ? <p>{t("sourceAccess.aiMemoryNoModel")}</p>
            : <button type="button" className="page-editor-action" data-testid="ai-memory-start" disabled={busy} onClick={() => start.mutate()}>{t(start.isPending ? "sourceAccess.aiMemoryReading" : "sourceAccess.aiMemoryStart")}</button>}
      </>}
      {failure && <div role="alert" className="source-dialog-error">{t("sourceAccess.aiMemoryFailed")}<p>{failure}</p></div>}
    </section>
    {onPaste && <section className="source-ai-memory" data-testid="ai-memory-cloud">
      <h3>{t("sourceAccess.aiMemoryCloud")}</h3>
      <p>{t("sourceAccess.aiMemoryCloudHint")}</p>
      <button type="button" className="page-editor-action" onClick={() => { onClose(); onPaste(); }}>{t("sourceAccess.aiMemoryPaste")}</button>
    </section>}
  </SourceDialog>;
}
