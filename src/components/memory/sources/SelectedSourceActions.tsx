// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPage, listPages, listAllTags, patchDocumentTag, type IndexedFileInfo } from "../../../lib/tauri";
import { useLiveAgents } from "../../connect/connectionState";
import SourceDialog from "./SourceDialog";
import { connectedTools } from "../askAi/WritePages";
import { handToTool, opensWithText, type HandOver } from "../askAi/askInTool";
import { ToolPicker } from "../askAi/ToolPicker";
import { documentTagKey, persistedOrganizePrompt } from "./sourceOrganize";
import { createResearchRun, getResearchRun, getResearchRules, mergeResearchTags, resumeResearchRun, saveResearchRules, type OrganizeRun } from "../../../lib/researchApi";
import { readOrganizeDraft, writeOrganizeDraft } from "../space-detail/sourceSelectionState";

type Mode = "organize" | "tags";

interface TagBatch {
  tag: string;
  action: "add" | "remove";
  failed: IndexedFileInfo[];
  saved: number;
  total: number;
}

export function SelectedSourceActions({ spaceId, spaceName, sources, mode, onClose, resumeRun, previousRuns = [] }: {
  spaceId: string;
  spaceName: string;
  sources: readonly IndexedFileInfo[];
  mode: Mode;
  onClose: () => void;
  resumeRun?: OrganizeRun | null;
  currentSelectionCount?: number;
  previousRuns?: readonly OrganizeRun[];
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
  const [instructions, setInstructions] = useState(() => resumeRun?.rules.instructions ?? readOrganizeDraft(spaceId) ?? "");
  const [instructionsTouched, setInstructionsTouched] = useState(readOrganizeDraft(spaceId) !== null || !!resumeRun);
  const [preparedRun, setPreparedRun] = useState<OrganizeRun | null>(resumeRun ?? null);
  const [runError, setRunError] = useState(false);
  const [destination, setDestination] = useState<string | null>(null);
  const pages = useQuery({ queryKey: ["research-destinations", spaceName], queryFn: () => listPages("active", spaceName, 1000), enabled: mode === "organize" && !resumeRun, retry: false });
  const availablePages = (pages.data ?? []).filter(page => page.space === spaceName);
  const previousTarget = [...previousRuns].sort((a, b) => b.updated_at - a.updated_at).find(run => run.target_page_id && availablePages.some(page => page.id === run.target_page_id));
  const selectedTarget = destination ?? previousTarget?.target_page_id ?? "";
  const previousTargetRun = [...previousRuns].filter(run => run.target_page_id === selectedTarget).sort((a, b) => b.updated_at - a.updated_at)[0];
  const selectedKeys = new Set(sources.map(source => `${source.source}::${source.source_id}`));
  const previousKeys = new Set(previousTargetRun?.items.map(source => `${source.source}::${source.source_id}`) ?? []);
  const addedSources = sources.filter(source => !previousKeys.has(`${source.source}::${source.source_id}`));
  const removedSources = previousTargetRun?.items.filter(source => !selectedKeys.has(`${source.source}::${source.source_id}`)) ?? [];
  const rules = useQuery({ queryKey: ["research-rules", spaceName], queryFn: () => getResearchRules(spaceName), enabled: mode === "organize", retry: false });
  const targetPage = useQuery({ queryKey: ["page", resumeRun?.target_page_id], queryFn: () => getPage(resumeRun!.target_page_id!), enabled: mode === "organize" && !!resumeRun?.target_page_id, retry: false });
  useEffect(() => {
    if (!instructionsTouched && rules.data) setInstructions(rules.data.instructions);
  }, [rules.data, instructionsTouched]);
  const [remember, setRemember] = useState(false);
  const [tag, setTag] = useState("");
  const [tagAction, setTagAction] = useState<"add" | "remove" | "merge">("add");
  const [intoTag, setIntoTag] = useState("");
  const [mergeError, setMergeError] = useState(false);
  const [working, setWorking] = useState(false);
  const [handover, setHandover] = useState<HandOver | null>(null);
  const [batch, setBatch] = useState<TagBatch | null>(null);
  const tagData = useQuery({ queryKey: ["tags"], queryFn: listAllTags, enabled: mode === "tags", retry: false });
  const handOffPrompt = async () => {
    if (!rules.data || !resumeRun && (!pages.isSuccess || selectedTarget && !availablePages.some(page => page.id === selectedTarget))) return;
    setWorking(true); setRunError(false);
    try {
      if (!resumeRun && remember && instructions.trim() !== rules.data.instructions) {
        await saveResearchRules({ space: spaceName, expected_revision: rules.data.revision, name: rules.data.name, instructions: instructions.trim() });
        await client.invalidateQueries({ queryKey: ["research-rules", spaceName] });
      }
      let resumed: OrganizeRun | null = null;
      if (resumeRun) {
        const current = await getResearchRun(spaceName, resumeRun.id);
        if (current.revision !== (preparedRun ?? resumeRun).revision) {
          await client.invalidateQueries({ queryKey: ["research-runs", spaceName] });
          throw new Error("run changed");
        }
        if (current.state === "cancelled") {
          resumed = await resumeResearchRun(spaceName, current.id, current.revision);
        }
      }
      const run = resumed ?? preparedRun ?? await createResearchRun({ space: spaceName, sources: sources.map(({ source, source_id, title }) => ({ source, source_id, title })), instructions: instructions.trim() || null, target_page_id: selectedTarget || null });
      setPreparedRun(run);
      await client.invalidateQueries({ queryKey: ["research-runs", spaceName] });
      const prompt = persistedOrganizePrompt(spaceName, run.id, run.target_page_id);
      const result = await handToTool(tool?.family ?? "other", prompt);
      setHandover(result);
    } catch { setRunError(true); }
    finally { setWorking(false); }
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
    if (tagAction === "merge") {
      setWorking(true); setBatch(null); setMergeError(false);
      void mergeResearchTags({ space: spaceName, sources: sources.map(({ source, source_id, title }) => ({ source, source_id, title })), from_tag: normalized, into_tag: intoTag.trim().toLowerCase() })
        .then(async ({ updated }) => { setBatch({ tag: intoTag, action: "add", failed: [], saved: updated, total: sources.length }); await client.invalidateQueries({ queryKey: ["tags"] }); })
        .catch(() => setMergeError(true)).finally(() => setWorking(false));
      return;
    }
    void applyTagBatch(normalized, tagAction, [...sources]);
  };
  const retryTagBatch = () => {
    if (batch?.failed.length) void applyTagBatch(batch.tag, batch.action, [...batch.failed], batch.total, batch.saved);
  };
  const clearBatch = () => { setBatch(null); setMergeError(false); };
  const title = mode === "organize" ? t("selectedSources.organizeTitle", { count: sources.length }) : t("selectedSources.tagsTitle", { count: sources.length });

  return <SourceDialog title={title} onClose={onClose}>
    {mode === "organize" ? <div className="space-y-3">
      <p>{t("selectedSources.organizeHint")}</p>
      {rules.isPending && <p role="status">{t("selectedSources.rulesLoading")}</p>}
      {rules.isError && <div role="alert">{t("selectedSources.rulesFailed")} <button type="button" onClick={() => void rules.refetch()}>{t("sourceAccess.retry")}</button></div>}
      {runError && <p role="alert">{t("selectedSources.runFailed")}</p>}
      {resumeRun && <div><p>{t("selectedSources.resumeScope", { count: resumeRun.items.length })}</p>{rules.data && rules.data.instructions.trim() !== resumeRun.rules.instructions.trim() && <p>{t("selectedSources.resumePreviousRules")}</p>}{resumeRun.target_page_id && <p>{targetPage.data?.space === spaceName && targetPage.data.title ? t("selectedSources.targetPage", { title: targetPage.data.title }) : t("selectedSources.targetPageFallback")}</p>}</div>}
      {!resumeRun && <>
        {pages.isPending && <p role="status">{t("selectedSources.pagesLoading")}</p>}
        {pages.isError && <div role="alert">{t("selectedSources.pagesFailed")} <button type="button" onClick={() => void pages.refetch()}>{t("sourceAccess.retry")}</button></div>}
        {pages.isSuccess && <label>{t("selectedSources.destination")} <select aria-label={t("selectedSources.destination")} value={selectedTarget} disabled={working || !!preparedRun} onChange={event => { setDestination(event.target.value); setHandover(null); }}><option value="">{t("selectedSources.newPage")}</option>{availablePages.map(page => <option key={page.id} value={page.id}>{page.title}</option>)}</select></label>}
        {pages.data && pages.data.length >= 1000 && <p>{t("selectedSources.pagesLimited")}</p>}
        {previousTargetRun && <details><summary>{t("selectedSources.scopeChanges", { added: addedSources.length, removed: removedSources.length })}</summary><p>{t("selectedSources.scopeChangesHint")}</p><p>{t(previousTargetRun.rules.instructions.trim() === instructions.trim() ? "selectedSources.sameRules" : "selectedSources.changedRules")}</p>{addedSources.length > 0 && <><p>{t("selectedSources.scopeAdded")}</p><ul>{addedSources.map(source => <li key={`${source.source}:${source.source_id}`}>{source.title}</li>)}</ul></>}{removedSources.length > 0 && <><p>{t("selectedSources.scopeRemoved")}</p><ul>{removedSources.map(source => <li key={`${source.source}:${source.source_id}`}>{source.title}</li>)}</ul></>}</details>}
      </>}
      <ul aria-label={t("selectedSources.selectedList")}>{sources.map((file) => <li key={`${file.source}:${file.source_id}`}>{file.title}</li>)}</ul>
      <label className="block">{t(resumeRun ? "selectedSources.previousInstructions" : "selectedSources.instructions")}<textarea value={instructions} onChange={(event) => { setInstructions(event.target.value); setInstructionsTouched(true); writeOrganizeDraft(spaceId, event.target.value); setHandover(null); }} rows={4} disabled={working || !!preparedRun} /></label>
      {!resumeRun && <label className="flex items-center gap-2" style={{ flexDirection: "row", alignItems: "center" }}><input type="checkbox" style={{ width: "auto", flexShrink: 0, padding: 0 }} checked={remember} disabled={working || !!preparedRun} onChange={(event) => setRemember(event.target.checked)} /> {t("selectedSources.remember")}</label>}
      {tools.length > 1 && tool ? <ToolPicker tools={tools} current={tool.family} onChoose={(family) => { setToolFamily(family); setHandover(null); }} /> : null}
      <p>{t(tool && opensWithText(tool.family) ? "selectedSources.externalOpens" : "selectedSources.externalCopies", { tool: tool?.name ?? t("selectedSources.anyExternalTool") })}</p>
      <button type="button" className="page-editor-action" disabled={working || !rules.data || !resumeRun && !pages.isSuccess} aria-live="polite" onClick={() => void handOffPrompt()}>{handover === "opened" ? t("askAi.handOver.opened", { tool: tool?.name ?? t("selectedSources.anyExternalTool") }) : handover === "copied" ? t("askAi.handOver.copied", { tool: tool?.name ?? t("selectedSources.anyExternalTool") }) : handover === "failed" ? t("askAi.handOver.failed") : tool && opensWithText(tool.family) ? t("selectedSources.openTool", { tool: tool.name }) : t("selectedSources.copyForTool", { tool: tool?.name ?? t("selectedSources.anyExternalTool") })}</button>
    </div> : <div className="space-y-3">
      <p>{t("selectedSources.tagsHint")}</p>
      {tagData.isError ? <div role="status"><p>{t("selectedSources.tagsLoadFailed")}</p><button type="button" disabled={working} onClick={() => void tagData.refetch()}>{t("sourceAccess.retry")}</button></div> : null}
      <label>{t("selectedSources.tagAction")} <select aria-label={t("selectedSources.tagAction")} value={tagAction} disabled={working} onChange={(event) => { setTagAction(event.target.value as "add" | "remove" | "merge"); clearBatch(); }}><option value="add">{t("selectedSources.addTag")}</option><option value="remove">{t("selectedSources.removeTag")}</option><option value="merge">{t("selectedSources.mergeTag")}</option></select></label>
      <label>{t("selectedSources.tag")} <input aria-label={t("selectedSources.tag")} value={tag} disabled={working} onChange={(event) => { setTag(event.target.value); clearBatch(); }} list="selected-source-tags" /></label>
      <datalist id="selected-source-tags">{(tagData.data?.tags ?? []).map((name) => <option key={name} value={name} />)}</datalist>
      {tagAction === "merge" && <><label>{t("selectedSources.mergeInto")} <input aria-label={t("selectedSources.mergeInto")} value={intoTag} disabled={working} onChange={event => { setIntoTag(event.target.value); clearBatch(); }} list="selected-source-tags" /></label><p>{t("selectedSources.mergeHint")}</p></>}
      {mergeError && <p role="alert">{t("selectedSources.mergeFailed")}</p>}
      <button type="button" className="page-editor-action" disabled={working || !tag.trim() || tagAction === "merge" && (!intoTag.trim() || intoTag.trim().toLowerCase() === tag.trim().toLowerCase())} onClick={startTagBatch}>{working ? t("selectedSources.saving") : t("selectedSources.applyTags", { count: sources.length })}</button>
      {batch ? <div role={batch.failed.length ? "alert" : "status"}><p>{batch.failed.length ? t("selectedSources.tagsPartial", { saved: batch.saved, failed: batch.failed.length, total: batch.total }) : t("selectedSources.tagsSaved", { saved: batch.saved, count: batch.saved })}</p>{batch.failed.length ? <ul>{batch.failed.map((file) => <li key={documentTagKey(file.source, file.source_id)}>{file.title}</li>)}</ul> : null}{batch.failed.length ? <button type="button" disabled={working} onClick={retryTagBatch}>{t("selectedSources.retryFailed")}</button> : null}</div> : null}
    </div>}
  </SourceDialog>;
}
