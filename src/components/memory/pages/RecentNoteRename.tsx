// SPDX-License-Identifier: AGPL-3.0-only
import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { PageIcon } from "../assets/PageIcon";
import { getPage, renamePage, type Page } from "../../../lib/tauri";
import { canRenameFromList } from "./RecentNoteMenu";

// Queries a rename can change; mirrors the page's own Rename action.
const RENAME_INVALIDATIONS: readonly (readonly string[])[] = [
  ["pages"], ["searchPages"], ["search"], ["knowledge-graph"], ["constellation-cartography"],
  ["recent-pages"], ["space-pages"], ["spaces-page-counts"], ["sidebar-space-page-counts"], ["page-links"],
];

/** Inline rename for a Recently used row. `onDone` gets the new title, or null when nothing changed. */
export function RecentNoteRename({ page, onDone }: {
  readonly page: Page;
  readonly onDone: (title: string | null) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState(page.title);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  // Blur follows Enter and Escape; only the first of them may finish the rename.
  const doneRef = useRef(false);
  const finish = (title: string | null) => {
    if (doneRef.current) return;
    doneRef.current = true;
    onDone(title);
  };

  const submit = async () => {
    if (pending || doneRef.current) return;
    const title = draft.trim();
    if (!title || title === page.title) { finish(null); return; }
    setPending(true); setError(false);
    try {
      // The menu's copy may be older than an edit made since; rename the latest version.
      const latest = await getPage(page.id, "explicit");
      if (!latest || !canRenameFromList(latest)) throw new Error("page cannot be renamed");
      const renamed = await renamePage(latest.id, title, latest.version);
      queryClient.setQueryData<Page | null>(["page", latest.id], current =>
        current?.id === renamed.id && current.version <= renamed.version
          ? { ...current, title: renamed.title, version: renamed.version }
          : current);
      for (const queryKey of [...RENAME_INVALIDATIONS, ["page-revisions", latest.id]]) {
        void queryClient.invalidateQueries({ queryKey });
      }
      finish(renamed.title);
    } catch {
      setPending(false);
      setError(true);
    }
  };

  return (
    <form className="wiki-recent-note-rename" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <PageIcon size={16} />
      <input
        aria-label={t("pageDetail.renamePage")}
        aria-invalid={error || undefined}
        autoFocus
        disabled={pending}
        maxLength={200}
        value={draft}
        onChange={event => { setDraft(event.target.value); setError(false); }}
        onFocus={event => event.currentTarget.select()}
        onBlur={() => {
          // Clicking away keeps a changed title, like a file rename; after a
          // failed rename it leaves the old title.
          if (pending) return;
          if (error) finish(null); else void submit();
        }}
        onKeyDown={event => {
          if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); finish(null); }
        }}
      />
      {error && <p role="alert">{t("pageDetail.renameError")}</p>}
    </form>
  );
}
