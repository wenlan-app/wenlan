// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import type { KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { GithubLogo } from "@phosphor-icons/react";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  bindRepoWiki,
  getRepoWiki,
  publishRepoWiki,
  suggestRepoWiki,
  unbindRepoWiki,
  type RepoWikiAgent,
  type RepoWikiBinding,
  type RepoWikiDaily,
  type RepoWikiPublishResult,
  type RepoWikiRunner,
} from "../../../lib/tauri";
import { Input, SegmentedControl, Select, Toggle } from "../settings/primitives";
import { RepoDocsSection } from "./RepoDocsSection";
import { RepoWikiGithub } from "./RepoWikiGithub";
import "./repo-wiki.css";

const AGENT_NAMES: Record<RepoWikiAgent, string> = {
  claude: "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  copilot: "GitHub Copilot",
};
const DEFAULT_DAILY: RepoWikiDaily = { enabled: false, hour: 9, agent: "claude", runs_on: "computer" };
const HOURS = Array.from({ length: 24 }, (_, hour) => hour);

/** Keep the panel inside the window: it scrolls instead of running past the bottom. */
function fitToWindow(panel: HTMLDivElement | null) {
  if (!panel) return;
  const room = window.innerHeight - panel.getBoundingClientRect().top - 16;
  panel.style.maxHeight = `${Math.max(240, room)}px`;
}

/** `github.com/owner/repo` from an origin URL, or null for another host. */
export function githubRepo(remote: string | null | undefined): string | null {
  const match = remote?.trim().match(/github\.com[:/]+([^/\s]+)\/([^/\s]+?)(?:\.git)?\/?$/i);
  return match ? `github.com/${match[1]}/${match[2]}` : null;
}

export function prNumber(url: string | null | undefined): string | null {
  return url?.match(/\/pull\/(\d+)/)?.[1] ?? null;
}

/** The pull request the last publish left open, if it said so. */
function openPr(publish: RepoWikiPublishResult | null | undefined): string | null {
  if (!publish?.pr_url) return null;
  return ["pr_opened", "pr_updated", "no_changes"].includes(publish.status) ? publish.pr_url : null;
}

function repoName(binding: RepoWikiBinding): string {
  const repo = githubRepo(binding.remote_url);
  if (repo) return repo.split("/").pop() ?? repo;
  return binding.repo_root.split(/[\\/]/).filter(Boolean).pop() ?? binding.repo_root;
}

const repoWikiKey = (space: string) => ["repo-wiki", space] as const;
const repoWikiSuggestKey = (space: string) => ["repo-wiki-suggest", space] as const;

/**
 * The Space header's GitHub button. Shown on a Space bound to a repository,
 * or with pages based on a repository's code; it opens a panel below itself
 * to connect the repository, put the pages in as a pull request, and set the
 * daily update.
 */
