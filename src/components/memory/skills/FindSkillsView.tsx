// SPDX-License-Identifier: AGPL-3.0-only
// Find, check and install other people's skills from skills.sh
// (Library › Skills). Only skills whose safety checks all passed are listed;
// ones with warnings appear after the person asks for them; failing ones are
// never shown. The installed version is pinned and the author's text is kept
// apart from the person's own additions.
import { useMemo, useState, type FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { MagnifyingGlass } from "@phosphor-icons/react";
import {
  LOCAL_SKILL_SOURCE,
  importLocalSkills,
  installSkill,
  listInstalledSkills,
  listSpaces,
  previewSkill,
  refreshInstalledSkills,
  removeSkill,
  scanLocalSkills,
  searchSkillRegistry,
  setSkillAdditions,
  type InstalledSkill,
  type SkillInstallResult,
  type SkillPreview,
  type SkillScope,
  type SkillSearchRow,
} from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";
import { isWebBuild } from "../../../web/runtime";
import "./findSkills.css";

export const INSTALLED_SKILLS_KEY = ["installed-skills"] as const;
export const LOCAL_SKILLS_KEY = ["local-skills"] as const;

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Why a skill could not be installed, taken in or removed, in the person's
 *  words. The two refusals Wenlan gives by rule (UX-SKILL-13) are said in
 *  the app's language; any other reason is the one that came back. */
export function refusalText(error: unknown, t: TFunction): string {
  const text = message(error);
  if (text.includes("skills_need_the_person")) return t("findSkills.refusedNotPerson");
  if (text.includes("skills_need_this_computer")) return t("findSkills.refusedBeyondComputer");
  return text;
}

/** Why Wenlan will not install the skill a preview shows, or null: it holds
 *  text that cannot be shown to the person as it is. The daemon and the
 *  desktop app both say so in `cannotInstall` and refuse the install
 *  themselves; an answer without the field (a daemon older than the rule)
 *  blocks nothing here. */
export function cannotInstallReason(preview: Pick<SkillPreview, "files" | "cannotInstall">, t: TFunction): string | null {
  const reason = preview.cannotInstall?.trim();
  if (!reason) return null;
  const cut = preview.files.find((file) => file.cut === true);
  if (cut) return t("findSkills.cannotInstallCut", { file: cut.path });
  const unreadable = /^(.+) is not readable text/u.exec(reason)?.[1];
  if (unreadable) return t("findSkills.cannotInstallUnreadable", { file: unreadable });
  const hidden = /^(.+) holds characters that do not show/u.exec(reason)?.[1];
  if (hidden) return t("findSkills.cannotInstallHidden", { file: hidden });
  return reason;
}

/** One `<U+XXXX>` in a file's text, as the daemon writes out a character
 *  that would not show. */
const WRITTEN_OUT = /(<U\+[0-9A-F]{4,6}>)/u;

/** A file's text with each written-out character set apart, so it reads as
 *  a mark and not as part of the author's words. */
function FileText({ text, marked }: { readonly text: string; readonly marked: boolean }) {
  if (!marked) return <pre>{text}</pre>;
  return (
    <pre>
      {text.split(WRITTEN_OUT).map((part, index) => index % 2 === 1
        ? <mark className="find-skills-mark" key={index}>{part}</mark>
        : part)}
    </pre>
  );
}

function openLink(url: string) {
  void openExternal(url).catch(() => undefined);
}

function Verdict({ verdict }: { readonly verdict: "pass" | "warn" | "fail" }) {
  const { t } = useTranslation();
  return verdict === "pass"
    ? <span className="find-skills-badge find-skills-badge--pass">✓ {t("findSkills.pass")}</span>
    : <span className="find-skills-badge find-skills-badge--warn">{t("findSkills.warn")}</span>;
}

export function visibleRows(rows: readonly SkillSearchRow[], showWarn: boolean): { shown: SkillSearchRow[]; hidden: number } {
  const usable = rows.filter((row) => row.verdict !== "fail");
  const shown = showWarn ? usable : usable.filter((row) => row.verdict === "pass");
  return { shown, hidden: usable.length - shown.length };
}

function SearchPane({ onOpen }: { readonly onOpen: (row: SkillSearchRow) => void }) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");
  const [showWarn, setShowWarn] = useState(false);
  const search = useQuery({
    queryKey: ["skill-registry-search", query],
    queryFn: () => searchSkillRegistry(query),
    enabled: query.trim().length >= 2,
    staleTime: 60_000,
    retry: false,
  });
  const { shown, hidden } = useMemo(() => visibleRows(search.data?.rows ?? [], showWarn), [search.data, showWarn]);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    setQuery(draft.trim());
  };
  return (
    <section className="find-skills-search" aria-label={t("findSkills.searchLabel")}>
      <form className="find-skills-search-row" onSubmit={submit} role="search">
        <label className="find-skills-search-field">
          <MagnifyingGlass aria-hidden="true" size={16} />
          <span className="sr-only">{t("findSkills.searchLabel")}</span>
          <input type="search" value={draft} placeholder={t("findSkills.placeholder")} onChange={(event) => setDraft(event.target.value)} />
        </label>
        <label className="find-skills-toggle">
          <input type="checkbox" checked={showWarn} onChange={(event) => setShowWarn(event.target.checked)} />
          {t("findSkills.showWarn")}
        </label>
      </form>
      {search.isFetching && <p className="find-skills-muted" role="status">{t("findSkills.searching")}</p>}
      {search.isError && <p role="alert">{t("findSkills.searchFailed")}</p>}
      {search.isSuccess && shown.length === 0 && hidden === 0 && <p className="find-skills-muted">{t("findSkills.empty")}</p>}
      {shown.length > 0 && (
        <ul className="find-skills-results">
          {shown.map((row) => (
            <li key={`${row.source}/${row.skillId}`}>
              <button type="button" className="find-skills-row" onClick={() => onOpen(row)}>
                <span className="find-skills-row-main">
                  <span className="find-skills-row-name">{row.name}</span>
                  <span className="find-skills-muted">{row.source}</span>
                </span>
                <span className="find-skills-muted">{t("findSkills.installs", { count: row.installs })}</span>
                <Verdict verdict={row.verdict} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {hidden > 0 && <p className="find-skills-muted">{t("findSkills.hiddenWarn", { count: hidden })}</p>}
    </section>
  );
}

function installedLine(result: SkillInstallResult, t: TFunction): string {
  const lines = [result.tools.length > 0 ? t("findSkills.installedTo", { count: result.tools.length }) : t("findSkills.noTools")];
  for (const path of result.skipped) lines.push(t("findSkills.skipped", { path }));
  return lines.join(" ");
}

function DetailPane({ row, onBack }: { readonly row: SkillSearchRow; readonly onBack: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const preview = useQuery({
    queryKey: ["skill-preview", row.source, row.skillId],
    queryFn: () => previewSkill(row.source, row.skillId),
    retry: false,
  });
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces, staleTime: 30_000 });
  const [where, setWhere] = useState<"all" | "space">("all");
  const [spaceName, setSpaceName] = useState("");
  const [warnOk, setWarnOk] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const install = useMutation({
    mutationFn: (target: SkillPreview) => {
      const scope: SkillScope = where === "space" ? { kind: "space", name: spaceName } : { kind: "all" };
      return installSkill(target, scope);
    },
    onSuccess: (result) => {
      setDone(installedLine(result, t));
      void queryClient.invalidateQueries({ queryKey: INSTALLED_SKILLS_KEY });
    },
  });
  const data = preview.data;
  const warn = data?.safety.verdict === "warn";
  const blocked = data ? cannotInstallReason(data, t) : null;
  const canInstall = !!data && blocked === null && !install.isPending && (where === "all" || spaceName !== "") && (!warn || warnOk);
  return (
    <div className="find-skills-detail">
      <div className="find-skills-detail-main">
        <button type="button" className="find-skills-back" onClick={onBack}>{t("findSkills.back")}</button>
        <h2 className="find-skills-detail-title">{data?.name ?? row.name}</h2>
        {data?.description && <p className="find-skills-muted">{data.description}</p>}
        {preview.isPending && <p className="find-skills-muted" role="status">{t("findSkills.opening")}</p>}
        {preview.isError && <p role="alert">{t("findSkills.previewFailed", { message: message(preview.error) })}</p>}
        {data && (
          <>
            <div className="find-skills-badges">
              {data.safety.verdict === "pass"
                ? <span className="find-skills-badge find-skills-badge--pass">✓ {t("findSkills.passDetail", { count: data.safety.audits.length })}</span>
                : <span className="find-skills-badge find-skills-badge--warn">{data.safety.audits.length === 0 ? t("findSkills.unchecked") : t("findSkills.warnDetail")}</span>}
              {data.runnable.length === 0 && <span className="find-skills-badge">{t("findSkills.noScripts")}</span>}
            </div>
            {data.runnable.length > 0 && <p className="find-skills-note">{t("findSkills.scripts", { files: data.runnable.join(", ") })}</p>}
            {blocked && <p className="find-skills-note" id="find-skills-blocked" role="note">{blocked}</p>}
            {warn && data.safety.audits.length > 0 && (
              <ul className="find-skills-audits">
                {data.safety.audits.filter((audit) => audit.status !== "pass").map((audit) => (
                  <li key={audit.provider}>{audit.provider}: {audit.summary || audit.status}</li>
                ))}
              </ul>
            )}
            <section className="find-skills-files" aria-label={t("findSkills.contents")}>
              <p className="find-skills-muted">{t("findSkills.contents")}</p>
              {data.files.map((file) => (
                <div key={file.path} className="find-skills-file">
                  <div className="find-skills-file-path">{file.path}</div>
                  {(file.marked ?? 0) > 0 && <p className="find-skills-note find-skills-marked">{t("findSkills.marked", { count: file.marked ?? 0 })}</p>}
                  {file.text === null
                    ? <p className="find-skills-muted">{t("findSkills.binary")}</p>
                    : <FileText text={file.text} marked={(file.marked ?? 0) > 0} />}
                  {file.cut === true && <p className="find-skills-note find-skills-cut">{t("findSkills.cut")}</p>}
                </div>
              ))}
            </section>
          </>
        )}
      </div>
      <aside className="find-skills-side" aria-label={t("findSkills.installWhere")}>
        <h3>{t("findSkills.installWhere")}</h3>
        <fieldset className="find-skills-where">
          <legend className="sr-only">{t("findSkills.installWhere")}</legend>
          <label className="find-skills-choice" data-selected={where === "all" ? "true" : undefined}>
            <input type="radio" name="skill-where" checked={where === "all"} onChange={() => setWhere("all")} />
            <span><span>{t("findSkills.allSpaces")}</span><span className="find-skills-muted">{t("findSkills.allSpacesHint")}</span></span>
          </label>
          <label className="find-skills-choice" data-selected={where === "space" ? "true" : undefined}>
            <input type="radio" name="skill-where" checked={where === "space"} onChange={() => setWhere("space")} />
            <span><span>{t("findSkills.oneSpace")}</span><span className="find-skills-muted">{t("findSkills.oneSpaceHint")}</span></span>
          </label>
          {where === "space" && (
            <select aria-label={t("findSkills.pickSpace")} value={spaceName} onChange={(event) => setSpaceName(event.target.value)}>
              <option value="">{t("findSkills.pickSpace")}</option>
              {(spaces.data ?? []).map((space) => <option key={space.id} value={space.name}>{space.name}</option>)}
            </select>
          )}
        </fieldset>
        {warn && (
          <label className="find-skills-toggle">
            <input type="checkbox" checked={warnOk} onChange={(event) => setWarnOk(event.target.checked)} />
            {t("findSkills.warnConfirm")}
          </label>
        )}
        <button type="button" className="find-skills-install" disabled={!canInstall} aria-describedby={blocked ? "find-skills-blocked" : undefined} onClick={() => data && install.mutate(data)}>
          {install.isPending ? t("findSkills.installing") : data?.installedAs ? t("findSkills.reinstall") : t("findSkills.install")}
        </button>
        {blocked && <p className="find-skills-note">{t("findSkills.cannotInstall")}</p>}
        {done && <p role="status">{done}</p>}
        {install.isError && (
          <div className="find-skills-refusal" role="alert">
            <p className="find-skills-refusal-lead">{t("findSkills.installFailedLead")}</p>
            <p>{refusalText(install.error, t)}</p>
          </div>
        )}
        <p className="find-skills-muted">{t("findSkills.pinned")}</p>
        {data && (
          <p className="find-skills-muted">
            {t("findSkills.source")}<a href={data.skillsShUrl} onClick={(event) => { event.preventDefault(); openLink(data.skillsShUrl); }}>skills.sh</a>
            {" · "}
            <a href={data.sourceUrl} onClick={(event) => { event.preventDefault(); openLink(data.sourceUrl); }}>{t("findSkills.original")}</a>
          </p>
        )}
      </aside>
    </div>
  );
}

function InstalledRow({ skill }: { readonly skill: InstalledSkill }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [text, setText] = useState(skill.additions);
  const [saved, setSaved] = useState(false);
  const save = useMutation({
    mutationFn: () => setSkillAdditions(skill.name, text),
    onSuccess: () => { setSaved(true); void queryClient.invalidateQueries({ queryKey: INSTALLED_SKILLS_KEY }); },
  });
  const remove = useMutation({
    mutationFn: () => removeSkill(skill.name),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: INSTALLED_SKILLS_KEY });
      void queryClient.invalidateQueries({ queryKey: LOCAL_SKILLS_KEY });
    },
  });
  const local = skill.source === LOCAL_SKILL_SOURCE;
  const askRemove = async () => {
    const question = local ? t("findSkills.removeLocalConfirm", { name: skill.name }) : t("findSkills.removeConfirm", { name: skill.name });
    const ok = await confirmAction(question, { confirm: t("findSkills.remove"), cancel: t("common.cancel") });
    if (ok) remove.mutate();
  };
  return (
    <li className="find-skills-installed-row">
      <div className="find-skills-installed-head">
        <span className="find-skills-row-name">{skill.name}</span>
        <span className="find-skills-muted">{skill.scope.kind === "space" ? t("findSkills.forSpace", { space: skill.scope.name }) : t("findSkills.forAll")}</span>
        <span className="find-skills-muted">{local ? t("findSkills.fromComputer") : skill.source}</span>
        <button type="button" className="find-skills-link-button" disabled={remove.isPending} onClick={() => void askRemove()}>{t("findSkills.remove")}</button>
      </div>
      {remove.isError && <p role="alert" className="find-skills-note">{t("findSkills.removeFailed", { message: refusalText(remove.error, t) })}</p>}
      {/* The person's additions are changed in the desktop app only: the
          browser has nowhere to save them, so it shows them and offers no box. */}
      {isWebBuild() && skill.additions.trim() !== "" && (
        <div className="find-skills-additions">
          <span>{t("findSkills.additions")}</span>
          <p className="find-skills-muted find-skills-additions-text">{skill.additions}</p>
        </div>
      )}
      {!isWebBuild() && (
        <>
          <label className="find-skills-additions">
            <span>{t("findSkills.additions")}</span>
            <span className="find-skills-muted">{t("findSkills.additionsHint")}</span>
            <textarea value={text} rows={3} onChange={(event) => { setText(event.target.value); setSaved(false); }} />
          </label>
          <div className="find-skills-installed-actions">
            <button type="button" disabled={save.isPending || text === skill.additions} onClick={() => save.mutate()}>{t("findSkills.save")}</button>
            {saved && <span role="status" className="find-skills-muted">{t("findSkills.saved")}</span>}
          </div>
        </>
      )}
    </li>
  );
}

