// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowClockwise } from "@phosphor-icons/react";
import {
  listRemoteGrants, revokeRemoteGrant,
  type RemoteGrant, type RemoteGrantPage,
} from "../../lib/tauri";
import { clearAwaitingConnection, useAwaitingConnection } from "../../lib/pairingLink";
import { clientIdentity } from "../../lib/remoteClient";
import { relativeTime } from "../../lib/relativeTime";
import { Button } from "./settings/primitives";
import { Disclosure, InlineConfirm, RemoteErrorMessage, secondaryText } from "./remoteAccessParts";
import { scopeLabel } from "./remoteScope";
import { REMOTE_GRANTS } from "./useRemoteAccess";

/** The list is quiet: a slow refresh, and a fast one only while a new app is expected. */
export const GRANTS_IDLE_REFRESH_MS = 60_000;
export const GRANTS_WAITING_REFRESH_MS = 5_000;

const END_REASON = {
  revoked: "remoteAccess.endReason.revoked",
  reset: "remoteAccess.endReason.reset",
  expired: "remoteAccess.endReason.expired",
  replaced: "remoteAccess.endReason.replaced",
} as const;

function appName(grant: RemoteGrant, fallback: string): string {
  const identity = clientIdentity({ knownClient: grant.knownClient, redirectHost: grant.redirectHost });
  return identity.kind === "known" ? identity.name : identity.host ?? fallback;
}

/**
 * The AI apps that can reach this library, in plain words: who, when it last
 * asked, when it ends. Client IDs stay behind "Details".
 */
