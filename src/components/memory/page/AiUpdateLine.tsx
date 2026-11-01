// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { revertPageSentences, type LastAiUpdate, type SentenceRevertResponse } from "../../../lib/tauri";
import "./AiUpdateLine.css";

/** Whether the AI's newest write is still news: the person has not edited the
 *  page since it landed. */
export function aiUpdateShows(update: LastAiUpdate | null | undefined, humanEditedAt: number | null | undefined): update is LastAiUpdate {
  return !!update && update.changed_sentences > 0 && (humanEditedAt == null || humanEditedAt < update.at);
}

/** The quiet line in Info that says the AI updated its own sentences, with
 *  one Undo (UX-WHO-2). Mount it with `key={pageId}` so a result never shows
 *  on another page. */
export function AiUpdateLine({ pageId, update, humanEditedAt, time, disabled }: {
  pageId: string;
  update: LastAiUpdate | null | undefined;
  humanEditedAt: number | null | undefined;
  /** When the update landed, already worded ("2h ago"). */
  time: string;
  /** True while an unsaved edit, a save or another page action holds the page. */
  disabled: boolean;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [result, setResult] = useState<SentenceRevertResponse | "failed" | null>(null);
  const undo = useMutation({
    mutationFn: (version: number) => revertPageSentences(pageId, version),
    onSuccess: async (answer) => {
      setResult(answer);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["page", pageId] }),
        queryClient.invalidateQueries({ queryKey: ["page-sentences", pageId] }),
        queryClient.invalidateQueries({ queryKey: ["page-revisions", pageId] }),
        queryClient.invalidateQueries({ queryKey: ["pages"] }),
      ]);
    },
    onError: () => setResult("failed"),
  });

  if (result && result !== "failed") {
    const message = result.reverted > 0
      ? [t("aiUpdate.undone", { count: result.reverted }), result.skipped > 0 ? t("aiUpdate.skipped", { count: result.skipped }) : null]
        .filter(Boolean).join(" ")
      : t("aiUpdate.nothing");
    return <p className="ai-update-line" role="status">{message}</p>;
  }
  if (!aiUpdateShows(update, humanEditedAt)) return null;
  return (
    <div className="ai-update-line">
      <span>{t("aiUpdate.updated", { count: update.changed_sentences, time })}</span>
      <button
        className="ai-update-undo"
        disabled={disabled || undo.isPending}
        onClick={() => { setResult(null); undo.mutate(update.version); }}
        type="button"
      >
        {t("aiUpdate.undo")}
      </button>
      {result === "failed" && <span className="ai-update-error" role="alert">{t("aiUpdate.failed")}</span>}
    </div>
  );
}
