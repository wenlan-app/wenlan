// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useState } from "react";
import { createPortal } from "react-dom";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  applyPersonalRulePlan, applyRulesReview, applyRulesSettings, declinePageRuleMerge, dismissRulesSkillUpdate, previewRulesReview, reviewRulesSkill,
  type PullbackAction, type PullbackCard, type PullbackDecision, type PullbackPreview, type RulesSettingsPlan,
  type PersonalRulePlan, type RulesReviewItem, type RulesReviewList, type RulesSetupStatus, type RulesSkillReviewRequest,
} from "../../../lib/tauri";
import { ConfirmDialog } from "../../ConfirmDialog";
import { Button } from "../settings/primitives";
import { refreshRulesSetup } from "./useRulesSetup";
import { RulesSetupDiff } from "./RulesSetupDiff";
import { toolDisplayName } from "./RulesToolsHeader";
import { rulesSetupFileLabel, rulesSetupMessage, rulesSetupName, rulesSetupNotice } from "./rulesSetupText";
import "./rulesSetup.css";

type OtherReviewItem = Exclude<RulesReviewItem, { kind: "tool_edit" | "personal_plan" }>;

export function RulesSetupReviews({ review }: { readonly review: RulesReviewList }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [preview, setPreview] = useState<{ decision: PullbackDecision; result: PullbackPreview; label: string } | null>(null);
  const [feedback, setFeedback] = useState<{ failed: boolean; message: string } | null>(null);

  const [otherPreview, setOtherPreview] = useState<{
    id: string; label: string; details?: React.ReactNode; apply: () => Promise<unknown>;
  } | null>(null);

  async function prepareOther(item: OtherReviewItem, action: PullbackAction, label: string) {
    setBusy(item.id);
    setFeedback(null);
    try {
      if (item.kind === "settings") {
        setOtherPreview({ id: item.id, label, details: <SettingsChanges plan={item.plan} />,
          apply: () => applyRulesSettings(item.plan.token) });
      } else if (item.kind === "skill_update") {
        setOtherPreview({ id: item.id, label, apply: () => dismissRulesSkillUpdate(item.id) });
      } else {
        const proposal = item.proposal;
        const scope = action === "all_tools" ? { kind: "shared" as const }
          : proposal.origin.kind !== "agent" ? { kind: "tool" as const, tool: proposal.origin.tool }
          : proposal.scope;
        const request: RulesSkillReviewRequest = { id: proposal.id, action: "dismiss" };
        let details: React.ReactNode;
        if (action !== "dismiss") {
          const result = await reviewRulesSkill({ id: proposal.id, action: "preview", scope });
          if (result.status !== "review_preview" || result.delivery.blocked.length) throw new Error("Skill review is blocked");
          const changed = result.delivery.files;
          details = <ul className="rules-setup-files">
            {changed.map(({ before, after }, index) => (
              <li key={index}>{rulesSetupMessage(t, !before ? "create_copy" : !after ? "remove_copy" : "replace_copy")}
                {before?.permissions !== after?.permissions && <> · {[before?.permissions, after?.permissions].filter(Boolean).join(" → ")}</>}
              </li>
            ))}
            {result.delivery.cleared_overrides?.map((tool) => <li key={tool}>{rulesSetupName(tool)}</li>)}
          </ul>;
          Object.assign(request, { action: "accept", scope, token: result.delivery.token });
        }
        setOtherPreview({ id: item.id, label, details, apply: () => reviewRulesSkill(request) });
      }
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.previewFailed") });
      await refreshRulesSetup(client, true);
    } finally {
      setBusy(null);
    }
  }

  async function confirmOther() {
    if (!otherPreview) return;
    setBusy(otherPreview.id);
    setOtherPreview(null);
    try {
      const result = await otherPreview.apply();
      if (result && typeof result === "object") {
        const report = "delivery" in result ? result.delivery : result;
        if (report && typeof report === "object" && "blocked" in report &&
            Array.isArray(report.blocked) && report.blocked.length) {
          setFeedback({ failed: false, message: t("rulesSetup.reviewRemaining") });
        }
      }
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.changeFailed") });
    } finally {
      await refreshRulesSetup(client, true);
      setBusy(null);
    }
  }

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

  const currentItem = preview && review.items.find((item) => item.id === preview.decision.id);
  const currentReview = currentItem?.kind === "tool_edit" ? currentItem.card : null;
  const currentOther = otherPreview && review.items.some((item) => item.id === otherPreview.id);
  return (
    <>
      {review.items.map((item) => item.kind === "personal_plan" ? (
        <PersonalRuleReview key={item.id} plan={item.plan} />
      ) : item.kind === "tool_edit" ? (
        <ReviewCard key={item.id} card={item.card} answers={item.answers} tools={review.review_tools}
          busy={busy !== null || !!currentReview || !!currentOther} onChoice={(decision, label) => void prepare(decision, label)} />
      ) : (
        <OtherReviewCard key={item.id} item={item} toolName={review.review_tools[item.id]?.name} busy={busy !== null || !!currentReview || !!currentOther}
          onChoice={(action, label) => void prepareOther(item, action, label)} />
      ))}
      {currentOther && otherPreview && createPortal(
        <ConfirmDialog tone="neutral" message={t("rulesSetup.reviewTitle")} details={otherPreview.details}
          confirmLabel={otherPreview.label} cancelLabel={t("rulesSetup.cancel")}
          onAnswer={(yes) => { if (yes) void confirmOther(); else setOtherPreview(null); }} />, document.body,
      )}
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
              <li key={file}>{rulesSetupFileLabel(t, review.review_tools[file], file)}</li>
            ))}</ul>
            <RulesSetupDiff diff={preview.result.diff} codes={preview.result.diff_codes} />
          </>}
          confirmLabel={preview.decision.action === "revert" ? t("rulesSetup.restore") : preview.label}
          cancelLabel={t("rulesSetup.cancel")}
          onAnswer={(yes) => { if (yes) void confirm(); else setPreview(null); }}
        />, document.body,
      )}
    </>
  );
}

