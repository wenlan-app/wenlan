// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation } from "@tanstack/react-query";
import {
  dismissRepoDocFix,
  type RepoDocFix,
  type RepoWikiBinding,
} from "../../../lib/tauri";

/** Docs shown before the rest fold into one line. */
const SHOWN_DOCS = 3;
/** Fixes shown in an opened doc before the rest fold into one line. */
const SHOWN_FIXES = 3;
/** The doc coding agents read first; it gets its own card. */
const AGENTS_FILE = "AGENTS.md";

/** Open fixes grouped by file, in the order they were found. */
function byFile(fixes: readonly RepoDocFix[]): [string, RepoDocFix[]][] {
  const groups = new Map<string, RepoDocFix[]>();
  for (const fix of fixes) {
    groups.set(fix.path, [...(groups.get(fix.path) ?? []), fix]);
  }
  return [...groups.entries()];
}

function codeLabel(code: NonNullable<RepoDocFix["code"]>[number]): string {
  return code.line_start === code.line_end
    ? `${code.path}:${code.line_start}`
    : `${code.path}:${code.line_start}-${code.line_end}`;
}

/**
 * The repository's own docs in the GitHub panel: a card for AGENTS.md, which
 * coding agents read first, then one line per doc an AI tool found a sentence
 * in that the code no longer agrees with. A line opens to show each fix, which
 * the user can drop before it is merged.
 */
export function RepoDocsSection({
  binding,
  onChanged,
}: {
  readonly binding: RepoWikiBinding;
  readonly onChanged: () => void;
}) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);
  const [allFixes, setAllFixes] = useState<ReadonlySet<string>>(new Set());
  const fixes = (binding.doc_fixes ?? []).filter((fix) => fix.state !== "dismissed");
  const agentsFixes = fixes.filter((fix) => fix.path === AGENTS_FILE);
  const docs = byFile(fixes.filter((fix) => fix.path !== AGENTS_FILE));
  const shown = showAll ? docs : docs.slice(0, SHOWN_DOCS);
  const hidden = docs.length - shown.length;
  const check = binding.doc_check ?? null;
  const publish = binding.last_publish ?? null;
  const dateFormat = new Intl.DateTimeFormat(i18n.language, { month: "short", day: "numeric" });

  const dismiss = useMutation({
    mutationFn: (id: string) => dismissRepoDocFix(binding.space, id),
    onSuccess: onChanged,
  });

  const fixList = (path: string, list: readonly RepoDocFix[]) => {
    const shownFixes = allFixes.has(path) ? list : list.slice(0, SHOWN_FIXES);
    const hiddenFixes = list.length - shownFixes.length;
    return (
      <>
        <ul className="repo-docs-fixes">
          {shownFixes.map((fix) => (
            <li className="repo-docs-fix" key={fix.id}>
              <del>{fix.old_text}</del>
              <ins>{fix.new_text}</ins>
              <span className="repo-docs-why">{fix.reason}</span>
              <span className="repo-docs-fix-foot">
                <code>{(fix.code ?? []).map(codeLabel).join(", ")}</code>
                <button
                  className="repo-docs-drop"
                  disabled={dismiss.isPending}
                  onClick={() => dismiss.mutate(fix.id)}
                  type="button"
                >
                  {t("repoWiki.docs.dontFix")}
                </button>
              </span>
            </li>
          ))}
        </ul>
        {hiddenFixes > 0 && (
          <button
            className="repo-docs-more"
            onClick={() => setAllFixes(new Set([...allFixes, path]))}
            type="button"
          >
            {t("repoWiki.docs.moreFixes", { count: hiddenFixes })}
          </button>
        )}
      </>
    );
  };

  const state = (list: readonly RepoDocFix[]) =>
    list.every((fix) => fix.state === "in_pr") ? t("repoWiki.docs.inPr") : t("repoWiki.docs.next");

  return (
    <>
      <section className="repo-docs" aria-labelledby={`${binding.space}-for-ai`}>
        <h3 className="repo-docs-title" id={`${binding.space}-for-ai`}>{t("repoWiki.docs.forAi")}</h3>
        <div className="repo-docs-agents">
          {agentsFixes.length > 0 ? (
            <button
              aria-expanded={open === AGENTS_FILE}
              className="repo-docs-row repo-docs-toggle"
              onClick={() => setOpen(open === AGENTS_FILE ? null : AGENTS_FILE)}
              type="button"
            >
              <span className="repo-docs-name">
                <code>{t("repoWiki.docs.agentsFile")}</code>
                <span>{t("repoWiki.docs.fixes", { count: agentsFixes.length })}</span>
              </span>
              <span className="repo-docs-chip">{state(agentsFixes)}</span>
            </button>
          ) : (
            <div className="repo-docs-row">
              <code>{t("repoWiki.docs.agentsFile")}</code>
              {check && <span className="repo-docs-chip">{t("repoWiki.docs.upToDate")}</span>}
            </div>
          )}
          <p className="repo-wiki-note">{t("repoWiki.docs.agentsWho")}</p>
          <p className="repo-wiki-note">{t("repoWiki.docs.agentsOurs")}</p>
          {open === AGENTS_FILE && fixList(AGENTS_FILE, agentsFixes)}
        </div>
      </section>

      <section className="repo-docs" aria-labelledby={`${binding.space}-repo-docs`}>
        <h3 className="repo-docs-title" id={`${binding.space}-repo-docs`}>{t("repoWiki.docs.repoDocs")}</h3>
        {shown.map(([path, list]) => {
          const expanded = open === path;
          return (
            <div className="repo-docs-doc" key={path}>
              <button
                aria-expanded={expanded}
                className="repo-docs-row repo-docs-toggle"
                onClick={() => setOpen(expanded ? null : path)}
                type="button"
              >
                <span className="repo-docs-name">
                  <code>{path}</code>
                  <span>{t("repoWiki.docs.fixes", { count: list.length })}</span>
                </span>
                <span className="repo-docs-chip">{state(list)}</span>
              </button>
              {expanded && fixList(path, list)}
            </div>
          );
        })}
        {hidden > 0 && (
          <button className="repo-docs-more" onClick={() => setShowAll(true)} type="button">
            {t("repoWiki.docs.more", { count: hidden })}
          </button>
        )}
        <p className="repo-wiki-note">
          {check
            ? t(fixes.length > 0 ? "repoWiki.docs.checkedSome" : "repoWiki.docs.checkedClean", {
              count: check.docs,
              date: dateFormat.format(new Date(check.at * 1000)),
            })
            : t("repoWiki.docs.notChecked")}
        </p>
        {dismiss.isError && <p className="repo-wiki-error" role="alert">{t("repoWiki.docs.dropFailed")}</p>}
      </section>

      {publish && (publish.taken_back ?? 0) > 0 && (
        <p className="repo-docs-callout">{t("repoWiki.docs.takenBack", { count: publish.taken_back })}</p>
      )}
      {publish && (publish.kept ?? 0) > 0 && (
        <p className="repo-docs-callout">{t("repoWiki.docs.kept", { count: publish.kept })}</p>
      )}
    </>
  );
}