function LocalPane() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const local = useQuery({ queryKey: LOCAL_SKILLS_KEY, queryFn: scanLocalSkills, retry: false });
  const [result, setResult] = useState<{ imported: number; failed: string[] } | null>(null);
  const take = useMutation({
    mutationFn: (names: string[]) => importLocalSkills(names),
    onSuccess: (outcome) => {
      setResult({ imported: outcome.imported.length, failed: outcome.failed.map(([name]) => name) });
      void queryClient.invalidateQueries({ queryKey: LOCAL_SKILLS_KEY });
      void queryClient.invalidateQueries({ queryKey: INSTALLED_SKILLS_KEY });
    },
  });
  const found = local.data ?? [];
  if (found.length === 0 && !result) return null;
  return (
    <section className="find-skills-local" aria-label={t("findSkills.localTitle")}>
      {found.length > 0 && (
        <>
          <h2>{t("findSkills.localTitle")}</h2>
          <p className="find-skills-muted">{t("findSkills.localHint", { count: found.length })}</p>
          <ul className="find-skills-local-names">
            {found.map((skill) => <li key={skill.name} className="find-skills-badge" title={skill.description ?? undefined}>{skill.name}</li>)}
          </ul>
          <button type="button" className="find-skills-install find-skills-local-button" disabled={take.isPending} onClick={() => take.mutate(found.map((skill) => skill.name))}>
            {take.isPending ? t("findSkills.localImporting") : t("findSkills.localImport")}
          </button>
        </>
      )}
      {result && result.imported > 0 && <p role="status" className="find-skills-muted">{t("findSkills.localImported", { count: result.imported })}</p>}
      {result?.failed.map((name) => <p key={name} className="find-skills-note">{t("findSkills.localFailed", { name })}</p>)}
      {take.isError && <p role="alert">{t("findSkills.localImportFailed", { message: refusalText(take.error, t) })}</p>}
    </section>
  );
}

