// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useState } from "react";
import { createPortal } from "react-dom";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  applyRulesReview, previewRulesReview, type PullbackAction, type PullbackCard,
  type PullbackDecision, type PullbackPreview, type RulesSetupStatus,
} from "../../../lib/tauri";
import { ConfirmDialog } from "../../ConfirmDialog";
import { Button } from "../settings/primitives";
import { refreshRulesSetup } from "./useRulesSetup";
import { RulesSetupDiff } from "./RulesSetupDiff";
import { rulesSetupFileLabel, rulesSetupMessage, rulesSetupName, rulesSetupNotice } from "./rulesSetupText";
import "./rulesSetup.css";

export function RulesSetupReviews({ status }: { readonly status: RulesSetupStatus }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ decision: PullbackDecision; result: PullbackPreview; label: string } | null>(null);
  const [feedback, setFeedback] = useState<{ failed: boolean; message: string } | null>(null);

  async function prepare(decision: PullbackDecision, label: string) {
    setBusy(decision.id);
    setFeedback(null);
    try {
      setPreview({ decision, result: await previewRulesReview(decision), label });
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.previewFailed") });
      await refreshRulesSetup(client);
    } finally {
      setBusy(null);
    }
  }

  async function confirm() {
    if (!preview) return;
    const { decision } = preview;
    setBusy(decision.id);
    setPreview(null);
    try {
      const result = await applyRulesReview(decision);
      if (result.blocked.length) setFeedback({ failed: false, message: t("rulesSetup.reviewRemaining") });
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.changeFailed") });
    } finally {
      await refreshRulesSetup(client);
      setBusy(null);
    }
  }

  const currentReview = preview && status.proposals.find((card) => card.id === preview.decision.id);
  return (
    <>
      {status.proposals.map((card) => (
        <ReviewCard key={card.id} card={card} tools={status.review_tools}
          busy={busy !== null || !!currentReview} onChoice={(decision, label) => void prepare(decision, label)} />
      ))}
      {feedback && <p className="rules-setup-note" role={feedback.failed ? "alert" : "status"}>{feedback.message}</p>}
      {preview && currentReview && createPortal(
        <ConfirmDialog
          tone="neutral"
          message={t("rulesSetup.reviewTitle")}
          details={<>
            {preview.result.plan_codes?.length ? (
              <ul className="rules-setup-files">{preview.result.plan_codes.map((notice, index) => (
                <li key={index}>{rulesSetupNotice(t, i18n.language, notice)}</li>
              ))}</ul>
            ) : <p className="rules-setup-reason">{rulesSetupMessage(t)}</p>}
            <ul className="rules-setup-files">{(preview.decision.file ? [preview.decision.file] : currentReview.files).map((file) => (
              <li key={file}>{rulesSetupFileLabel(t, status.review_tools[file], file)}</li>
            ))}</ul>
            <RulesSetupDiff diff={preview.result.diff} tools={status.review_tools} codes={preview.result.diff_codes} />
          </>}
          confirmLabel={preview.decision.action === "dismiss" ? t("rulesSetup.restore") : preview.label}
          cancelLabel={t("rulesSetup.cancel")}
          onAnswer={(yes) => { if (yes) void confirm(); else setPreview(null); }}
        />, document.body,
      )}
    </>
  );
}

function ReviewCard({ card, tools, busy, onChoice }: {
  readonly card: PullbackCard;
  readonly tools: RulesSetupStatus["review_tools"];
  readonly busy: boolean;
  readonly onChoice: (decision: PullbackDecision, label: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const headingId = useId();
  const [source, setSource] = useState(card.files[0]);
  const tool = rulesSetupName(tools[source]?.name) || t("rulesSetup.thisTool");
  const names = [...new Set(card.files.map((file) => rulesSetupName(tools[file]?.name)).filter(Boolean))];
  const scope = card.kind === "scope";
  const all = t("rulesSetup.allTools");
  const only = t("rulesSetup.toolOnly", { tool });
  const no = t("rulesSetup.no");
  const choice = (action: PullbackAction, label: string) =>
    onChoice({ id: card.id, action, ...(action !== "dismiss" && card.files.length > 1 ? { file: source } : {}) }, label);

  return (
    <article className="rules-setup-review" aria-labelledby={headingId}>
      <h2 id={headingId}>{names.length === 1
        ? t(scope ? "rulesSetup.ruleChanged" : "rulesSetup.toolChanged", { tool: names[0] })
        : t("rulesSetup.reviewTitle")}</h2>
      {scope && <p>{t("rulesSetup.alreadyApplies", { tool })}</p>}
      <p className="rules-setup-reason">{rulesSetupMessage(t, card.reason_code)}</p>
      {card.files.length > 1 ? (
        <label className="rules-setup-source">
          {t("rulesSetup.chooseSource")}
          <select value={source} disabled={busy} onChange={(event) => setSource(event.target.value)}>
            {card.files.map((file) => <option key={file} value={file}>{rulesSetupFileLabel(t, tools[file], file)}</option>)}
          </select>
        </label>
      ) : (
        <ul className="rules-setup-files">{card.files.map((file) => <li key={file}>{rulesSetupFileLabel(t, tools[file], file)}</li>)}</ul>
      )}
      <RulesSetupDiff diff={card.diff} tools={tools} codes={card.diff_codes} />
      <div className="rules-setup-actions">
        {scope && <Button type="button" variant="primary" disabled={busy} onClick={() => choice("all_tools", all)}>{all}</Button>}
        <Button type="button" disabled={busy} onClick={() => choice("tool_only", only)}>{only}</Button>
        {!scope && <>
          <Button type="button" variant="primary" disabled={busy} onClick={() => choice("all_tools", all)}>{all}</Button>
          <Button type="button" variant="ghost" disabled={busy} onClick={() => choice("dismiss", no)}>{no}</Button>
        </>}
      </div>
      {!scope && <p className="rules-setup-note">{names.length > 1
        ? t("rulesSetup.noEffectMany", { tools: new Intl.ListFormat(i18n.resolvedLanguage).format(names) })
        : t("rulesSetup.noEffect", { tool })}</p>}
    </article>
  );
}
