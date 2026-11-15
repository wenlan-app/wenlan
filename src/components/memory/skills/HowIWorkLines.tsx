// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { SkillLine } from "../../../lib/tauri";
import { visibleSourceTitle } from "./howIWorkModel";

type Props = {
  readonly lines: readonly SkillLine[];
  readonly onOpenPage: (pageId: string) => void;
  readonly onEditLine?: (line: SkillLine, text: string) => Promise<boolean>;
  /** The one-time ask lists candidates, which nobody has edited yet: always show the source. */
  readonly showEdited?: boolean;
};

export function HowIWorkLines({ lines, onOpenPage, onEditLine, showEdited = true }: Props) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);

  const begin = (line: SkillLine) => {
    if (!onEditLine || saving) return;
    setEditing(line.id);
    setDraft(line.text);
  };
  const save = async (line: SkillLine) => {
    if (!onEditLine || saving) return;
    setSaving(true);
    const saved = await onEditLine(line, draft);
    setSaving(false);
    if (saved) setEditing(null);
  };

  return (
    <ul className="how-i-work-lines" data-testid="how-i-work-lines">
      {lines.map((line) => {
        const source = visibleSourceTitle(line.source_page_title, t);
        return (
          <li className="how-i-work-line" data-kind={line.kind} key={line.id}>
            <span className="how-i-work-chip">{t(`howIWork.kind.${line.kind}`)}</span>
            <div className="how-i-work-line-copy">
              {editing === line.id ? (
                <input
                  autoFocus
                  aria-label={t("howIWork.page.editLineLabel")}
                  className="how-i-work-line-input"
                  disabled={saving}
                  value={draft}
                  onBlur={() => { if (!saving) setEditing(null); }}
                  onChange={(event) => setDraft(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Escape") setEditing(null);
                    if (event.key === "Enter") {
                      event.preventDefault();
                      void save(line);
                    }
                  }}
                />
              ) : onEditLine ? (
                <button
                  aria-label={t("howIWork.page.editLine", { line: line.text })}
                  className="how-i-work-line-text how-i-work-line-edit"
                  onClick={() => begin(line)}
                  type="button"
                >
                  {line.text}
                </button>
              ) : (
                <p className="how-i-work-line-text">{line.text}</p>
              )}
              {showEdited && line.person_edited ? (
                <p className="how-i-work-line-source">{t("howIWork.page.youChanged")}</p>
              ) : (
                <p className="how-i-work-line-source">
                  {t("howIWork.sourcePrefix")}{" "}
                  <button type="button" onClick={() => onOpenPage(line.source_page_id)}>{source}</button>
                </p>
              )}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