export function RepoWikiButton({ space }: { readonly space: string }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [publishFailed, setPublishFailed] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  const bindingQuery = useQuery({
    queryKey: repoWikiKey(space),
    queryFn: async () => (await getRepoWiki(space)) ?? null,
    retry: false,
    staleTime: 30_000,
  });
  const binding = bindingQuery.data ?? null;
  const suggestQuery = useQuery({
    queryKey: repoWikiSuggestKey(space),
    queryFn: async () => (await suggestRepoWiki(space)) ?? { candidates: [] },
    enabled: bindingQuery.isSuccess && !binding,
    retry: false,
    staleTime: 60_000,
  });
  const candidates = suggestQuery.data?.candidates ?? [];

  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => document.removeEventListener("pointerdown", closeOutside);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    // The page under the panel can scroll or the window resize while it is open.
    const fit = () => fitToWindow(panelRef.current);
    fit();
    window.addEventListener("resize", fit);
    document.addEventListener("scroll", fit, true);
    return () => {
      window.removeEventListener("resize", fit);
      document.removeEventListener("scroll", fit, true);
    };
  }, [open]);

  if (!binding && candidates.length === 0) return null;

  const pr = binding ? openPr(binding.last_publish) : null;
  const number = prNumber(pr);
  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };
  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    close();
  };

  return (
    <div ref={wrapRef} className="repo-wiki">
      <button
        ref={triggerRef}
        aria-controls={open ? panelId : undefined}
        aria-expanded={open}
        aria-label={binding ? t("repoWiki.settings") : undefined}
        className="repo-wiki-trigger"
        data-state={binding ? "bound" : "unbound"}
        onClick={() => setOpen((value) => !value)}
        type="button"
      >
        {binding && number ? (
          <>
            <span aria-hidden="true" className="repo-wiki-dot" />
            <span>{t("repoWiki.pr", { number })}</span>
          </>
        ) : (
          <>
            <GithubLogo aria-hidden="true" size={16} />
            <span>{binding ? repoName(binding) : t("repoWiki.connect")}</span>
          </>
        )}
      </button>
      {open && (
        <div
          id={panelId}
          ref={panelRef}
          className="mem-popover-surface repo-wiki-panel"
          onKeyDown={handleKeyDown}
          role="dialog"
          aria-label={binding ? t("repoWiki.settings") : t("repoWiki.connectTitle")}
        >
          {binding ? (
            <BoundPanel
              binding={binding}
              publishFailed={publishFailed}
              onChanged={(next) => queryClient.setQueryData(repoWikiKey(space), next)}
              onPublished={() => void queryClient.invalidateQueries({ queryKey: repoWikiKey(space) })}
              onUnbound={() => {
                queryClient.setQueryData(repoWikiKey(space), null);
                void queryClient.invalidateQueries({ queryKey: repoWikiSuggestKey(space) });
                close();
              }}
            />
          ) : (
            <ConnectPanel
              candidates={candidates}
              onCancel={close}
              onConnected={(next, published) => {
                setPublishFailed(!published);
                queryClient.setQueryData(repoWikiKey(space), next);
                void queryClient.invalidateQueries({ queryKey: repoWikiKey(space) });
              }}
              space={space}
            />
          )}
        </div>
      )}
    </div>
  );
}

function DailyFields({
  daily,
  disabled,
  onChange,
}: {
  readonly daily: RepoWikiDaily;
  readonly disabled?: boolean;
  readonly onChange: (daily: RepoWikiDaily) => void;
}) {
  const { t, i18n } = useTranslation();
  const hourFormat = new Intl.DateTimeFormat(i18n.language, { hour: "numeric" });
  const hourLabel = (hour: number) => hourFormat.format(new Date(2026, 0, 1, hour));
  return (
    <div className="repo-wiki-daily">
      <div className="repo-wiki-row">
        <span>{t("repoWiki.daily")}</span>
        <Toggle
          aria-label={t("repoWiki.daily")}
          enabled={daily.enabled}
          onToggle={() => { if (!disabled) onChange({ ...daily, enabled: !daily.enabled }); }}
        />
      </div>
      {daily.enabled && (
        <>
          <div className="repo-wiki-row">
            <span className="repo-wiki-note">{t("repoWiki.github.runsOn")}</span>
            <SegmentedControl<RepoWikiRunner>
              aria-label={t("repoWiki.github.runsOn")}
              onChange={(runsOn) => { if (!disabled) onChange({ ...daily, runs_on: runsOn }); }}
              options={[
                { value: "computer", label: t("repoWiki.github.computer") },
                { value: "github", label: t("repoWiki.github.github") },
              ]}
              value={daily.runs_on ?? "computer"}
            />
          </div>
          <div className="repo-wiki-daily-fields">
            <Select
              aria-label={t("repoWiki.hour")}
              disabled={disabled}
              onChange={(event) => onChange({ ...daily, hour: Number(event.target.value) })}
              size="sm"
              value={daily.hour}
            >
              {HOURS.map((hour) => <option key={hour} value={hour}>{hourLabel(hour)}</option>)}
            </Select>
            <Select
              aria-label={t("repoWiki.tool")}
              disabled={disabled}
              onChange={(event) => onChange({ ...daily, agent: event.target.value as RepoWikiAgent })}
              size="sm"
              value={daily.agent}
            >
              {(Object.keys(AGENT_NAMES) as RepoWikiAgent[]).map((agent) => (
                <option key={agent} value={agent}>{AGENT_NAMES[agent]}</option>
              ))}
            </Select>
          </div>
          <p className="repo-wiki-note">
            {daily.runs_on === "github"
              ? t("repoWiki.github.quota")
              : t("repoWiki.quota", { tool: AGENT_NAMES[daily.agent] })}
          </p>
        </>
      )}
    </div>
  );
}

