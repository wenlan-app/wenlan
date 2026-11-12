// SPDX-License-Identifier: AGPL-3.0-only
// Joining a Space or a page someone shared: choose the folder they shared,
// see what it holds, and add it.
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  createSpace,
  joinTeamSpace,
  listSpaces,
  lookInSharedFolder,
  sharePage,
  shareSpace,
  teamJoin,
  type SharedFolder,
  type SharedKind,
  type TeamJoinResponse,
} from "../../../lib/tauri";
import { FolderIcon } from "./ShareDialog";
import { SHARES_KEY, shortFolder, teamLabel, useShares } from "./sharing";
import { hostName } from "./team";
import "./sharing.css";

type JoinSharedDialogProps = {
  readonly onClose: () => void;
  /** Called once it is added: the Space's name here, or the page's id; for
   *  a team joined from an invite link, "team" and the team's id. */
  readonly onJoined?: (kind: SharedKind | "team", target: string) => void;
  /** Only one way in: a folder someone shared (from Spaces), or a team's
   *  invite link (from the switch or Settings › Team). Teams stay out of
   *  sight of someone who never joined one. */
  readonly only?: "folder" | "code";
};

function namesOf(folder: SharedFolder): string {
  return folder.members.map((member) => member.name).join(", ");
}

export function JoinSharedDialog({ onClose, onJoined, only }: JoinSharedDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const shares = useShares();
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [mode, setMode] = useState<"folder" | "code">(only ?? "code");
  const [code, setCode] = useState("");
  const [checked, setChecked] = useState<{ code: string; response: TeamJoinResponse } | null>(null);
  const [checking, setChecking] = useState(false);
  const [folder, setFolder] = useState<string | null>(null);
  const [found, setFound] = useState<SharedFolder | null | undefined>(undefined);
  const [isLibrary, setIsLibrary] = useState(false);
  const [localName, setLocalName] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);

  const choose = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setFolder(chosen);
    setError(null);
    setFound(undefined);
    setIsLibrary(false);
    try {
      const shared = await lookInSharedFolder(chosen);
      // A library is joined from Settings, not here.
      setIsLibrary(shared?.kind === "library");
      setFound(shared?.kind === "library" ? null : shared);
      if (shared) setLocalName(shared.name);
    } catch (caught) {
      setFound(null);
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  };

  const checkCode = async (value: string) => {
    const trimmed = value.trim();
    if (!trimmed || checking || checked?.code === trimmed) return;
    setChecking(true);
    setError(null);
    setChecked(null);
    try {
      const response = await teamJoin(trimmed);
      setChecked({ code: trimmed, response });
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setChecking(false);
    }
  };

  const spaceNames = new Set((spaces.data ?? []).map((space) => space.name.trim().toLocaleLowerCase()));
  const nameTaken = found?.kind === "space" && spaceNames.has((found?.name ?? "").trim().toLocaleLowerCase());
  const chosenName = nameTaken ? localName.trim() : found?.name ?? "";
  const stillTaken = nameTaken && spaceNames.has(chosenName.toLocaleLowerCase());
  const takenHere = (serverSpace: string) => spaceNames.has(serverSpace.trim().toLocaleLowerCase());
  const refreshAfterJoin = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
    queryClient.invalidateQueries({ queryKey: ["spaces"] }),
    queryClient.invalidateQueries({ queryKey: ["pages"] }),
    queryClient.invalidateQueries({ queryKey: ["recent-pages"] }),
  ]);
  // A Space here with the same name may hold this person's own pages: the
  // team's copy goes beside it as "<name> (<team>)", never into it.
  const teamLocalName = (serverSpace: string) => (takenHere(serverSpace) && checked
    ? `${serverSpace} (${checked.response.me.team?.name?.trim() || hostName(checked.response.url)})`
    : serverSpace);
  const joiningTeam = teamLabel(t, checked?.response.me.team);
  const joinAll = async () => {
    if (!checked || pending) return;
    setPending(true);
    setError(null);
    const joined: string[] = [];
    try {
      for (const entry of checked.response.me.spaces) {
        const local = teamLocalName(entry.space);
        await joinTeamSpace(local, checked.code, entry.space);
        joined.push(local);
      }
      await refreshAfterJoin();
      onClose();
      onJoined?.("team", checked.response.me.team?.id ?? checked.response.url);
    } catch (caught) {
      if (joined.length > 0) await refreshAfterJoin();
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setPending(false);
    }
  };
  const already = mode === "folder" && found && (shares.data ?? []).some((share) => share.folder === folder
    || (found.kind === "page" && share.kind === "page" && share.target === found.page_id));

  const join = async (event: React.FormEvent) => {
    event.preventDefault();
    if (mode === "code") return;
    if (!folder || !found || already || pending || (found.kind === "space" && (!chosenName || stillTaken))) return;
    setPending(true);
    setError(null);
    try {
      if (found.kind === "space") {
        await createSpace(chosenName);
        await shareSpace(chosenName, folder);
      } else if (found.page_id) {
        await sharePage(found.page_id, folder);
      }
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
        queryClient.invalidateQueries({ queryKey: ["spaces"] }),
        queryClient.invalidateQueries({ queryKey: ["pages"] }),
        queryClient.invalidateQueries({ queryKey: ["recent-pages"] }),
      ]);
      onClose();
      onJoined?.(found.kind === "space" ? "space" : "page", found.kind === "space" ? chosenName : found.page_id ?? "");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setPending(false);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="share-dialog"
      aria-labelledby="join-dialog-title"
      onCancel={(event) => { event.preventDefault(); if (!pending) onClose(); }}
    >
      <form onSubmit={join}>
        <h2 id="join-dialog-title">{mode === "code" && checked ? t("team.join.title", { team: joiningTeam }) : only === "code" ? t("team.join.titleLink") : t("sharing.joinTitle")}</h2>
        {mode === "folder" && !only && (
          <button type="button" className="share-link share-back" onClick={() => { setMode("code"); setError(null); }}>{t("team.joinUseCode")}</button>
        )}
        {mode === "folder" && (
          <>
        <div className="share-field">
          <span className="share-label">{t("sharing.folder")}</span>
          <div className="share-folder-row">
            <span className="share-folder" data-empty={folder === null}>
              <FolderIcon />
              <span>{folder ? shortFolder(folder, t) : t("sharing.noFolder")}</span>
            </span>
            <button type="button" className="share-secondary" onClick={() => void choose()} disabled={pending}>{t("sharing.choose")}</button>
          </div>
          <p className="share-hint">{t("sharing.joinFolderHint")}</p>
          {folder === null && <p className="share-hint">{t("sharing.joinFindHint")}</p>}
        </div>
        {folder !== null && found === null && !error && <p className="share-warning">{t(isLibrary ? "sharing.joinIsLibrary" : "sharing.joinNotShared")}</p>}
        {found && (
          <div className="share-found">
            <strong>{found.name}</strong>
            <span>{t(found.kind === "space" ? "sharing.joinKindSpace" : "sharing.joinKindPage", { names: namesOf(found) })}</span>
          </div>
        )}
        {found?.kind === "page" && !already && <p className="share-hint">{t("sharing.joinPageLands")}</p>}
        {nameTaken && !already && (
          <label className="share-field">
            <span className="share-label">{t("sharing.joinNameTaken")}</span>
            <input className="share-input" value={localName} onChange={(event) => setLocalName(event.target.value)} disabled={pending} />
          </label>
        )}
        {already && <p className="share-warning">{t("sharing.joinAlready")}</p>}
          </>
        )}
        {mode === "code" && !checked && (
          <>
            <div className="share-field">
              <label className="share-label" htmlFor="join-team-code">{t("team.join.linkLabel")}</label>
              <input
                id="join-team-code"
                className="share-input"
                value={code}
                placeholder={t("team.join.linkPlaceholder")}
                spellCheck={false}
                disabled={pending || checking}
                onChange={(event) => {
                  setCode(event.target.value);
                  // A whole link typed or dropped in is checked at once, as a paste is.
                  if (/wlt_[0-9a-f]{64}/.test(event.target.value)) void checkCode(event.target.value);
                }}
                onKeyDown={(event) => { if (event.key === "Enter") void checkCode(code); }}
                onPaste={(event) => { const pasted = event.clipboardData.getData("text"); setCode(pasted); void checkCode(pasted); event.preventDefault(); }}
                onBlur={(event) => void checkCode(event.target.value)}
              />
              {checking && <p className="share-hint">{t("team.joinChecking")}</p>}
            </div>
            {!only && (
              <p className="share-hint">
                {t("team.joinFolderAsk")}{" "}
                <button type="button" className="share-link" onClick={() => { setMode("folder"); setError(null); }}>{t("sharing.chooseFolder")}</button>
              </p>
            )}
          </>
        )}
        {mode === "code" && checked && (
          <div className="share-field team-join-confirm">
            <p className="team-join-as">{checked.response.me.spaces.length > 0
              ? t("team.join.asGets", { name: checked.response.me.name })
              : t("team.join.asNothing", { name: checked.response.me.name })}</p>
            {checked.response.me.spaces.length > 0 && (
              <ul className="share-rows team-join-list">
                {checked.response.me.spaces.map((entry) => (
                  <li key={entry.space}>
                    <span className="share-row-name">{entry.access === "read" ? t("team.joinAccessFor", { space: teamLocalName(entry.space), access: t("team.readOnly") }) : teamLocalName(entry.space)}</span>
                  </li>
                ))}
              </ul>
            )}
            <p className="share-hint">{t("team.join.privacy")}</p>
          </div>
        )}
        {error && <p className="share-warning" role="alert">{t("sharing.joinFailed", { message: error })}</p>}
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose} disabled={pending}>{t("sharing.cancel")}</button>
          {mode === "code" && checked && (
            <button type="button" className="share-primary" disabled={pending} onClick={() => void joinAll()}>
              {pending ? t("sharing.joining") : t("team.join.action", { team: joiningTeam })}
            </button>
          )}
          {mode === "folder" && (
            <button type="submit" className="share-primary" disabled={!found || Boolean(already) || pending || (found.kind === "space" && (!chosenName || stillTaken))}>
              {t(pending ? "sharing.joining" : "sharing.join")}
            </button>
          )}
        </div>
      </form>
    </dialog>
  );
}
