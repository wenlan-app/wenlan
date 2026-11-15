// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { setSkillsFromLibraryEnabled, skillsFromLibraryEnabled } from "../../../lib/tauri";
import { Card, Toggle } from "../settings/primitives";
import { HOW_I_WORK_QUERY_KEYS } from "./howIWorkModel";

/** The one switch for how-I-work rules (UX-SKILL-1). The ask is shown once
 *  for the whole library; this is where the person changes that answer. */
export function HowIWorkSetting() {
  const { t } = useTranslation();
  const titleId = useId();
  const detailId = useId();
  const queryClient = useQueryClient();
  const enabled = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.enabled,
    queryFn: skillsFromLibraryEnabled,
    retry: false,
  });
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const value = enabled.isSuccess ? enabled.data : undefined;

  const toggle = async () => {
    if (value === undefined || saving) return;
    setSaving(true);
    setFailed(false);
    try {
      await setSkillsFromLibraryEnabled(!value);
      queryClient.setQueryData(HOW_I_WORK_QUERY_KEYS.enabled, !value);
      void queryClient.invalidateQueries({ queryKey: HOW_I_WORK_QUERY_KEYS.spaceSuggestions });
    } catch {
      setFailed(true);
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="mem-fade-up" data-testid="how-i-work-setting">
      <Card padding="none">
        <div className="settings-row flex items-center justify-between gap-4 px-5 py-4">
          <div className="min-w-0">
            <div
              id={titleId}
              style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-md)", fontWeight: 500, color: "var(--mem-text)" }}
            >
              {t("howIWork.setting.title")}
            </div>
            <p id={detailId} className="mt-0.5 text-[length:var(--mem-text-xs)] text-[var(--mem-text-tertiary)]">
              {t("howIWork.setting.detail")}
            </p>
          </div>
          <Toggle
            enabled={value === true}
            valueUnknown={value === undefined || saving}
            unknownState={saving ? "saving" : enabled.isPending ? "loading" : "unavailable"}
            onToggle={() => void toggle()}
            aria-label={t("howIWork.setting.title")}
            aria-describedby={detailId}
          />
        </div>
        {failed ? (
          <p className="px-5 pb-4 text-[length:var(--mem-text-sm)] text-[var(--mem-status-danger-text)]" role="alert">
            {t("howIWork.setting.failed")}
          </p>
        ) : null}
      </Card>
    </section>
  );
}
