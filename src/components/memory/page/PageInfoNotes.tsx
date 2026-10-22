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
  /**
   * The note's Space. Notes in it come first; notes in other named Spaces
   * follow with their Space shown, the same notes the `[[` menu offers.
   */
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
    queryFn: async () => {
      if (term) return searchPages(term, PICKER_LIMIT);
      // The own Space first, so a busy library elsewhere cannot crowd it out.
      const [own, all] = await Promise.all([
        listPages("active", space ?? undefined, PICKER_LIMIT),
        listPages("active", undefined, PICKER_LIMIT),
      ]);
      return [...own, ...all];
    },
  });
  const taken = new Set(basis.map((entry) => entry.page_id));
  const seen = new Set<string>();
  const choices = (candidates.data ?? []).filter((page: Page) => {
    if (seen.has(page.id)) return false;
    seen.add(page.id);
    return page.status === "active"
      && page.id !== pageId
      && !taken.has(page.id)
      && page.creation_kind !== "entity"
      // A note in no Space is reachable only from no Space, as for `[[`.
      && ((page.space ?? null) === space || !!page.space);
  }).sort((a, b) => Number((a.space ?? null) !== space) - Number((b.space ?? null) !== space));

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
              {(page.space ?? null) !== space && <span className="note-info-link-status note-info-notes__space">{page.space}</span>}
            </button>
          </li>)}
        </ul>
        {failed && <p role="alert" className="note-info-notes__hint">{t("noteBasis.markFailed")}</p>}
      </div>}
    </section>
  );
}
