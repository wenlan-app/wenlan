// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listAllTags, patchDocumentTag, type IndexedFileInfo } from "../../../lib/tauri";
import { useLiveAgents } from "../../connect/connectionState";
import SourceDialog from "./SourceDialog";
import { connectedTools } from "../askAi/WritePages";
import { handToTool, opensWithText, type HandOver } from "../askAi/askInTool";
import { ToolPicker } from "../askAi/ToolPicker";
import { documentTagKey, selectedSourcePrompt } from "./sourceOrganize";
import { readOrganizeDraft, writeOrganizeDraft } from "../space-detail/sourceSelectionState";

type Mode = "organize" | "tags";
const PREF_PREFIX = "wenlan:source-organize:";

interface TagBatch {
  tag: string;
  action: "add" | "remove";
  failed: IndexedFileInfo[];
  saved: number;
  total: number;
}

export function SelectedSourceActions({ spaceId, spaceName, sources, mode, onClose }: {
  spaceId: string;
  spaceName: string;
  sources: readonly IndexedFileInfo[];
  mode: Mode;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const agents = useLiveAgents();
  const tools = agents.isSuccess ? connectedTools(agents.data).filter(tool => ["claude-code", "codex", "cursor", "claude-desktop", "continue", "copilot-cli", "gemini-cli", "kimi-code", "kiro", "muse-code", "opencode", "windsurf"].includes(tool.family)) : [];
  const [toolFamily, setToolFamily] = useState<string | null>(null);
  useEffect(() => {
    if (!toolFamily || !tools.some((tool) => tool.family === toolFamily)) setToolFamily(tools[0]?.family ?? null);
  }, [tools, toolFamily]);
  const tool = tools.find((candidate) => candidate.family === toolFamily) ?? null;
  const [instructions, setInstructions] = useState(() => {
    const draft = readOrganizeDraft(spaceId);
    if (draft !== null) return draft;
    try { return localStorage.getItem(`${PREF_PREFIX}${spaceName}`) ?? ""; } catch { return ""; }
  });
  const [remember, setRemember] = useState(false);
  const [tag, setTag] = useState("");
  const [tagAction, setTagAction] = useState<"add" | "remove">("add");
  const [working, setWorking] = useState(false);
  const [handover, setHandover] = useState<HandOver | null>(null);
  const [batch, setBatch] = useState<TagBatch | null>(null);
  const tagData = useQuery({ queryKey: ["tags"], queryFn: listAllTags, enabled: mode === "tags", retry: false });
  const prompt = selectedSourcePrompt(spaceName, sources.map(({ source, source_id, title }) => ({ source, source_id, title })), instructions);

  const saveInstructions = () => {
    if (!remember) return;
    try {
      if (instructions.trim()) localStorage.setItem(`${PREF_PREFIX}${spaceName}`, instructions.trim());
      else localStorage.removeItem(`${PREF_PREFIX}${spaceName}`);
    } catch { /* Optional local preference. */ }
  };
  const handOffPrompt = async () => {
    setWorking(true);
    const result = await handToTool(tool?.family ?? "other", prompt);
    setHandover(result);
    setWorking(false);
    if (result !== "failed") saveInstructions();
  };
  const applyTagBatch = async (nextTag: string, nextAction: "add" | "remove", pending: IndexedFileInfo[], total = pending.length, priorSaved = 0) => {
    setWorking(true);
    setBatch(null);
    let saved = priorSaved;
    const errors: IndexedFileInfo[] = [];
    for (const file of pending) {
      try { await patchDocumentTag(file.source, file.source_id, nextTag, nextAction); saved += 1; }
      catch { errors.push(file); }
    }
    setBatch({ tag: nextTag, action: nextAction, failed: errors, saved, total });
    await client.invalidateQueries({ queryKey: ["tags"] });
    setWorking(false);
  };
  const startTagBatch = () => {
    const normalized = tag.trim().toLowerCase();
    if (!normalized) return;
    void applyTagBatch(normalized, tagAction, [...sources]);
  };
  const retryTagBatch = () => {
    if (batch?.failed.length) void applyTagBatch(batch.tag, batch.action, [...batch.failed], batch.total, batch.saved);
  };
  const clearBatch = () => setBatch(null);
  const title = mode === "organize" ? t("selectedSources.organizeTitle", { count: sources.length }) : t("selectedSources.tagsTitle", { count: sources.length });

  return <SourceDialog title={title} onClose={onClose}>
    {mode === "organize" ? <div className="space-y-3">
      <p>{t("selectedSources.organizeHint")}</p>
      <ul aria-label={t("selectedSources.selectedList")}>{sources.map((file) => <li key={`${file.source}:${file.source_id}`}>{file.title}</li>)}</ul>
      <label className="block">{t("selectedSources.instructions")}<textarea value={instructions} onChange={(event) => { setInstructions(event.target.value); writeOrganizeDraft(spaceId, event.target.value); setHandover(null); }} rows={4} disabled={working} /></label>
      <label className="flex items-center gap-2" style={{ flexDirection: "row", alignItems: "center" }}><input type="checkbox" style={{ width: "auto", flexShrink: 0, padding: 0 }} checked={remember} disabled={working} onChange={(event) => setRemember(event.target.checked)} /> {t("selectedSources.remember")}</label>
      {tools.length > 1 && tool ? <ToolPicker tools={tools} current={tool.family} onChoose={(family) => { setToolFamily(family); setHandover(null); }} /> : null}
      <p>{t(tool && opensWithText(tool.family) ? "selectedSources.externalOpens" : "selectedSources.externalCopies", { tool: tool?.name ?? t("selectedSources.anyExternalTool") })}</p>
      <button type="button" className="page-editor-action" disabled={working} aria-live="polite" onClick={() => void handOffPrompt()}>{handover === "opened" ? t("askAi.handOver.opened", { tool: tool?.name ?? t("selectedSources.anyExternalTool") }) : handover === "copied" ? t("askAi.handOver.copied", { tool: tool?.name ?? t("selectedSources.anyExternalTool") }) : handover === "failed" ? t("askAi.handOver.failed") : tool && opensWithText(tool.family) ? t("selectedSources.openTool", { tool: tool.name }) : t("selectedSources.copyForTool", { tool: tool?.name ?? t("selectedSources.anyExternalTool") })}</button>
    </div> : <div className="space-y-3">
      <p>{t("selectedSources.tagsHint")}</p>
      {tagData.isError ? <div role="status"><p>{t("selectedSources.tagsLoadFailed")}</p><button type="button" disabled={working} onClick={() => void tagData.refetch()}>{t("sourceAccess.retry")}</button></div> : null}
      <label>{t("selectedSources.tagAction")} <select value={tagAction} disabled={working} onChange={(event) => { setTagAction(event.target.value as "add" | "remove"); clearBatch(); }}><option value="add">{t("selectedSources.addTag")}</option><option value="remove">{t("selectedSources.removeTag")}</option></select></label>
      <label>{t("selectedSources.tag")} <input value={tag} disabled={working} onChange={(event) => { setTag(event.target.value); clearBatch(); }} list="selected-source-tags" /></label>
      <datalist id="selected-source-tags">{(tagData.data?.tags ?? []).map((name) => <option key={name} value={name} />)}</datalist>
      <button type="button" className="page-editor-action" disabled={working || !tag.trim()} onClick={startTagBatch}>{working ? t("selectedSources.saving") : t("selectedSources.applyTags", { count: sources.length })}</button>
      {batch && (batch.saved > 0 || batch.failed.length > 0) ? <div role={batch.failed.length ? "alert" : "status"}><p>{batch.failed.length ? t("selectedSources.tagsPartial", { saved: batch.saved, failed: batch.failed.length, total: batch.total }) : t("selectedSources.tagsSaved", { saved: batch.saved })}</p>{batch.failed.length ? <ul>{batch.failed.map((file) => <li key={documentTagKey(file.source, file.source_id)}>{file.title}</li>)}</ul> : null}{batch.failed.length ? <button type="button" disabled={working} onClick={retryTagBatch}>{t("selectedSources.retryFailed")}</button> : null}</div> : null}
    </div>}
  </SourceDialog>;
}
