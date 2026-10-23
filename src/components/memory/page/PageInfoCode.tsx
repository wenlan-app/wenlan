// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import type { CodeBasisRef } from "../../../lib/tauri";
import { codeBasisLocation, codeBasisLocationLabel, codeBasisQuietNoteText, useCopyLocation } from "./codeBasisShared";
import { OverflowMenu } from "./OverflowMenu";
import { codeBasisKeyOf, useEverydayModelAvailable } from "./useCodeBasisActions";
import "./NoteInfo.css";

interface PageInfoCodeProps {
  codeBasis: readonly CodeBasisRef[];
  onUnmarkCodeBasis?: (row: CodeBasisRef) => void;
}

interface CodeRowProps {
  row: CodeBasisRef;
  modelAvailable: boolean;
  onUnmark?: (row: CodeBasisRef) => void;
}

function CodeRow({ row, modelAvailable, onUnmark }: CodeRowProps) {
  const { t } = useTranslation();
  const { copied, copy } = useCopyLocation();
  // A changed row that asks nothing of the person still says, quietly, what is being done about it.
  const quietNote = codeBasisQuietNoteText(t, row, modelAvailable);
  // Shown as a range, copied as `path:line_start`: the place an editor opens.
  const location = codeBasisLocation(row);
  const label = codeBasisLocationLabel(row);
  // The label is `path:20–30`: the path stays mono, the range after it is set
  // in the UI face so the en dash does not read as a hyphen.
  const rangeAt = row.line_end > row.line_start ? label.lastIndexOf(`:${row.line_start}`) : -1;
  return <li className="note-info-notes__row note-info-code-row">
    <button
      type="button"
      className="note-info-link-row"
      title={t("codeBasis.copyLocation")}
      onClick={() => { void copy(location); }}
    >
      <span className="note-info-link-copy">
        <span className="note-info-link-label note-info-code-path">{rangeAt < 0 ? label : <>
          {label.slice(0, rangeAt)}
          <span className="note-info-code-range">{label.slice(rangeAt)}</span>
        </>}</span>
      </span>
    </button>
    <span className="note-info-link-status" role={copied ? "status" : undefined}>
      {copied
        ? t("codeBasis.copied")
        : t(row.reason === "file_deleted" ? "codeBasis.state.deleted" : `codeBasis.state.${row.state}`)}
    </span>
    {onUnmark && <OverflowMenu
      label={t("codeBasis.moreActions")}
      items={[{ key: "stop", label: t("codeBasis.stopRelying"), onSelect: () => onUnmark(row) }]}
    />}
    {quietNote && <p className="note-info-code-ai">{quietNote}</p>}
  </li>;
}

/** The lines of code this note relies on. Code bases are attached by the AI tool that writes the note; here they are only shown and removed. */
export default function PageInfoCode({ codeBasis, onUnmarkCodeBasis }: PageInfoCodeProps) {
  const { t } = useTranslation();
  const modelAvailable = useEverydayModelAvailable();
  return (
    <section aria-label={t("codeBasis.groupLabel")} className="note-info-notes">
      <h4 className="note-info-section-heading">{t("codeBasis.group")}</h4>
      <ul className="note-info-source-list">
        {codeBasis.map((row) => <CodeRow
          key={codeBasisKeyOf("", row)}
          row={row}
          modelAvailable={modelAvailable}
          onUnmark={onUnmarkCodeBasis}
        />)}
      </ul>
    </section>
  );
}
