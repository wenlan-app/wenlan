// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { createPortal } from "react-dom";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  adoptRulesSetup, previewRulesAdoption, restoreRulesSetup, syncRulesSetup,
  type RulesAdoptionPlan, type RulesSetupNotice, type RulesSetupStatus, type RulesSyncReport,
} from "../../../lib/tauri";
import { ConfirmDialog } from "../../ConfirmDialog";
import { Button, Card, SectionHeader, SettingRow } from "../settings/primitives";
import { RulesSetupDiff } from "./RulesSetupDiff";
import { rulesSetupFileLabel, rulesSetupMessage, rulesSetupName, rulesSetupNotice, rulesSetupSkillSource } from "./rulesSetupText";
import { refreshRulesSetup, useRulesSetup } from "./useRulesSetup";
import "./rulesSetup.css";

export function RulesSetupSettings() {
  const status = useRulesSetup();
  return status ? <EnabledRulesSetupSettings status={status} /> : null;
}

function EnabledRulesSetupSettings({ status }: { readonly status: RulesSetupStatus }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const [dialog, setDialog] = useState<RulesAdoptionPlan | "restore" | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ failed: boolean; message: string; remaining?: (RulesSetupNotice | undefined)[] } | null>(null);

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
      if (result.blocked.length > 0) {
        setFeedback({ failed: false, message: t("rulesSetup.setupRemaining"), remaining: result.blocked.map((_, index) => result.blocked_codes?.[index]) });
      } else {
        setFeedback({ failed: false, message: success });
      }
    } catch {
      setFeedback({ failed: true, message: t("rulesSetup.changeFailed") });
    } finally {
      await refreshRulesSetup(client, refreshSkills);
      setBusy(false);
    }
  }

  const canConfirmAdoption = dialog !== null && dialog !== "restore" && dialog.files.some((file) => !dialog.skipped_files?.includes(file.path));
  return (
    <section className="rules-setup-settings">
      <SectionHeader label={t("rulesSetup.settingsTitle")} />
      <Card>
        {status.can_adopt && <SettingRow
          title={t("rulesSetup.adoptTitle")}
          description={t("rulesSetup.adoptHint")}
          control={<Button type="button" disabled={busy} onClick={() => void preview()}>{t("rulesSetup.adoptOpen")}</Button>}
        />}
        {status.adopted && <SettingRow
          title={t("rulesSetup.syncTitle")}
          description={t("rulesSetup.syncHint")}
          control={<Button type="button" disabled={busy}
            onClick={() => void run(syncRulesSetup, t("rulesSetup.syncDone"))}>{t("rulesSetup.sync")}</Button>}
        />}
        {status.can_restore && <SettingRow
          title={t("rulesSetup.restore")}
          description={t("rulesSetup.restoreHint")}
          control={<Button type="button" disabled={busy} onClick={() => setDialog("restore")}>{t("rulesSetup.restoreOpen")}</Button>}
        />}
        {feedback && (
          <div className="rules-setup-feedback" role={feedback.failed ? "alert" : "status"}>
            <p>{feedback.message}</p>
            {feedback.remaining && <ul className="rules-setup-files">
              {feedback.remaining.map((notice, index) => <li key={index}>{rulesSetupNotice(t, i18n.language, notice)}</li>)}
            </ul>}
          </div>
        )}
      </Card>
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
      {planned.length === 0 && <p>{t("rulesSetup.nothingToAdopt")}</p>}
      <p>{t("rulesSetup.sharedUntouched")}</p>
      {planned.length > 0 && <details>
        <summary>{t("rulesSetup.fileChanges")}</summary>
        {planned.map((file) => (
          <section key={file.path}>
            <h3>{rulesSetupFileLabel(t, file.tool, file.path)}</h3>
            {file.text_diff === undefined ? <p>{rulesSetupMessage(t)}</p> :
              <RulesSetupDiff diff={file.text_diff.kind === "extracted" ? file.text_diff.diff : file.diff}
                tool={file.tool} codes={file.notice_codes} />}
          </section>
        ))}
      </details>}
      {plan.skills.length > 0 && <details>
        <summary>{t("rulesSetup.skillSources")}</summary>
        <ul className="rules-setup-plan-list">{plan.skills.map((skill) => (
          <li key={skill.path}>
            <strong>{skill.tool ? rulesSetupFileLabel(t, skill.tool, skill.path) : t("rulesSetup.sharedSource")}</strong>
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