function InstalledPane() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const installed = useQuery({ queryKey: INSTALLED_SKILLS_KEY, queryFn: listInstalledSkills });
  // Author updates that pass every check land when this view opens.
  const refresh = useQuery({
    queryKey: ["installed-skills-refresh"],
    queryFn: async () => {
      const outcomes = await refreshInstalledSkills();
      if (outcomes.some((o) => o.outcome === "updated")) void queryClient.invalidateQueries({ queryKey: INSTALLED_SKILLS_KEY });
      return outcomes;
    },
    enabled: (installed.data?.length ?? 0) > 0,
    staleTime: 10 * 60_000,
    retry: false,
  });
  if (!installed.data || installed.data.length === 0) return null;
  const updated = refresh.data?.filter((o) => o.outcome === "updated").length ?? 0;
  const needsLook = refresh.data?.filter((o) => o.outcome === "needs_look") ?? [];
  return (
    <section className="find-skills-installed" aria-label={t("findSkills.yourSkills")}>
      <h2>{t("findSkills.yourSkills")}</h2>
      {updated > 0 && <p role="status" className="find-skills-muted">{t("findSkills.updated", { count: updated })}</p>}
      {needsLook.map((o) => <p key={o.name} className="find-skills-note">{t("findSkills.needsLook", { name: o.name })}</p>)}
      <ul>{installed.data.map((skill) => <InstalledRow key={`${skill.name}:${skill.updatedAt}`} skill={skill} />)}</ul>
    </section>
  );
}

export function FindSkillsView() {
  const { t } = useTranslation();
  const [open, setOpen] = useState<SkillSearchRow | null>(null);
  return (
    <div className="find-skills">
      <header className="find-skills-header">
        <div>
          <p className="find-skills-muted">{t("findSkills.crumb")}</p>
          <h1>{t("findSkills.title")}</h1>
        </div>
        <p className="find-skills-muted">
          {t("findSkills.fromSite")} <a href="https://skills.sh" onClick={(event) => { event.preventDefault(); openLink("https://skills.sh"); }}>skills.sh</a>
        </p>
      </header>
      {open ? <DetailPane row={open} onBack={() => setOpen(null)} /> : (
        <>
          <SearchPane onOpen={setOpen} />
          <LocalPane />
          <InstalledPane />
        </>
      )}
    </div>
  );
}
