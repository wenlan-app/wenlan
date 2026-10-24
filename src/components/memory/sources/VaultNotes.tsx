// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  acceptNoteProposal,
  dismissNoteProposal,
  getKnowledgePath,
  getNoteAiChanges,
  getNoteProposal,
  listNoteProposals,
  listRegisteredSources,
  setNoteAiChanges,
  type AiChanges,
  type NoteAiChanges,
  type NoteProposal,
  type RegisteredSource,
} from "../../../lib/tauri";
import { resolveAgentDisplayName } from "../../../lib/agents";
import { diffWords, type DiffSegment } from "../../../lib/wordDiff";
import { AiChangesOptions } from "../AiChangesOptions";
import { DiffText } from "../ReviewDialog";
import "../page/NoteReviewNotice.css";
import "../page/BasisUpdates.css";
import type { enSourceContainer } from "./sourceContainerCopy";

type CopyKey = `sourceContainer.${keyof typeof enSourceContainer}`;

function normalized(path: string) { return path.replace(/\\/g, "/").replace(/\/+$/, ""); }

/**
 * The settings key for `path` inside `vault`: `""` for the vault itself, `dir/`
 * for a folder, `dir/note.md` for a note. `null` when it is outside the vault.
 */
export function vaultKey(vault: string, path: string, folder: boolean): string | null {
  const root = normalized(vault);
  const inner = normalized(path);
  if (inner === root) return "";
  if (!inner.startsWith(`${root}/`)) return null;
  const rel = inner.slice(root.length + 1);
  return folder ? `${rel}/` : rel;
}

/** True when `path` is Wenlan's own pages folder or inside it. */
export function inPagesFolder(path: string, pages: string | undefined): boolean {
  if (!pages) return false;
  const inner = normalized(path);
  const root = normalized(pages);
  return inner === root || inner.startsWith(`${root}/`);
}

const FROM_KEY: Record<NoteAiChanges["from"], CopyKey> = {
  file: "sourceContainer.aiFromFile",
  folder: "sourceContainer.aiFromFolder",
  vault: "sourceContainer.aiFromVault",
  default: "sourceContainer.aiFromDefault",
  pdf: "sourceContainer.aiFromPdf",
};

const OPTIONS: { value: AiChanges; label: CopyKey; hint: CopyKey }[] = [
  { value: "ask_first", label: "sourceContainer.aiAskFirst", hint: "sourceContainer.aiAskFirstHint" },
  { value: "never", label: "sourceContainer.aiNever", hint: "sourceContainer.aiNeverHint" },
];

/** The pages folder, so Wenlan's own pages never show a note setting. */
export function usePagesFolder(enabled: boolean) {
  return useQuery({ queryKey: ["knowledgePath"], queryFn: () => getKnowledgePath(), staleTime: 60_000, enabled }).data;
}

/**
 * Whether AI may change the person's notes at one place in a vault: the vault,
 * a folder, or one note. The choice is stored in Wenlan, never in the notes.
 */
export function NoteAiChangesSection({ source, path, kind }: { source: RegisteredSource; path: string; kind: "vault" | "folder" | "note" }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const key = vaultKey(source.path, path, kind === "folder");
  const queryKey = ["noteAiChanges", source.id, key];
  const setting = useQuery({
    queryKey,
    queryFn: () => getNoteAiChanges(source.id, key ?? ""),
    enabled: key !== null,
  });
  const save = useMutation({
    mutationFn: (value: AiChanges | null) => setNoteAiChanges(source.id, key ?? "", value),
    onSuccess: async (next) => {
      queryClient.setQueryData(queryKey, next);
      await queryClient.invalidateQueries({ queryKey: ["noteAiChanges", source.id] });
      await queryClient.invalidateQueries({ queryKey: ["noteProposals", source.id] });
    },
    onError: (error) => {
      console.warn("[sources] note setting failed", error);
      toast(t("sourceContainer.aiSaveFailed"));
    },
  });
  if (key === null || !setting.data) return null;
  const current = setting.data;
  const pdf = current.from === "pdf";
  const own = kind === "vault" ? current.from === "vault" : kind === "folder" ? current.from === "folder" && current.path === key : current.from === "file";
  const heading = kind === "vault" ? "sourceContainer.aiVaultHeading" : kind === "folder" ? "sourceContainer.aiFolderHeading" : "sourceContainer.aiChanges";

  if (pdf) return <section className="note-ai-changes" aria-label={t(heading)}>
    <h3 className="note-info-section-heading">{t(heading)}</h3>
    <p>{t("sourceContainer.aiFromPdf")}</p>
  </section>;
  return <AiChangesOptions heading={t(heading)} name={`note-ai-${source.id}-${key}`}
    options={OPTIONS.map((option) => ({ value: option.value, label: t(option.label), hint: t(option.hint) }))}
    value={current.value} disabled={save.isPending} onChange={(value) => save.mutate(value)}>
    <p className="note-ai-from">
      {t(kind === "vault" && current.from === "default" ? "sourceContainer.aiVaultDefault" : FROM_KEY[current.from])}
      {own && kind !== "vault" && <> <button type="button" className="note-ai-reset" onClick={() => save.mutate(null)}>{t("sourceContainer.aiReset")}</button></>}
    </p>
    {kind === "vault" && <p className="note-ai-from">{t("sourceContainer.aiPdfNote")}</p>}
  </AiChangesOptions>;
}

