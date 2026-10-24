// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  getRepoWikiGithub,
  setupRepoWikiGithub,
  type RepoWikiAgent,
  type RepoWikiGithubRequest,
  type RepoWikiGithubStatus,
} from "../../../lib/tauri";
import { Input } from "../settings/primitives";

const githubKey = (space: string) => ["repo-wiki-github", space] as const;

function Step({
  done,
  label,
  action,
}: {
  readonly done: boolean;
  readonly label: string;
  readonly action?: React.ReactNode;
}) {
  return (
    <div className="repo-wiki-step" data-done={done || undefined}>
      <span aria-hidden="true" className="repo-wiki-mark" />
      <span className="repo-wiki-step-label">{label}</span>
      {!done && action}
    </div>
  );
}

/**
 * The daily update on GitHub: three one-time steps (the workflow goes in with
 * the next pull request, a secret for the AI tool, Actions allowed to open
 * pull requests), then how the last run went.
 */
export function RepoWikiGithub({
  space,
  agent,
  formatTime,
  onOpen,
}: {
  readonly space: string;
  readonly agent: RepoWikiAgent;
  readonly formatTime: (date: Date) => string;
  readonly onOpen: (url: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [sheet, setSheet] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const status = useQuery({
    queryKey: githubKey(space),
    queryFn: () => getRepoWikiGithub(space),
    retry: false,
    staleTime: 30_000,
  });
  const setup = useMutation({
    mutationFn: (request: RepoWikiGithubRequest) => setupRepoWikiGithub(space, request),
    onSuccess: (next: RepoWikiGithubStatus) => {
      queryClient.setQueryData(githubKey(space), next);
      setSheet(false);
      setApiKey("");
    },
  });
  // Claude Code signs in through the browser; every other tool takes a key.
  const keyAgent = agent === "claude" ? null : agent;
  const pastesKey = keyAgent !== null;
  const current = status.data;
  if (!current) return null;
  if (!current.repo) return <p className="repo-wiki-note">{t("repoWiki.github.notGithub")}</p>;

  const run = current.last_run;
  const pr = run?.pr_url?.match(/\/pull\/(\d+)/)?.[1];
  const link = (url: string, label: string) => (
    <a className="repo-wiki-link" href={url} onClick={(event) => { event.preventDefault(); onOpen(url); }}>
      {label}
    </a>
  );

  return (
    <div className="repo-wiki-github">
      {!current.gh_ready && <p className="repo-wiki-note">{t("repoWiki.github.needsGh")}</p>}
      <div className="repo-wiki-steps">
        <Step done={current.workflow === "in_repo"} label={t("repoWiki.github.workflow")} />
        <Step
          done={current.secret === true}
          label={t("repoWiki.github.secret")}
          action={
            <button
              className="repo-wiki-primary"
              disabled={!current.gh_ready || sheet}
              onClick={() => setSheet(true)}
              type="button"
            >
              {t("repoWiki.github.setUp")}
            </button>
          }
        />
        <Step
          done={current.can_open_prs === true}
          label={t("repoWiki.github.allowPrs")}
          action={
            <button
              className="repo-wiki-secondary"
              disabled={!current.gh_ready || setup.isPending}
              onClick={() => setup.mutate({ action: "allow_pull_requests" })}
              type="button"
            >
              {t("repoWiki.github.allow")}
            </button>
          }
        />
      </div>
      {sheet && (
        <form
          className="repo-wiki-sheet"
          onSubmit={(event) => {
            event.preventDefault();
            if (setup.isPending) return;
            setup.mutate(pastesKey ? { action: "api_key", key: apiKey } : { action: "claude_token" });
          }}
        >
          <span className="repo-wiki-sheet-title">
            {t(keyAgent ? `repoWiki.github.keyTitle.${keyAgent}` : "repoWiki.github.claudeTitle")}
          </span>
          <p className="repo-wiki-note">
            {t(keyAgent ? `repoWiki.github.keyBody.${keyAgent}` : "repoWiki.github.claudeBody")}
          </p>
          {pastesKey && (
            <Input
              aria-label={keyAgent ? t(`repoWiki.github.keyTitle.${keyAgent}`) : undefined}
              autoComplete="off"
              mono
              onChange={(event) => setApiKey(event.target.value)}
              spellCheck={false}
              type="password"
              value={apiKey}
            />
          )}
          <p className="repo-wiki-note">{t("repoWiki.github.safe")}</p>
          <div className="repo-wiki-buttons">
            <button
              className="repo-wiki-secondary"
              disabled={setup.isPending}
              onClick={() => setSheet(false)}
              type="button"
            >
              {t("repoWiki.cancel")}
            </button>
            <button
              className="repo-wiki-primary"
              disabled={setup.isPending || (pastesKey && !apiKey.trim())}
              type="submit"
            >
              {pastesKey
                ? t("repoWiki.github.save")
                : setup.isPending
                  ? t("repoWiki.github.confirming")
                  : t("repoWiki.github.openBrowser")}
            </button>
          </div>
        </form>
      )}
      {setup.isError && (
        <p className="repo-wiki-error" role="alert" title={String(setup.error)}>{t("repoWiki.github.failed")}</p>
      )}
      {run && (run.conclusion === "failure" ? (
        <p className="repo-wiki-note repo-wiki-run">
          <span className="repo-wiki-run-failed">{t("repoWiki.github.lastFailed")}</span>
          {link(run.url, t("repoWiki.github.view"))}
        </p>
      ) : (
        <p className="repo-wiki-note repo-wiki-run">
          <span>{t("repoWiki.github.last", { time: formatTime(new Date(run.at)) })}</span>
          {pr && run.pr_url
            ? link(run.pr_url, t("repoWiki.pr", { number: pr }))
            : link(run.url, t("repoWiki.github.view"))}
        </p>
      ))}
    </div>
  );
}
