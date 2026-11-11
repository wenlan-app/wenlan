// SPDX-License-Identifier: AGPL-3.0-only
// One box for sharing a Space or one page with people: choose a cloud folder
// and share; once shared, the same box lists the members.
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  lookInSharedFolder,
  revealSharedFolder,
  sharePage,
  shareSpace,
  stopSharingPage,
  stopSharingSpace,
  teamHostSpace,
  type SharedKind,
  type SharedStatus,
} from "../../../lib/tauri";
import { ConfirmDialog } from "../../ConfirmDialog";
import type { SettingsSection } from "../settings/SettingsSidebar";
import { TEAM_KEY, useTeamStatus } from "./team";
import { TeamSpaceAccess } from "./TeamMembers";
import { relativeMs } from "../page/format";
import { SHARES_KEY, cloudName, others, pageShare, revealFolderLabel, shortFolder, spaceShare, teamLabel, useShares } from "./sharing";
import "./sharing.css";

export type ShareTarget = {
  readonly kind: SharedKind;
  /** The Space's name, or the page's id. */
  readonly target: string;
  /** What the box calls it: the Space's name or the page's title. */
  readonly title: string;
};

type ShareDialogProps = ShareTarget & {
  readonly onClose: () => void;
  readonly onOpenSettings?: (section?: SettingsSection) => void;
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function ShareDialog({ kind, target, title, onClose, onOpenSettings }: ShareDialogProps) {
  const { t } = useTranslation();
  const shares = useShares();
  const share = kind === "space" ? spaceShare(shares.data, target) : pageShare(shares.data, target);
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  return (
    <dialog
      ref={dialogRef}
      className="share-dialog"
      aria-labelledby="share-dialog-title"
      onCancel={(event) => { event.preventDefault(); onClose(); }}
    >
      {shares.isPending ? (
        <p role="status">{t("sharing.syncing")}</p>
      ) : share && share.kind === "space" && (share.host || share.hosted) ? (
        <TeamSpaceBox share={share} shares={shares.data ?? []} onClose={onClose} />
      ) : share ? (
        <MembersBox share={share} onClose={onClose} onOpenSettings={onOpenSettings} />
      ) : (
        <ShareBox kind={kind} target={target} title={title} onClose={onClose} />
      )}
    </dialog>
  );
}

function ShareBox({ kind, target, title, onClose }: ShareTarget & { readonly onClose: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [folder, setFolder] = useState<string | null>(null);
  const [taken, setTaken] = useState(false);
  const [pending, setPending] = useState<"team" | "folder" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Only a computer that runs a team server can host a Space for a team; it
  // is asked first how to share, and goes straight on with the answer.
  const teamStatus = useTeamStatus();
  const canHost = kind === "space" && Boolean(teamStatus.data?.listening);
  const [step, setStep] = useState<"choose" | "folder">("choose");
  const choosing = canHost && step === "choose";
  const choose = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setFolder(chosen);
    setError(null);
    setTaken(false);
    try {
      setTaken((await lookInSharedFolder(chosen)) !== null);
    } catch {
      // A folder that cannot be read is reported when sharing.
    }
  };
  const hostForTeam = async () => {
    if (pending) return;
    setPending("team");
    setError(null);
    try {
      await teamHostSpace(target);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
        queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
      ]);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setPending(null);
    }
  };
  const share = async (event: React.FormEvent) => {
    event.preventDefault();
    if (pending || choosing || !folder || taken) return;
    setPending("folder");
    setError(null);
    try {
      if (kind === "space") await shareSpace(target, folder);
      else await sharePage(target, folder);
      await queryClient.invalidateQueries({ queryKey: SHARES_KEY });
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setPending(null);
    }
  };
  const outsideCloud = folder !== null && cloudName(folder, t) === null;
  return (
    <form onSubmit={share}>
      <h2 id="share-dialog-title">{t("sharing.shareTitle", { name: title })}</h2>
      {choosing ? (
        <>
          <ul className="share-ways">
            <li>
              <button type="button" className="share-way" disabled={pending !== null} onClick={() => void hostForTeam()}>
                <span className="share-way-text">
                  <span className="share-way-title">{t("team.shareChoiceTeam")}</span>
                  <span className="share-way-hint">{t(pending === "team" ? "sharing.sharing" : "team.shareTeamHint")}</span>
                </span>
                <span aria-hidden="true" className="share-way-chevron">›</span>
              </button>
            </li>
            <li>
              <button type="button" className="share-way" disabled={pending !== null} onClick={() => { setStep("folder"); setError(null); }}>
                <span className="share-way-text">
                  <span className="share-way-title">{t("team.shareChoiceFolder")}</span>
                  <span className="share-way-hint">{t("team.shareFolderHint")}</span>
                </span>
                <span aria-hidden="true" className="share-way-chevron">›</span>
              </button>
            </li>
          </ul>
          {error && <p className="share-warning" role="alert">{t("sharing.failed", { message: error })}</p>}
          <div className="share-actions">
            <button type="button" className="share-secondary" onClick={onClose} disabled={pending !== null}>{t("sharing.cancel")}</button>
          </div>
        </>
      ) : (
        <>
          {canHost && <button type="button" className="share-link share-back" onClick={() => { setStep("choose"); setError(null); }}>{t("team.shareBack")}</button>}
          <div className="share-field">
            <span className="share-label">{t("sharing.folder")}</span>
            <div className="share-folder-row">
              <span className="share-folder" data-empty={folder === null}>
                <FolderIcon />
                <span>{folder ? shortFolder(folder, t) : t("sharing.noFolder")}</span>
              </span>
              <button type="button" className="share-secondary" onClick={() => void choose()} disabled={pending !== null}>{t("sharing.choose")}</button>
            </div>
            <p className="share-hint">{t(kind === "space" ? "sharing.folderHintSpace" : "sharing.folderHintPage")}</p>
            {taken && <p className="share-warning" role="alert">{t("sharing.alreadyShared")}</p>}
            {!taken && outsideCloud && <p className="share-warning">{t("sharing.notCloud")}</p>}
            {error && <p className="share-warning" role="alert">{t("sharing.failed", { message: error })}</p>}
          </div>
          <div className="share-actions">
            <button type="button" className="share-secondary" onClick={onClose} disabled={pending !== null}>{t("sharing.cancel")}</button>
            <button type="submit" className="share-primary" disabled={!folder || taken || pending !== null}>{t(pending ? "sharing.sharing" : "sharing.share")}</button>
          </div>
        </>
      )}
    </form>
  );
}

