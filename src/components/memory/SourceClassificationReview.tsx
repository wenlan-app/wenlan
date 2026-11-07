// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import type { ClassificationChange } from "../../lib/researchApi";

export function SourceClassificationReview({ changes, editing, onChange }: {
  changes: ClassificationChange[];
  editing: boolean;
  onChange: (changes: ClassificationChange[]) => void;
}) {
  const { t } = useTranslation();
  const parse = (value: string) => value.split(",").map(tag => tag.trim()).filter(Boolean);
  const fields = [
    { field: "add_tags" as const, label: t("selectedSources.classificationAdd") },
    { field: "remove_tags" as const, label: t("selectedSources.classificationRemove") },
  ];
  return <div style={{ display: "grid", gap: 12 }}>
    {editing && <p>{t("selectedSources.classificationEditHint")}</p>}
    {changes.map((change, index) => <section key={`${change.source}:${change.source_id}`} style={{ border: "1px solid var(--mem-border)", borderRadius: "var(--mem-r-8)", padding: 12 }}>
      <strong>{change.title || change.source_id}</strong>
      <p>{t("selectedSources.classificationBefore")}: {change.before_tags?.join(", ") || t("selectedSources.classificationNone")}</p>
      {fields.map(({ field, label }) => <label key={field} style={{ display: "block" }}>
        {label}: {editing
          ? <input aria-label={`${change.title || change.source_id} ${label}`} defaultValue={change[field].join(", ")} onChange={event => onChange(changes.map((entry, i) => i === index ? { ...entry, [field]: parse(event.target.value) } : entry))} />
          : change[field].join(", ") || t("selectedSources.classificationNone")}
      </label>)}
      <p>{t("selectedSources.classificationReason")}: {change.reason}</p>
    </section>)}
  </div>;
}