function ConnectPanel({
  candidates,
  onCancel,
  onConnected,
  space,
}: {
  readonly candidates: readonly { repo_root: string; remote_url?: string | null }[];
  readonly onCancel: () => void;
  readonly onConnected: (binding: RepoWikiBinding, published: boolean) => void;
  readonly space: string;
}) {
  const { t } = useTranslation();
  const folderId = useId();
  const pagesId = useId();
  const [repoRoot, setRepoRoot] = useState(candidates[0]?.repo_root ?? "");
  const [folder, setFolder] = useState("wiki");
  const [daily, setDaily] = useState<RepoWikiDaily>(DEFAULT_DAILY);
  const found = githubRepo(
    candidates.find((candidate) => candidate.repo_root === repoRoot.trim())?.remote_url,
  );
  // Bind, then put the pages in. The panel turns into the connected view only
  // once both ran, so the button keeps saying what it is doing; a publish that
  // fails after the bind still lands there, with the failure shown.
  const connect = useMutation({
    mutationFn: async () => {
      const binding = await bindRepoWiki(space, repoRoot.trim(), folder.trim() || undefined, daily);
      const published = await publishRepoWiki(space).then(() => true, () => false);
      onConnected(binding, published);
    },
  });

  return (
    <form
      className="repo-wiki-form"
      onSubmit={(event) => {
        event.preventDefault();
        if (repoRoot.trim() && !connect.isPending) connect.mutate();
      }}
    >
      <h2 className="repo-wiki-title">{t("repoWiki.connectTitle")}</h2>
      <label className="repo-wiki-label" htmlFor={folderId}>{t("repoWiki.folderLabel")}</label>
      <div className="repo-wiki-folder">
        <Input
          autoFocus
          id={folderId}
          list={candidates.length > 1 ? `${folderId}-list` : undefined}
          mono
          onChange={(event) => setRepoRoot(event.target.value)}
          spellCheck={false}
          value={repoRoot}
        />
        {candidates.length > 1 && (
          <datalist id={`${folderId}-list`}>
            {candidates.map((candidate) => <option key={candidate.repo_root} value={candidate.repo_root} />)}
          </datalist>
        )}
        <button
          className="repo-wiki-secondary"
          onClick={() => {
            void openDialog({ directory: true, multiple: false }).then((picked) => {
              if (typeof picked === "string") setRepoRoot(picked);
            }).catch(() => {});
          }}
          type="button"
        >
          {t("repoWiki.choose")}
        </button>
      </div>
      {found && <p className="repo-wiki-found">{t("repoWiki.found", { repo: found })}</p>}
      <label className="repo-wiki-label" htmlFor={pagesId}>{t("repoWiki.pagesFolder")}</label>
      <Input id={pagesId} mono onChange={(event) => setFolder(event.target.value)} spellCheck={false} value={folder} />
      <DailyFields daily={daily} disabled={connect.isPending} onChange={setDaily} />
      <p className="repo-wiki-note">{t("repoWiki.consent")}</p>
      {connect.isError && (
        <p className="repo-wiki-error" role="alert" title={String(connect.error)}>{t("repoWiki.connectFailed")}</p>
      )}
      <div className="repo-wiki-buttons">
        <button className="repo-wiki-secondary" onClick={onCancel} type="button">{t("repoWiki.cancel")}</button>
        <button className="repo-wiki-primary" disabled={!repoRoot.trim() || connect.isPending} type="submit">
          {connect.isPending ? t("repoWiki.connecting") : t("repoWiki.connectGo")}
        </button>
      </div>
    </form>
  );
}

