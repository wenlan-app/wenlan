// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { createPortal } from "react-dom";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  adoptRulesSetup, previewRulesAdoption, restoreRulesSetup, syncRulesSetup,
  type RulesAdoptionPlan, type RulesSetupStatus, type RulesSyncReport,
} from "../../../lib/tauri";
import { ConfirmDialog } from "../../ConfirmDialog";
import { Button, Card, SectionHeader, SettingRow } from "../settings/primitives";
import { RulesSetupDiff, rulesSetupFileName } from "./RulesSetupReviews";
import { refreshRulesSetup, useRulesSetup } from "./useRulesSetup";
import "./rulesSetup.css";

export function RulesSetupSettings() {
  const status = useRulesSetup();
  return status ? <EnabledRulesSetupSettings status={status} /> : null;
}

function EnabledRulesSetupSettings({ status }: { readonly status: RulesSetupStatus }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [dialog, setDialog] = useState<RulesAdoptionPlan | "restore" | null>(null);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ failed: boolean; message: string; remaining?: string[] } | null>(null);

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
        setFeedback({ failed: false, message: t("rulesSetup.setupRemaining"), remaining: result.blocked });
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
              {feedback.remaining.map((line, index) => <li key={index}>{line}</li>)}
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
            <li key={tool.name}><strong>{tool.name}</strong><span>{counts(tool)}</span></li>
          ))}</ul>
        </section>}
      {(skipped.length > 0 || !!plan.skipped_tools?.length) &&
        <section>
          <h3>{t("rulesSetup.skipped")}</h3>
          <ul className="rules-setup-plan-list">
            {plan.skipped_tools?.map((reason) => <li key={reason}>{reason}</li>)}
            {skipped.map((file) => (
            <li key={file.path}><strong>{rulesSetupFileName(file.path)}</strong><span>{file.diff.trim()}</span></li>
          ))}</ul>
        </section>}
      {planned.length === 0 && <p>{t("rulesSetup.nothingToAdopt")}</p>}
      <p>{t("rulesSetup.sharedUntouched")}</p>
      {planned.length > 0 && <details>
        <summary>{t("rulesSetup.fileChanges")}</summary>
        {planned.map((file) => (
          <section key={file.path}>
            <h3>{rulesSetupFileName(file.path)}</h3>
            <RulesSetupDiff diff={file.diff} />
          </section>
        ))}
      </details>}
      {plan.skills.length > 0 && <details>
        <summary>{t("rulesSetup.skillSources")}</summary>
        <ul className="rules-setup-plan-list">{plan.skills.map((skill) => (
          <li key={skill.path}>
            <strong>{rulesSetupFileName(skill.path)}</strong>
            <span>{skill.outcome === "skipped" ? skill.reason : skill.outcome === "plain_copy"
              ? t("rulesSetup.plainSkill")
              : t("rulesSetup.trackedSkill", { source: skill.upstream.kind === "github" ? skill.upstream.repository : skill.upstream.package })}
              {skill.byte_copy_reason && <> · {skill.byte_copy_reason}</>}
            </span>
          </li>
        ))}</ul>
      </details>}
      {plan.notes?.some((note) => !plan.skipped_tools?.includes(note)) && <details>
        <summary>{t("rulesSetup.planNotes")}</summary>
        <ul className="rules-setup-files">{plan.notes.filter((note) => !plan.skipped_tools?.includes(note)).map((note, index) => <li key={index}>{note}</li>)}</ul>
      </details>}
    </div>
  );
}
