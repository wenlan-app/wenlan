// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { createPortal } from "react-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  adoptRulesSetup, previewRulesAdoption, restoreRulesSetup, syncRulesSetup, getRulesDelivery,
  type RulesAdoptionPlan, type RulesSetupNotice, type RulesSetupStatus, type RulesSyncReport,
} from "../../../lib/tauri";
import { ConfirmDialog } from "../../ConfirmDialog";
import { Button, Card } from "../settings/primitives";
import { PersonalRuleReview, RulesSetupReviews, SettingsChanges } from "./RulesSetupReviews";
import { RulesToolsHeader } from "./RulesToolsHeader";
import { RulesSetupDiff } from "./RulesSetupDiff";
import { rulesSetupFileLabel, rulesSetupMessage, rulesSetupName, rulesSetupNotice, rulesSetupSkillSource } from "./rulesSetupText";
import { RULES_DELIVERY_QUERY_KEY, refreshRulesSetup, useRulesSetup, useRulesSetupReview } from "./useRulesSetup";
import "./rulesSetup.css";

export function RulesSetupSettings({ skillUpdates, onSkills = false }: { readonly skillUpdates?: number; readonly onSkills?: boolean } = {}) {
  const status = useRulesSetup();
  return status ? <EnabledRulesSetupSettings status={status} skillUpdates={skillUpdates} onSkills={onSkills} /> : null;
}

