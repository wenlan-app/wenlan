// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { PageIcon } from "../assets/PageIcon";
import { listPages, searchPages, type BasisRef, type Page } from "../../../lib/tauri";
import { OverflowMenu } from "./OverflowMenu";
import "./NoteInfo.css";

interface PageInfoNotesProps {
  pageId: string;
  /** The note's Space. Only notes in the same Space can be relied on. */
  space: string | null;
  basis: readonly BasisRef[];
  onPageClick?: (pageId: string) => void;
  onMarkBasis: (pageId: string) => Promise<boolean>;
  onUnmarkBasis: (pageId: string) => void;
}

const PICKER_LIMIT = 40;

/** Notes that this note relies on, and a way to add one. */
export default function PageInfoNotes({
  pageId,
  space,
  basis,
  onPageClick,
  onMarkBasis,
  onUnmarkBasis,
}: PageInfoNotesProps) {
  const { t } = useTranslation();
  const [picking, setPicking] = useState(false);
  const [search, setSearch] = useState("");
  const [failed, setFailed] = useState(false);
  const term = search.trim();

  const candidates = useQuery({
    queryKey: ["basis-picker", space, term],
    enabled: picking,
    staleTime: 10_000,
    retry: false,
    queryFn: () => (term ? searchPages(term, PICKER_LIMIT) : listPages("active", space ?? undefined, PICKER_LIMIT)),
  });
  const taken = new Set(basis.map((entry) => entry.page_id));
  const choices = (candidates.data ?? []).filter((page: Page) =>
    page.status === "active"
    && page.id !== pageId
    && !taken.has(page.id)
    && page.creation_kind !== "entity"
    && (page.space ?? null) === space);

  const add = (target: string) => {
    setFailed(false);
    void onMarkBasis(target).then((ok) => {
      if (ok) {
        setPicking(false);
        setSearch("");
      } else {
        setFailed(true);
      }
    });
  };

  return (
    <section aria-label={t("noteBasis.groupLabel")} className="note-info-notes">
      <h4 className="note-info-section-heading">{t("noteBasis.group")}</h4>
      {basis.length === 0 && <p className="note-info-notes__hint">{t("noteBasis.emptyHint")}</p>}
      {basis.length > 0 && <ul className="note-info-source-list">
        {basis.map((entry) => <li key={entry.page_id} className="note-info-notes__row">
          <button
            type="button"
            className="note-info-link-row"
            disabled={!onPageClick || entry.state === "deleted"}
            onClick={() => onPageClick?.(entry.page_id)}
          >
            <span className="note-info-link-copy">
              <PageIcon />
              <span className="note-info-link-label">{entry.title || t("noteBasis.untitled")}</span>
            </span>
          </button>
          <span className="note-info-link-status">{t(`noteBasis.state.${entry.state}`)}</span>
          <OverflowMenu
            label={t("noteBasis.moreActions")}
            items={[{ key: "stop", label: t("noteBasis.stopUsing"), onSelect: () => onUnmarkBasis(entry.page_id) }]}
          />
        </li>)}
      </ul>}
      <button
        type="button"
        className="note-info-notes__add"
        aria-expanded={picking}
        onClick={() => { setPicking((open) => !open); setFailed(false); }}
      >{t("noteBasis.addNote")}</button>
      {picking && <div className="note-info-notes__picker" role="group" aria-label={t("noteBasis.pickerLabel")}>
        <input
          type="search"
          className="note-info-notes__search"
          aria-label={t("noteBasis.pickerSearch")}
          placeholder={t("noteBasis.pickerSearch")}
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
        {candidates.isError && <p role="alert" className="note-info-notes__hint">{t("noteBasis.pickerError")}</p>}
        {!candidates.isError && !candidates.isLoading && choices.length === 0
          && <p className="note-info-notes__hint">{t("noteBasis.pickerEmpty")}</p>}
        <ul className="note-info-source-list">
          {choices.map((page) => <li key={page.id}>
            <button type="button" className="note-info-link-row" onClick={() => add(page.id)}>
              <span className="note-info-link-copy">
                <PageIcon />
                <span className="note-info-link-label">{page.title}</span>
              </span>
            </button>
          </li>)}
        </ul>
        {failed && <p role="alert" className="note-info-notes__hint">{t("noteBasis.markFailed")}</p>}
      </div>}
    </section>
  );
}