function syncLine(share: SharedStatus, t: TFunction, locale: string): string {
  const last = share.last;
  if (!last) return t("sharing.syncing");
  if (last.paused) return share.host ? t("team.pausedRemoved") : t("sharing.paused");
  const when = relativeMs(Date.parse(last.at), locale);
  if (last.counts.skipped > 0) return t("sharing.waiting", { count: last.counts.skipped });
  return t("sharing.upToDate", { when });
}

/** A Space of a team: who can use it. The owner picks everyone or some
 *  people; a member reads the same thing. A member leaves the whole team,
 *  never one Space, so there is no "stop" here for them. */
function TeamSpaceBox({ share, shares, onClose }: {
  readonly share: SharedStatus;
  readonly shares: readonly SharedStatus[];
  readonly onClose: () => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const [confirmingStop, setConfirmingStop] = useState(false);
  const team = teamLabel(t, share.team ?? shares.find((other) => other.hosted && other.team)?.team);
  const me = share.members.find((member) => member.id === share.member_id) ?? { id: share.member_id, name: share.member_name };
  const rest = others(share);
  const broken = share.last?.paused || share.last?.error;
  const stop = async (confirmed: boolean) => {
    setConfirmingStop(false);
    if (!confirmed) return;
    try {
      await stopSharingSpace(share.target);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
        queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
      ]);
      onClose();
    } catch (caught) {
      setError(errorText(caught));
    }
  };
  return (
    <div className="share-members">
      <h2 id="share-dialog-title">{t("team.access.title", { space: share.target, team })}</h2>
      {share.hosted ? (
        <TeamSpaceAccess space={share.target} team={team} onError={setError} />
      ) : (
        <ul className="share-member-list">
          <li><span>{t(share.read_only ? "team.youReadOnly" : "team.access.youCanEdit", { name: me.name })}</span></li>
          {rest.map((member) => <li key={member.id}><span>{member.name}</span></li>)}
        </ul>
      )}
      {broken && <p className="share-warning" role="status">{share.last?.paused ? t("team.pausedRemoved") : share.last?.error}</p>}
      {error && <p className="share-warning" role="alert">{t("sharing.failed", { message: error })}</p>}
      <div className={share.hosted ? "share-actions share-actions-split" : "share-actions"}>
        {share.hosted && <button type="button" className="share-link" onClick={() => setConfirmingStop(true)}>{t("team.access.takeOff")}</button>}
        <button type="button" className="share-primary" onClick={onClose}>{t("sharing.done")}</button>
      </div>
      {confirmingStop && (
        <ConfirmDialog
          message={t("team.access.takeOffConfirm", { space: share.target, team })}
          confirmLabel={t("team.access.takeOff")}
          cancelLabel={t("sharing.cancel")}
          onAnswer={(confirmed) => void stop(confirmed)}
        />
      )}
    </div>
  );
}