function EnabledRulesSetupSettings({ status, skillUpdates, onSkills }: { readonly status: RulesSetupStatus; readonly skillUpdates?: number; readonly onSkills: boolean }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const [panel, setPanel] = useState<"status" | "review">("status");
  const review = useRulesSetupReview(true);
  const [dialog, setDialog] = useState<RulesAdoptionPlan | "restore" | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ failed: boolean; message: string; notices?: (RulesSetupNotice | undefined)[] } | null>(null);

  async function preview() {
    setBusy(true);
    setFeedback(null);
    try {
      setDialog(await previewRulesAdoption());
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.previewFailed") });
    } finally {
      setBusy(false);
    }
  }

  async function run(action: () => Promise<RulesSyncReport>, success: string, refreshSkills = false) {
    setDialog(null);
    setBusy(true);
    setFeedback(null);
    try {
      const result = await action();
      setFeedback({
        failed: false,
        message: result.blocked.length ? t("rulesSetup.setupRemaining") : success,
        notices: [...(result.notice_codes ?? []), ...result.blocked.map((_, index) => result.blocked_codes?.[index])],
      });
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.changeFailed") });
    } finally {
      await refreshRulesSetup(client, refreshSkills);
      setBusy(false);
    }
  }

  const canConfirmAdoption = dialog !== null && dialog !== "restore" && (
    dialog.files.some((file) => !dialog.skipped_files?.includes(file.path)) ||
    !!dialog.settings?.files.length || !!dialog.settings?.notice
  );
  const count = review.data?.items.length ?? null;
  const delivery = useQuery({ queryKey: RULES_DELIVERY_QUERY_KEY, queryFn: getRulesDelivery, staleTime: 30_000, retry: false });
  const connected = new Set((delivery.data ?? []).filter((tool) => tool.connected).map((tool) => tool.id));
  const tools = delivery.isSuccess
    ? (status.tools ?? []).filter((tool) => connected.has(tool.id))
    : (status.tools ?? []);
  const deliveryProblems = new Set((delivery.isSuccess ? delivery.data : []).filter((tool) => tool.connected && ["out_of_date", "blocked", "over_limit", "folder_missing", "setting_off", "isolated_run", "on_request"].includes(tool.state)).map((tool) => tool.id));
  const problems = tools.filter((tool) => tool.state !== "up_to_date" || deliveryProblems.has(tool.id));
  const lastCheck = status.last_checked_at ? new Date(status.last_checked_at * 1000) : null;
  const list = new Intl.ListFormat(i18n.resolvedLanguage, { type: "unit" });
  return (
    <section className="rules-setup-settings" aria-label={t("rulesSetup.settingsTitle")}>
      <header className="rules-setup-actions">
        {status.can_adopt && (!status.adopted || problems.some((tool) => tool.state === "not_managed")) && <Button type="button" disabled={busy} onClick={() => void preview()}>{t("rulesSetup.adoptOpen")}</Button>}
        <Button type="button" variant={panel === "status" ? "primary" : "secondary"}
          aria-pressed={panel === "status"} onClick={() => setPanel("status")}>{t("rulesSetup.statusTitle")}</Button>
        <Button type="button" variant={panel === "review" ? "primary" : "secondary"}
          aria-pressed={panel === "review"} onClick={() => setPanel("review")}>
          {t("rulesSetup.reviewAction")}{!!count && <> <span className="rules-setup-count">{count}</span></>}
        </Button>
        {status.can_restore && <Button type="button" disabled={busy} onClick={() => setDialog("restore")}>{t("rulesSetup.restoreOpen")}</Button>}
      </header>
      <RulesToolsHeader delivers setupOnly onSkills={onSkills} skillUpdates={skillUpdates} reviewCount={count} onReview={() => setPanel("review")} />
      {panel === "status" && review.data?.items.map((item) => item.kind === "personal_plan" && item.plan.page_merge
        ? <PersonalRuleReview key={item.id} plan={item.plan} /> : null)}
      {panel === "status" ? <Card>
        <div className="rules-setup-status-panel">
          {lastCheck && <p>{t("rulesSetup.lastCheck", { date: new Intl.DateTimeFormat(i18n.resolvedLanguage, {
            dateStyle: "medium", timeStyle: "short",
          }).format(lastCheck) })}</p>}
          {tools.length > 0 && (problems.length === 0
            // Shared rules changed since the last sync: the header already
            // says a sync is needed, so the card never claims all is current.
            ? (status.adopted && !status.in_sync ? null
              : <p className="rules-setup-status-summary">{t("rulesSetup.allToolsUpToDate", { count: tools.length })}</p>)
            : <ul className="rules-setup-plan-list">{problems.map((tool) => {
              const counts = [
                tool.counts.rules && t("rulesSetup.ruleCount", { count: tool.counts.rules }),
                tool.counts.skills && t("rulesSetup.skills", { count: tool.counts.skills }),
                tool.counts.commands && t("rulesSetup.commandCount", { count: tool.counts.commands }),
                tool.counts.subagents && t("rulesSetup.agentCount", { count: tool.counts.subagents }),
                tool.counts.mcp && t("rulesSetup.mcpCount", { count: tool.counts.mcp }),
              ].filter((value): value is string => typeof value === "string");
              return <li key={tool.id} title={counts.length ? list.format(counts) : undefined}>
                <strong>{rulesSetupName(tool.name) || t("rulesSetup.thisTool")}</strong>
                <span>{tool.state === "up_to_date" ? t("rulesSetup.statusNotDelivered") : t(`rulesSetup.toolStatus.${tool.state}`)} · {t(tool.state === "needs_review" ? "rulesSetup.statusReviewAction" : tool.state === "not_managed" ? "rulesSetup.statusAdoptAction" : "rulesSetup.statusSyncAction")}</span>
              </li>;
            })}</ul>)}
          {status.adopted && <div className="rules-setup-actions">
            {/* Everything is current: Sync stays available but quiet. */}
            <Button type="button" disabled={busy}
              {...(status.in_sync && tools.length > 0 && problems.length === 0 ? { variant: "ghost" as const, size: "sm" as const } : {})}
              onClick={() => void run(syncRulesSetup, t("rulesSetup.syncDone"))}>{t("rulesSetup.sync")}</Button>
          </div>}
        </div>
      </Card> : <>
        {review.isError && <p role="alert">{t("rulesSetup.reviewLoadFailed")}</p>}
        {review.isSuccess && (review.data.items.length ? <RulesSetupReviews review={review.data} />
          : <p className="rules-setup-note">{t("rulesSetup.reviewEmpty")}</p>)}
      </>}
        {feedback && (
          <div className="rules-setup-feedback" role={feedback.failed ? "alert" : "status"} aria-label={feedback.message}>
            <p>{feedback.message}</p>
            {!!feedback.notices?.length && <ul className="rules-setup-files">
              {feedback.notices.map((notice, index) => <li key={index}>{rulesSetupNotice(t, i18n.language, notice)}</li>)}
            </ul>}
          </div>
        )}
      {dialog && createPortal(dialog === "restore" ? (
        <ConfirmDialog
          message={t("rulesSetup.restoreQuestion")}
          details={<><p>{t("rulesSetup.restoreOriginals")}</p><p>{t("rulesSetup.restoreLaterEdits")}</p></>}
          confirmLabel={t("rulesSetup.restore")}
          cancelLabel={t("rulesSetup.cancel")}
          onAnswer={(yes) => { if (yes) void run(restoreRulesSetup, t("rulesSetup.restoreDone"), true); else setDialog(null); }}
        />
      ) : (
        <ConfirmDialog
          message={t("rulesSetup.adoptPreviewTitle")}
          details={<AdoptionDetails plan={dialog} />}
          tone="neutral"
          confirmLabel={t(canConfirmAdoption ? "rulesSetup.adopt" : "rulesSetup.close")}
          cancelLabel={canConfirmAdoption ? t("rulesSetup.cancel") : undefined}
          onAnswer={(yes) => {
            if (yes && canConfirmAdoption) void run(() => adoptRulesSetup(dialog.token), t("rulesSetup.adoptDone"), true);
            else setDialog(null);
          }}
        />
      ), document.body)}
    </section>
  );
}