export function PersonalRuleReview({ plan }: { readonly plan: PersonalRulePlan }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const heading = useId();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const move = plan.page_merge;

  async function answer(apply: boolean) {
    setBusy(true);
    setFailed(false);
    try {
      if (apply) await applyPersonalRulePlan(plan);
      else await declinePageRuleMerge(plan.id);
    } catch {
      setFailed(true);
    } finally {
      await refreshRulesSetup(client);
      setBusy(false);
    }
  }

  return <article className="rules-setup-review" aria-labelledby={heading}>
    <h2 id={heading}>{move ? t("rulesSetup.movePageRules", { count: move.rule_count }) : t("rulesSetup.reviewPersonalRule")}</h2>
    {plan.proposer && <p>{t("rulesSetup.proposedBy", { agent: toolDisplayName(plan.proposer) })}</p>}
    {move?.excludes_space_only && <p>{t("rulesSetup.keepSpaceRules")}</p>}
    {plan.changes.map(({ diff }, index) => diff && <RulesSetupDiff key={index} diff={diff} />)}
    {!!move?.excluded.length && <>
      <p>{t("rulesSetup.credentialSkipped")}</p>
      <ul className="rules-setup-files">{move.excluded.map(({ page_id, line }) => (
        <li key={`${page_id}:${line}`}>{t("rulesSetup.credentialPosition", { source: page_id ?? t("rulesSetup.conversationPreferences"), line })}</li>
      ))}</ul>
    </>}
    {!!plan.blocked_tools.length && <p>{t("rulesSetup.reviewRemaining")}</p>}
    <div className="rules-setup-actions">
      <Button type="button" variant="primary" disabled={busy || !!plan.blocked_tools.length} onClick={() => void answer(true)}>
        {t(move ? "rulesSetup.moveRules" : "rulesSetup.applyPersonalRule")}
      </Button>
      <Button type="button" disabled={busy} onClick={() => void answer(false)}>{t(move ? "rulesSetup.keepPageRules" : "rulesSetup.no")}</Button>
    </div>
    {failed && <p className="rules-setup-note" role="alert">{t("rulesSetup.changeFailed")}</p>}
  </article>;
}

