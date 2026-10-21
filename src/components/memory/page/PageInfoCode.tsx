// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import type { CodeBasisRef } from "../../../lib/tauri";
import { codeBasisLocation, useCopyLocation } from "./codeBasisShared";
import { OverflowMenu } from "./OverflowMenu";
import { codeBasisKeyOf } from "./useCodeBasisActions";
import "./NoteInfo.css";

interface PageInfoCodeProps {
  codeBasis: readonly CodeBasisRef[];
  onUnmarkCodeBasis?: (row: CodeBasisRef) => void;
}

function CodeRow({ row, onUnmark }: { row: CodeBasisRef; onUnmark?: (row: CodeBasisRef) => void }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopyLocation();
  const location = codeBasisLocation(row);
  return <li className="note-info-notes__row">
    <button
      type="button"
      className="note-info-link-row"
      title={t("codeBasis.copyLocation")}
      onClick={() => { void copy(location); }}
    >
      <span className="note-info-link-copy">
        <span className="note-info-link-label note-info-code-path">{location}</span>
      </span>
    </button>
    <span className="note-info-link-status" role={copied ? "status" : undefined}>
      {copied ? t("codeBasis.copied") : t(`codeBasis.state.${row.state}`)}
    </span>
    {onUnmark && <OverflowMenu
      label={t("codeBasis.moreActions")}
      items={[{ key: "stop", label: t("codeBasis.stopRelying"), onSelect: () => onUnmark(row) }]}
    />}
  </li>;
}

/** The lines of code this note relies on. Code bases are attached by the AI tool that writes the note; here they are only shown and removed. */
export default function PageInfoCode({ codeBasis, onUnmarkCodeBasis }: PageInfoCodeProps) {
  const { t } = useTranslation();
  return (
    <section aria-label={t("codeBasis.groupLabel")} className="note-info-notes">
      <h4 className="note-info-section-heading">{t("codeBasis.group")}</h4>
      <ul className="note-info-source-list">
        {codeBasis.map((row) => <CodeRow key={codeBasisKeyOf("", row)} row={row} onUnmark={onUnmarkCodeBasis} />)}
      </ul>
    </section>
  );
}