function MembersBox({ share, onClose, onOpenSettings }: {
  readonly share: SharedStatus;
  readonly onClose: () => void;
  readonly onOpenSettings?: (section?: SettingsSection) => void;
}) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const queryClient = useQueryClient();
  const [confirmingStop, setConfirmingStop] = useState(false);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cloud = cloudName(share.folder, t);
  const team = Boolean(share.host) || Boolean(share.hosted);
  const me = share.members.find((member) => member.id === share.member_id) ?? { id: share.member_id, name: share.member_name };
  const rest = others(share);
  const change = async () => {
    const chosen = await openDialog({ directory: true, multiple: false, defaultPath: share.folder });
    if (typeof chosen !== "string" || chosen === share.folder) return;
    setChanging(true);
    setError(null);
    try {
      if (share.kind === "space") await shareSpace(share.target, chosen);
      else await sharePage(share.target, chosen);
      await queryClient.invalidateQueries({ queryKey: SHARES_KEY });
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setChanging(false);
    }
  };
  const stop = async (confirmed: boolean) => {
    setConfirmingStop(false);
    if (!confirmed) return;
    try {
      if (share.kind === "space") await stopSharingSpace(share.target);
      else await stopSharingPage(share.target);
      await queryClient.invalidateQueries({ queryKey: SHARES_KEY });
      onClose();
    } catch (caught) {
      setError(errorText(caught));
    }
  };
  return (
    <div className="share-members">
      <h2 id="share-dialog-title">{share.hosted && share.kind === "space" ? t("team.hostedMembersTitle", { space: share.target }) : t("sharing.membersTitle")}</h2>
      {team ? (
        <p className="share-hint">{teamLabel(t, share.team)}</p>
      ) : (
        <>
          <p className="share-hint">{t("sharing.inviteHow", { cloud: cloud ?? t("sharing.yourCloud") })}</p>
          <div>
            <button type="button" className="share-secondary" onClick={() => void revealSharedFolder(share.folder).catch(() => undefined)}>{revealFolderLabel(t)}</button>
          </div>
        </>
      )}
      <ul className="share-member-list">
        <li>
          <span>{share.host && share.read_only ? t("team.youReadOnly", { name: me.name }) : t("sharing.you", { name: me.name })}</span>
          {onOpenSettings && !share.host && <button type="button" className="share-link" onClick={() => { onClose(); onOpenSettings(); }}>{t("sharing.changeInSettings")}</button>}
        </li>
        {!share.hosted && rest.map((member) => <li key={member.id}><span>{member.name}</span></li>)}
        {!share.hosted && rest.length === 0 && <li className="share-muted">{t("sharing.noOthers")}</li>}
      </ul>
      <dl className="share-facts">
        {!team && <div>
          <dt>{t("sharing.folder")}</dt>
          <dd>
            <span className="share-folder-text">{shortFolder(share.folder, t)}</span>
            <button type="button" className="share-link" disabled={changing} onClick={() => void change()}>{t("sharing.change")}</button>
          </dd>
        </div>}
        <div>
          <dt>{t("sharing.sync")}</dt>
          <dd className={share.last?.paused ? "share-warning-text" : undefined}>{changing ? t("sharing.syncing") : syncLine(share, t, locale)}</dd>
        </div>
      </dl>
      {error && <p className="share-warning" role="alert">{t("sharing.failed", { message: error })}</p>}
      <div className="share-actions share-actions-split">
        <button type="button" className="share-danger" onClick={() => setConfirmingStop(true)}>{t("sharing.stop")}</button>
        <button type="button" className="share-primary" onClick={onClose}>{t("sharing.done")}</button>
      </div>
      {confirmingStop && (
        <ConfirmDialog
          message={team
            ? t("team.stopConfirm", { name: share.target })
            : share.kind === "space"
            ? t("sharing.stopConfirmSpace", { name: share.target, cloud: cloud ?? t("sharing.yourCloud") })
            : t("sharing.stopConfirmPage", { cloud: cloud ?? t("sharing.yourCloud") })}
          confirmLabel={t("sharing.stop")}
          cancelLabel={t("sharing.cancel")}
          onAnswer={(confirmed) => void stop(confirmed)}
        />
      )}
    </div>
  );
}

export function FolderIcon() {
  return (
    <svg aria-hidden="true" className="share-folder-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4.2l2 2h8.8A1.5 1.5 0 0 1 21 9.5v8A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5z" />
    </svg>
  );
}