/** Long unchanged runs shrink to their ends, so the change is what you see. */
export function trimUnchanged(segments: DiffSegment[], keep = 160): DiffSegment[] {
  return segments.map((segment, index) => {
    if (segment.kind !== "same" || segment.text.length <= keep * 2 + 1) return segment;
    const head = index === 0 ? "" : segment.text.slice(0, keep);
    const tail = index === segments.length - 1 ? "" : segment.text.slice(-keep);
    return { kind: "same" as const, text: `${head} … ${tail}` };
  });
}

// The interval pauses while the window is hidden, so coming back checks at once.
function useNoteProposals(sourceId: string) {
  return useQuery({ queryKey: ["noteProposals", sourceId], queryFn: () => listNoteProposals(sourceId), refetchInterval: 10_000, refetchOnWindowFocus: true });
}

/**
 * Shown above a note when an AI proposed a change to it: what changes and why,
 * with Accept and Decline. Nothing is written to the note until Accept.
 */
export function NoteProposalNotice({ source, path, onAccepted }: { source: RegisteredSource; path: string; onAccepted?: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const key = vaultKey(source.path, path, false);
  const proposals = useNoteProposals(source.id);
  const proposal = proposals.data?.find((item) => item.path === key);
  const view = useQuery({
    queryKey: ["noteProposal", proposal?.id, proposal?.created_at],
    queryFn: () => getNoteProposal(proposal!.id),
    enabled: !!proposal,
  });
  const segments = useMemo(() => view.data ? trimUnchanged(diffWords(view.data.current, view.data.proposal.content)) : [], [view.data]);
  const removes = segments.some((segment) => segment.kind === "del");
  const adds = segments.some((segment) => segment.kind === "ins");
  const legend = removes && !adds ? "sourceContainer.suggestLegendRemoved"
    : adds && !removes ? "sourceContainer.suggestLegendAdded" : "sourceContainer.suggestLegend";
  const refresh = async () => {
    // Every list of suggestions, the home one included.
    await queryClient.invalidateQueries({ queryKey: ["noteProposals"] });
    await queryClient.invalidateQueries({ queryKey: ["fileText"] });
    await queryClient.invalidateQueries({ queryKey: ["indexedFiles"] });
  };
  const accept = useMutation({
    mutationFn: (id: string) => acceptNoteProposal(id),
    onSuccess: async () => { await refresh(); toast(t("sourceContainer.suggestAccepted")); onAccepted?.(); },
    onError: async (error) => {
      console.warn("[sources] accept note change failed", error);
      await refresh();
      toast(t(String(error).includes("note_changed_since_proposal") ? "sourceContainer.suggestStale" : "sourceContainer.suggestFailed"));
    },
  });
  const decline = useMutation({
    mutationFn: (id: string) => dismissNoteProposal(id),
    onSuccess: refresh,
    onError: (error) => { console.warn("[sources] decline note change failed", error); toast(t("sourceContainer.suggestFailed")); },
  });
  if (!proposal) return null;
  const busy = accept.isPending || decline.isPending;

  return <section className="note-proposal-notice" aria-label={t("sourceContainer.suggestTitle")}>
    <div className="note-proposal-head">
      <div>
        <p className="note-proposal-title">{t("sourceContainer.suggestTitle")}</p>
        {proposal.reason && <p className="note-proposal-reason">{proposal.reason}</p>}
        <p className="note-proposal-by">{t("sourceContainer.suggestBy", { agent: resolveAgentDisplayName(proposal.agent) })}</p>
      </div>
      <div className="note-proposal-actions">
        <button type="button" className="collection-control note-proposal-accept" disabled={busy} onClick={() => accept.mutate(proposal.id)}>{t("sourceContainer.suggestAccept")}</button>
        <button type="button" className="collection-control" disabled={busy} onClick={() => decline.mutate(proposal.id)}>{t("sourceContainer.suggestDecline")}</button>
      </div>
    </div>
    {view.data && <p className="note-proposal-diff"><DiffText segments={segments} /></p>}
    <p className="note-proposal-legend">{t(legend)}</p>
  </section>;
}

/** Every waiting suggestion, across all vaults: the home and the menu bar count these. */
export function useAllNoteProposals(enabled = true) {
  return useQuery({ queryKey: ["noteProposals", "all"], queryFn: () => listNoteProposals(), refetchInterval: 30_000, refetchOnWindowFocus: true, enabled });
}

/** A note's name as people see it: no folders, no Markdown extension. */
export function noteName(path: string): string {
  return (path.split("/").pop() ?? path).replace(/\.(md|markdown)$/i, "");
}

/**
 * Home: one line per note an AI wants to change, in the same notice as the
 * other things that need the person, each opening that note where Accept and
 * Decline are. Nothing at all when no suggestion waits.
 */
export function NoteSuggestionsSummary({ onOpen }: { onOpen: (location: { sourceId: string; path: string; file: string }) => void }) {
  const { t } = useTranslation();
  const proposals = useAllNoteProposals();
  const sources = useQuery({ queryKey: ["registeredSources"], queryFn: listRegisteredSources });
  const idPrefix = useId();
  const items = (proposals.data ?? []).flatMap((proposal) => {
    const source = sources.data?.find((item) => item.id === proposal.source_id);
    return source ? [{ proposal, source }] : [];
  });
  if (items.length === 0) return null;
  return <div className="note-review-notice basis-updates-home">
    <ul className="note-review-notice__basis-list" aria-label={t("sourceContainer.suggestionsHeading")}>
      {items.map(({ proposal, source }) => {
        const textId = `${idPrefix}-${proposal.id}`;
        const parts = proposal.path.split("/");
        const file = parts.pop() ?? proposal.path;
        return <li key={proposal.id} className="note-review-notice__basis">
          <span id={textId} className="note-review-notice__basis-text">{t("sourceContainer.homeSuggestion", { name: noteName(proposal.path) })}</span>
          <button type="button" className="note-review-notice__button" aria-describedby={textId}
            onClick={() => onOpen({ sourceId: source.id, path: [normalized(source.path), ...parts].join("/"), file })}>
            {t("sourceContainer.homeSuggestionAction")}
          </button>
        </li>;
      })}
    </ul>
  </div>;
}

/** The vault's waiting suggestions, each opening its note. */
export function VaultSuggestions({ source, onOpen }: { source: RegisteredSource; onOpen: (proposal: NoteProposal) => void }) {
  const { t } = useTranslation();
  const proposals = useNoteProposals(source.id);
  const items = proposals.data ?? [];
  if (items.length === 0) return null;
  return <section className="vault-suggestions" aria-label={t("sourceContainer.suggestionsHeading")}>
    <h3 className="note-info-section-heading">{t("sourceContainer.suggestionsHeading")}</h3>
    <ul>
      {items.map((item) => <li key={item.id}>
        <button type="button" className="vault-suggestion" onClick={() => onOpen(item)}>
          <span className="vault-suggestion-name">{item.path.split("/").pop()}</span>
          {item.reason && <span className="vault-suggestion-reason">{item.reason}</span>}
        </button>
      </li>)}
    </ul>
  </section>;
}