export function ConnectedApps({ revision, enabled }: { revision: string; enabled: boolean }) {
  const { t, i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language;
  const cache = useQueryClient();
  const waitingSince = useAwaitingConnection();
  const [cursor, setCursor] = useState<string | null>(null);
  const [removing, setRemoving] = useState<RemoteGrant | null>(null);
  const listKey = [...REMOTE_GRANTS, revision, cursor];

  const grants = useQuery({
    queryKey: listKey,
    queryFn: () => listRemoteGrants(revision, cursor),
    enabled,
    retry: false,
    // Pause after an error until a manual refresh or a return to the window.
    refetchInterval: (query) =>
      query.state.status === "error" || !enabled ? false
        : waitingSince !== null ? GRANTS_WAITING_REFRESH_MS : GRANTS_IDLE_REFRESH_MS,
    refetchIntervalInBackground: false,
    refetchOnWindowFocus: false,
  });

  // Coming back to the window (from another app, or from the tray) refreshes once.
  const refetch = grants.refetch;
  const fetching = grants.isFetching;
  useEffect(() => {
    if (!enabled) return;
    const onReturn = () => {
      if (document.visibilityState === "hidden" || fetching) return;
      void refetch();
    };
    window.addEventListener("focus", onReturn);
    document.addEventListener("visibilitychange", onReturn);
    return () => {
      window.removeEventListener("focus", onReturn);
      document.removeEventListener("visibilitychange", onReturn);
    };
  }, [enabled, refetch, fetching]);

  // The app that was just allowed has shown up: stop looking so fast.
  useEffect(() => {
    if (waitingSince === null || !grants.data) return;
    // The relay's clock may differ a little from this computer's.
    if (grants.data.items.some((item) => item.status === "active" && item.createdAt >= waitingSince - 60_000)) {
      clearAwaitingConnection();
    }
  }, [waitingSince, grants.data]);

  const remove = useMutation({
    mutationFn: async (grant: RemoteGrant) => {
      const result = await revokeRemoteGrant(revision, grant.id);
      cache.setQueryData<RemoteGrantPage>(listKey, (page) => page && ({
        ...page,
        items: page.items.map((item) => item.id === grant.id
          ? { ...item, status: "inactive" as const, cleanupPending: result.cleanupPending, endReason: "revoked" as const }
          : item),
      }));
      setRemoving(null);
    },
  });

  return (
    <div className="min-w-0 space-y-3">
      <div className="flex items-center justify-between gap-2">
        <h4 className="text-sm font-semibold">{t("remoteAccess.connectedApps")}</h4>
        <Button variant="ghost" size="sm" disabled={grants.isFetching || remove.isPending}
          aria-label={t("remoteAccess.refreshGrants")} title={t("remoteAccess.refreshGrants")}
          onClick={() => { void grants.refetch(); }}>
          <ArrowClockwise size={16} aria-hidden="true" />
        </Button>
      </div>
      {grants.isPending && enabled && <p className={secondaryText}>{t("remoteAccess.loadingGrants")}</p>}
      {grants.error && <RemoteErrorMessage error={grants.error} />}
      {grants.data?.items.length === 0 && <p className={secondaryText}>{t("remoteAccess.noGrants")}</p>}
      <ul className="space-y-5">
        {grants.data?.items.map((grant) => (
          <GrantRow key={grant.id} grant={grant} language={language}
            busy={remove.isPending} confirming={removing?.id === grant.id}
            onAskRemove={() => { remove.reset(); setRemoving(grant); }}
            onCancelRemove={() => setRemoving(null)}
            onRemove={() => remove.mutate(grant)}
            onRetryCleanup={() => remove.mutate(grant)} />
        ))}
      </ul>
      {remove.error && <RemoteErrorMessage error={remove.error} />}
      <div className="flex gap-2">
        {cursor && <Button variant="secondary" size="sm" onClick={() => setCursor(null)}>{t("remoteAccess.firstPage")}</Button>}
        {grants.data?.cursor && <Button variant="secondary" size="sm" onClick={() => setCursor(grants.data!.cursor)}>{t("remoteAccess.nextPage")}</Button>}
      </div>
    </div>
  );
}

function GrantRow({ grant, language, busy, confirming, onAskRemove, onCancelRemove, onRemove, onRetryCleanup }: {
  grant: RemoteGrant;
  language: string;
  busy: boolean;
  confirming: boolean;
  onAskRemove: () => void;
  onCancelRemove: () => void;
  onRemove: () => void;
  onRetryCleanup: () => void;
}) {
  const { t } = useTranslation();
  const name = appName(grant, t("remoteAccess.unrecognizedApp"));
  const active = grant.status === "active";
  const reasonKey = grant.endReason && grant.endReason !== "other" ? END_REASON[grant.endReason] : null;
  const lastUsed = grant.lastUsedAt === undefined ? null
    : grant.lastUsedAt === null ? t("remoteAccess.notUsed")
      : t("remoteAccess.lastUsed", { when: relativeTime(grant.lastUsedAt / 1000, t, language) });
  const ends = Number.isFinite(grant.expiresAt)
    ? t("remoteAccess.ends", { date: new Date(grant.expiresAt).toLocaleDateString(language) }) : null;
  return (
    <li className="min-w-0 space-y-2">
      <div className="flex flex-wrap items-start gap-3">
        <div className="flex-1 min-w-0 basis-40 space-y-1">
          <p className="text-sm font-medium break-words">{name}</p>
          {active
            ? <p className={secondaryText + " break-words"}>{[lastUsed, ends].filter(Boolean).join(" · ")}</p>
            : <p className={secondaryText + " break-words"}>{reasonKey ? t(reasonKey) : t("remoteAccess.ended")}</p>}
          {grant.cleanupPending && <p className={secondaryText}>{t("remoteAccess.cleanupPending")}</p>}
        </div>
        {active && !confirming && <Button variant="ghost" size="sm" className="-mr-[10px]" disabled={busy} onClick={onAskRemove}>{t("remoteAccess.remove")}</Button>}
        {!active && grant.cleanupPending && <Button variant="secondary" size="sm" disabled={busy} onClick={onRetryCleanup}>{t("remoteAccess.retry")}</Button>}
      </div>
      {confirming && (
        <InlineConfirm message={t("remoteAccess.removeConfirm", { name })} confirmLabel={t("remoteAccess.remove")}
          busy={busy} onConfirm={onRemove} onCancel={onCancelRemove} />
      )}
      <Disclosure label={t("remoteAccess.details")}>
        <dl className="text-xs space-y-1 break-all text-[var(--mem-text-secondary)]">
          <div><dt className="inline">{t("remoteAccess.clientId")}: </dt><dd className="inline font-mono">{grant.clientId}</dd></div>
          <div><dd className="break-words">{t("remoteAccess.sharing", { space: scopeLabel(t, grant.space) })}</dd></div>
        </dl>
      </Disclosure>
    </li>
  );
}
