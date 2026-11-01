// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { flagPageSentence, recheckPageSentence, type PageSentence } from "../../../lib/tauri";
import { leadingWeakReason } from "../editor/sentenceAuthorship";
import "./thisSentence.css";

interface ThisSentenceProps {
  readonly pageId: string;
  /** The sentence the cursor is in; nothing shows without one. */
  readonly sentence: PageSentence | null;
  /** Put the cursor on the sentence so the person can fix it themselves. */
  readonly onFixMyself: (text: string) => void;
  readonly disabled: boolean;
}

/** Who wrote it, in the person's words (UX-WHO-1). */
export function sentenceWhoKey(sentence: PageSentence) {
  if (sentence.author === "original") return "thisSentence.who.original" as const;
  if (sentence.protected) return sentence.accepted ? "thisSentence.who.accepted" as const : "thisSentence.who.you" as const;
  if (sentence.author === "ai") return "thisSentence.who.ai" as const;
  return "thisSentence.who.unknown" as const;
}

function evidenceKey(sentence: PageSentence) {
  switch (sentence.evidence) {
    case "verified": return "thisSentence.evidence.verified" as const;
    case "unverified": return "thisSentence.evidence.unverified" as const;
    case "unevaluated": return "thisSentence.evidence.unevaluated" as const;
    default: return "thisSentence.evidence.none" as const;
  }
}

/**
 * Info's "this sentence": who said it, what backs it, and 「不對」 (fix it
 * yourself, or ask the AI, whose fix is still a proposal). A sentence that
 * can't be backed up can be rechecked alone (docs/ux/spec.md, UX-WHO-1,
 * UX-WHO-3, UX-WHO-4).
 */
export function ThisSentence({ pageId, sentence, onFixMyself, disabled }: ThisSentenceProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [choosing, setChoosing] = useState(false);
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["page-sentences", pageId] });
  const flag = useMutation({
    mutationFn: (hash: string) => flagPageSentence(pageId, hash, true),
    onSuccess: () => { setChoosing(false); void refresh(); },
  });
  const recheck = useMutation({
    mutationFn: (hash: string) => recheckPageSentence(pageId, hash),
    onSuccess: () => void refresh(),
  });
  const askedAi = !!sentence && flag.isSuccess && flag.variables === sentence.hash;
  const aiQueued = askedAi && flag.data?.ai_fix_queued === true;
  const proposalCount = sentence?.proposals?.length ?? 0;
  // The AI's fix arrives a few seconds after the flag, off the request; look
  // again for a while until it shows up.
  useEffect(() => {
    if (!aiQueued || proposalCount > 0) return;
    let tries = 0;
    const timer = window.setInterval(() => {
      tries += 1;
      void refresh();
      if (tries >= 15) window.clearInterval(timer);
    }, 4000);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [aiQueued, proposalCount, pageId]);
  if (!sentence || sentence.author === "original") return null;
  const weak = leadingWeakReason(sentence.weak ?? []);
  return (
    <section className="this-sentence" aria-labelledby="this-sentence-title" data-testid="this-sentence">
      <h3 id="this-sentence-title" className="this-sentence-title">{t("thisSentence.title")}</h3>
      <p className="this-sentence-text">{sentence.text}</p>
      <p className="this-sentence-line">
        <span className={`this-sentence-who this-sentence-who--${sentence.protected ? "you" : sentence.author}`}>
          {t(sentenceWhoKey(sentence))}
        </span>
        <span aria-hidden="true">·</span>
        <span>{t(evidenceKey(sentence))}</span>
      </p>
      {/* "No source" is already said on the line above. */}
      {weak && weak !== "no_citation" ? (
        <p className="this-sentence-line this-sentence-weak">
          {t("thisSentence.weak", { reason: t(`weakReason.${weak}`) })}
        </p>
      ) : null}
      {recheck.isSuccess && recheck.variables === sentence.hash && !weak ? (
        <p className="this-sentence-line" role="status">{t("thisSentence.recheckedOk")}</p>
      ) : null}
      {recheck.isError ? <p className="this-sentence-error" role="alert">{t("thisSentence.recheckFailed")}</p> : null}
      {askedAi || sentence.flagged ? (
        <p className="this-sentence-line" role="status">
          {proposalCount > 0
            ? t("thisSentence.proposalReady")
            : aiQueued
              ? t("thisSentence.aiWillPropose")
              : askedAi
                ? t("thisSentence.noModel")
                : t("thisSentence.flagged")}
        </p>
      ) : choosing ? (
        <div className="this-sentence-actions" role="group" aria-label={t("thisSentence.notRight")}>
          <button
            type="button"
            className="this-sentence-button"
            disabled={disabled}
            onClick={() => { setChoosing(false); onFixMyself(sentence.text); }}
          >
            {t("thisSentence.fixMyself")}
          </button>
          <button
            type="button"
            className="this-sentence-button"
            disabled={disabled || flag.isPending}
            onClick={() => flag.mutate(sentence.hash)}
          >
            {t("thisSentence.askAi")}
          </button>
          <button type="button" className="memory-detail-text-button" onClick={() => setChoosing(false)}>
            {t("thisSentence.cancel")}
          </button>
        </div>
      ) : (
        <div className="this-sentence-actions">
          {weak && weak !== "no_citation" ? (
            <button
              type="button"
              className="this-sentence-button"
              disabled={disabled || recheck.isPending}
              onClick={() => recheck.mutate(sentence.hash)}
            >
              {recheck.isPending ? t("thisSentence.rechecking") : t("thisSentence.recheck")}
            </button>
          ) : null}
          <button type="button" className="this-sentence-button" disabled={disabled} onClick={() => setChoosing(true)}>
            {t("thisSentence.notRight")}
          </button>
          {weak && weak !== "no_citation" ? null : <span className="this-sentence-hint">{t("thisSentence.notRightHint")}</span>}
        </div>
      )}
      {flag.isError ? <p className="this-sentence-error" role="alert">{t("thisSentence.flagFailed")}</p> : null}
    </section>
  );
}