function BoundPanel({
  binding,
  publishFailed,
  onChanged,
  onPublished,
  onUnbound,
}: {
  readonly binding: RepoWikiBinding;
  /** The publish right after connecting failed. */
  readonly publishFailed: boolean;
  readonly onChanged: (binding: RepoWikiBinding) => void;
  readonly onPublished: () => void;
  readonly onUnbound: () => void;
}) {
  const { t, i18n } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const [linkFailed, setLinkFailed] = useState(false);
  const publish = binding.last_publish ?? null;
  const pr = openPr(publish);
  const number = prNumber(pr);
  const repo = githubRepo(binding.remote_url) ?? binding.repo_root;
  const timeFormat = new Intl.DateTimeFormat(i18n.language, {
    month: "short", day: "numeric", hour: "numeric", minute: "2-digit",
  });

  const update = useMutation({
    mutationFn: () => publishRepoWiki(binding.space),
    onSettled: onPublished,
  });
  const save = useMutation({
    mutationFn: (daily: RepoWikiDaily) => bindRepoWiki(binding.space, binding.repo_root, undefined, daily),
    onSuccess: onChanged,
  });
  const unbind = useMutation({
    mutationFn: () => unbindRepoWiki(binding.space),
    onSuccess: onUnbound,
  });
  const follow = (url: string) => {
    setLinkFailed(false);
    void openExternal(url).catch(() => setLinkFailed(true));
  };
  const daily = save.isPending && save.variables ? save.variables : binding.daily ?? DEFAULT_DAILY;

  return (
    <div className="repo-wiki-form">
      <div className="repo-wiki-repo">
        <GithubLogo aria-hidden="true" size={16} />
        <span>{repo}</span>
        <code>{binding.folder}/</code>
      </div>
      <p className="repo-wiki-status" aria-live="polite">
        {publish
          ? publish.pages === 0 && (publish.doc_fixes ?? 0) > 0
            ? t("repoWiki.docs.lastDocs", {
              count: publish.doc_fixes,
              time: timeFormat.format(new Date(publish.at * 1000)),
            })
            : t(
              // Every page waits for its code: the waiting line below says so.
              publish.status === "nothing_to_publish" && publish.waiting > 0
                ? "repoWiki.last.no_changes"
                : `repoWiki.last.${publish.status}`,
              {
                count: publish.pages,
                time: timeFormat.format(new Date(publish.at * 1000)),
              },
            )
          : t("repoWiki.never")}
      </p>
      {number && pr && (
        <a className="repo-wiki-link" href={pr} onClick={(event) => { event.preventDefault(); follow(pr); }}>
          {t("repoWiki.openPr", { number })}
        </a>
      )}
      {!pr && publish?.status === "pushed" && publish.compare_url && (
        <a
          className="repo-wiki-link"
          href={publish.compare_url}
          onClick={(event) => { event.preventDefault(); follow(publish.compare_url ?? ""); }}
        >
          {t("repoWiki.openCompare")}
        </a>
      )}
      {publish && publish.waiting > 0 && (
        <p className="repo-wiki-note">{t("repoWiki.waiting", { count: publish.waiting })}</p>
      )}
      {publish && publish.left_out > 0 && (
        <p className="repo-wiki-note">{t("repoWiki.leftOut", { count: publish.left_out })}</p>
      )}
      <div className="repo-wiki-buttons repo-wiki-buttons-start">
        <button
          className="repo-wiki-primary"
          disabled={update.isPending}
          onClick={() => update.mutate()}
          type="button"
        >
          {update.isPending ? t("repoWiki.updating") : t("repoWiki.updateNow")}
        </button>
      </div>
      <RepoDocsSection binding={binding} onChanged={onPublished} />
      <DailyFields daily={daily} disabled={save.isPending} onChange={(next) => save.mutate(next)} />
      {daily.enabled && daily.runs_on === "github" && (
        <RepoWikiGithub
          agent={daily.agent}
          formatTime={(date) => timeFormat.format(date)}
          onOpen={follow}
          space={binding.space}
        />
      )}
      {(update.isError || (publishFailed && update.isIdle)) && (
        <p className="repo-wiki-error" role="alert" title={String(update.error)}>{t("repoWiki.updateFailed")}</p>
      )}
      {save.isError && <p className="repo-wiki-error" role="alert">{t("repoWiki.saveFailed")}</p>}
      {linkFailed && <p className="repo-wiki-error" role="alert">{t("repoWiki.linkFailed")}</p>}
      <div className="repo-wiki-footer">
        {confirming ? (
          <>
            <span>{t("repoWiki.disconnectConfirm")}</span>
            <div className="repo-wiki-buttons">
              <button className="repo-wiki-secondary" onClick={() => setConfirming(false)} type="button">
                {t("repoWiki.keep")}
              </button>
              <button
                className="repo-wiki-danger"
                disabled={unbind.isPending}
                onClick={() => unbind.mutate()}
                type="button"
              >
                {t("repoWiki.disconnect")}
              </button>
            </div>
          </>
        ) : (
          <button className="repo-wiki-danger" onClick={() => setConfirming(true)} type="button">
            {t("repoWiki.disconnect")}
          </button>
        )}
      </div>
    </div>
  );
}