function ReviewCard({ card, answers, tools, busy, onChoice }: {
  readonly card: PullbackCard;
  readonly answers: PullbackAction[];
  readonly tools: RulesSetupStatus["review_tools"];
  readonly busy: boolean;
  readonly onChoice: (decision: PullbackDecision, label: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const headingId = useId();
  const [source, setSource] = useState(card.files[0]);
  const tool = rulesSetupName(tools[source]?.name) || t("rulesSetup.thisTool");
  const names = [...new Set(card.files.map((file) => rulesSetupName(tools[file]?.name)).filter(Boolean))];
  const [file] = card.files;
  const fileLabel = card.files.length === 1 ? rulesSetupFileLabel(t, tools[file], file) : undefined;
  const scope = card.kind === "scope";
  const all = t("rulesSetup.allTools");
  const only = t("rulesSetup.toolOnly", { tool });
  const no = t("rulesSetup.no");
  const choice = (action: PullbackAction, label: string) =>
    onChoice({ id: card.id, action, ...((action === "tool_only" || action === "all_tools") && card.files.length > 1 ? { file: source } : {}) }, label);

  return (
    <article className="rules-setup-review" aria-labelledby={headingId}>
      <h2 id={headingId}>{names.length === 1
        ? t(scope ? "rulesSetup.ruleChanged" : "rulesSetup.toolChanged", { tool: names[0] })
        : t("rulesSetup.reviewTitle")}</h2>
      {scope && <p>{t("rulesSetup.alreadyApplies", { tool })}</p>}
      {(!scope || card.reason_code !== "scope_edit") &&
        <p className="rules-setup-reason">{rulesSetupMessage(t, card.reason_code)}</p>}
      {card.files.length > 1 ? (
        <label className="rules-setup-source">
          {t("rulesSetup.chooseSource")}
          <select value={source} disabled={busy} onChange={(event) => setSource(event.target.value)}>
            {card.files.map((file) => <option key={file} value={file}>{rulesSetupFileLabel(t, tools[file], file)}</option>)}
          </select>
        </label>
      ) : fileLabel && fileLabel !== names[0] && <p className="rules-setup-files">{fileLabel}</p>}
      <RulesSetupDiff diff={card.diff} codes={card.diff_codes} />
      <div className="rules-setup-actions">
        {answers.includes("all_tools") && <Button type="button" variant="primary" disabled={busy} onClick={() => choice("all_tools", all)}>{all}</Button>}
        {answers.includes("tool_only") && <Button type="button" disabled={busy} onClick={() => choice("tool_only", only)}>{only}</Button>}
        {!scope && answers.includes("revert") && <Button type="button" variant="ghost" disabled={busy} onClick={() => choice("revert", no)}>{no}</Button>}
      </div>
      {!scope && <p className="rules-setup-note">{names.length > 1
        ? t("rulesSetup.noEffectMany", { tools: new Intl.ListFormat(i18n.resolvedLanguage).format(names) })
        : t("rulesSetup.noEffect", { tool })}</p>}
    </article>
  );
}


export function SettingsChanges({ plan }: { readonly plan: RulesSettingsPlan }) {
  const { t } = useTranslation();
  return <>
    {plan.notice && <p>{rulesSetupMessage(t, "recovery_needed")}</p>}
    {plan.files.map((file) => (
      <section key={file.path}>
        <h3>{rulesSetupName(file.tool?.name) || t("rulesSetup.thisTool")}</h3>
        <RulesSetupDiff diff={addedTextDiff(file.diff.split("\n").slice(1).filter((line) => line.startsWith("+")).map((line) => line.slice(1)))} />
      </section>
    ))}
  </>;
}

function OtherReviewCard({ item, toolName, busy, onChoice }: {
  readonly item: OtherReviewItem;
  readonly toolName?: string;
  readonly busy: boolean;
  readonly onChoice: (action: PullbackAction, label: string) => void;
}) {
  const { t } = useTranslation();
  const heading = useId();
  const proposal = item.kind === "skill_proposal" ? item.proposal : null;
  const tool = rulesSetupName(toolName) || t("rulesSetup.thisTool");
  const label = (action: PullbackAction) => {
    if (action === "tool_only") return t("rulesSetup.toolOnly", { tool });
    if (action === "all_tools") return t("rulesSetup.allTools");
    return t("rulesSetup.dismiss");
  };
  return (
    <article className="rules-setup-review" aria-labelledby={heading}>
      <h2 id={heading}>{item.kind === "settings" ? t("rulesSetup.settingsChanges")
        : item.kind === "skill_update" ? t("rulesSetup.updateFor", { name: item.update.name })
        : t("rulesSetup.proposalFor", { name: item.proposal.name })}</h2>
      {item.kind === "settings" && (item.answers.length ? <SettingsChanges plan={item.plan} />
        : <p>{t("rulesSetup.previewFailed")}</p>)}
      {item.kind === "skill_update" && item.update.status.state === "available" && <><p>{item.update.status.revision}</p><p>{t("rulesSetup.dismissUpdateExplanation")}</p></>}
      {proposal?.files.map((file) => file.text !== null && (
        <RulesSetupDiff key={file.path} diff={addedTextDiff(file.text.trimEnd().split("\n"))} />
      ))}
      {proposal?.changes?.removed.map((path) => <p key={path}>{rulesSetupMessage(t, "remove_copy")}</p>)}
      {!!proposal?.changes?.oversized?.length && <p>{rulesSetupMessage(t)}</p>}
      <div className="rules-setup-actions">
        {item.answers.map((action) => (
          <Button key={action} type="button" disabled={busy} onClick={() => onChoice(action, label(action))}>{label(action)}</Button>
        ))}
      </div>
    </article>
  );
}

function addedTextDiff(lines: string[]): string {
  if (!lines.length) return "";
  return ["--- ", "+++ ", `@@ -0,0 +1,${lines.length} @@`, ...lines.map((line) => "+" + line)].join("\n");
}