function AdoptionDetails({ plan }: { readonly plan: RulesAdoptionPlan }) {
  const { t, i18n } = useTranslation();
  const skippedPaths = new Set(plan.skipped_files);
  const skipped = plan.files.filter((file) => skippedPaths.has(file.path));
  const planned = plan.files.filter((file) => !skippedPaths.has(file.path));
  const list = new Intl.ListFormat(i18n.resolvedLanguage, { type: "unit" });
  const notes = plan.notes?.flatMap((note, index) =>
    plan.skipped_tools?.includes(note) ? [] : [plan.note_codes?.[index]]) ?? [];
  const counts = (tool: NonNullable<RulesAdoptionPlan["tools"]>[number]) => list.format([
    ...(tool.rule_files ? [t("rulesSetup.ruleFiles", { count: tool.rule_files })] : []),
    ...(tool.skills ? [t("rulesSetup.skills", { count: tool.skills })] : []),
    ...(tool.other_files ? [t("rulesSetup.otherFiles", { count: tool.other_files })] : []),
  ]);
  return (
    <div className="rules-setup-adoption">
      {plan.tools && plan.tools.length > 0 &&
        <section>
          <h3>{t("rulesSetup.updatedTools")}</h3>
          <ul className="rules-setup-plan-list">{plan.tools.map((tool) => (
            <li key={tool.name}><strong>{rulesSetupName(tool.name) || t("rulesSetup.thisTool")}</strong><span>{counts(tool)}</span></li>
          ))}</ul>
        </section>}
      {(skipped.length > 0 || !!plan.skipped_tools?.length) &&
        <section>
          <h3>{t("rulesSetup.skipped")}</h3>
          <ul className="rules-setup-plan-list">
            {plan.skipped_tools?.map((_, index) => (
              <li key={index}>{rulesSetupNotice(t, i18n.language, plan.skipped_tool_codes?.[index])}</li>
            ))}
            {skipped.map((file) => (
              <li key={file.path}>
                <strong>{rulesSetupFileLabel(t, file.tool, file.path)}</strong>
                <span>{rulesSetupMessage(t, file.notice_codes?.[0])}</span>
              </li>
            ))}</ul>
        </section>}
      {planned.length === 0 && !plan.settings?.files.length && <p>{t("rulesSetup.nothingToAdopt")}</p>}
      {plan.rule_sources?.map((path) => <p className="break-all" key={path}>{t("rulesSetup.sharedUntouched", { path })}</p>)}
      {planned.length > 0 && <details>
        <summary>{t("rulesSetup.fileChanges")}</summary>
        {planned.map((file) => (
          <section key={file.path}>
            <h3>{rulesSetupFileLabel(t, file.tool, file.path)}</h3>
            {file.text_diff === undefined ? <p>{rulesSetupMessage(t)}</p> :
              <RulesSetupDiff diff={file.text_diff.kind === "extracted" ? file.text_diff.diff : file.diff}
                codes={file.notice_codes} />}
          </section>
        ))}
      </details>}
      {plan.settings && <details open>
        <summary>{t("rulesSetup.settingsChanges")}</summary>
        <SettingsChanges plan={plan.settings} />
      </details>}
      {plan.skills.length > 0 && <details>
        <summary>{t("rulesSetup.skillSources")}</summary>
        <ul className="rules-setup-plan-list">{plan.skills.map((skill) => (
          <li key={skill.path}>
            <strong>{rulesSetupFileLabel(t, skill.tool, skill.path, "rulesSetup.sharedSource")}</strong>
            <span>{skill.outcome === "skipped" ? rulesSetupMessage(t, skill.reason_code) : skill.outcome === "plain_copy"
              ? t("rulesSetup.plainSkill")
              : t("rulesSetup.trackedSkill", { source: rulesSetupSkillSource(
                skill.upstream.kind === "github" ? skill.upstream.repository : skill.upstream.package,
              ) || (skill.upstream.kind === "github" ? "GitHub" : "npm") })}
              {(skill.byte_copy_reason_code || skill.byte_copy_reason) && <> · {rulesSetupMessage(t, skill.byte_copy_reason_code)}</>}
            </span>
          </li>
        ))}</ul>
      </details>}
      {notes.length > 0 && <details>
        <summary>{t("rulesSetup.planNotes")}</summary>
        <ul className="rules-setup-files">{notes.map((notice, index) => <li key={index}>{rulesSetupNotice(t, i18n.language, notice)}</li>)}</ul>
      </details>}
    </div>
  );
}
