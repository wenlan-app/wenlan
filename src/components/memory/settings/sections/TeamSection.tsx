// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team: this computer as a team server. Which Spaces it shares with
// the team, who is on it, and each person's join code (shown once).
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import {
  listSpaces,
  teamAddMember,
  teamHostSpace,
  teamNewMemberKey,
  teamRemoveMember,
  teamStatus,
  teamUpdateMember,
  type TeamAccess,
  type TeamMember,
  type TeamMemberKey,
  type TeamSpaceAccess,
} from "../../../../lib/tauri";
import { confirmAction } from "../../../../lib/confirmAction";
import { SHARES_KEY } from "../../sharing/sharing";
import { Button, SettingRow } from "../primitives";
import { SettingsGroup } from "../SettingsView";
import { TEAM_DOCS_URL, activeMembers, prefillTeamUrl, rememberTeamUrl } from "./teamHelpers";
import "../../sharing/sharing.css";
import "./teamSection.css";

export const TEAM_KEY = ["team"] as const;

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function Modal({ titleId, title, onClose, children }: { titleId: string; title: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current;
    if (dialog && !dialog.open) {
      if (typeof dialog.showModal === "function") dialog.showModal();
      else dialog.setAttribute("open", "");
    }
    return () => dialog?.close?.();
  }, []);
  return (
    <dialog ref={ref} className="share-dialog" aria-labelledby={titleId} onCancel={(event) => { event.preventDefault(); onClose(); }}>
      <h2 id={titleId}>{title}</h2>
      {children}
    </dialog>
  );
}

/** The one-time join code of a person: a read-only field and Copy. */
function CodeDialog({ name, code, onClose }: { name: string; code: string | null | undefined; onClose: () => void }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code ?? "");
      setCopied(true);
    } catch {
      // The field stays selectable to copy by hand.
    }
  };
  return (
    <Modal titleId="team-code-title" title={t("team.codeTitle", { name })} onClose={onClose}>
      <div className="share-members">
        {code ? (
          <div className="team-code-row">
            <input className="share-input team-code" readOnly value={code} aria-label={t("team.codeField")} onFocus={(event) => event.currentTarget.select()} />
            <button type="button" className="share-secondary" onClick={() => void copy()}>{t(copied ? "team.copied" : "team.copy")}</button>
          </div>
        ) : (
          <p className="share-warning">{t("team.codeMissing")}</p>
        )}
        <div className="share-actions">
          <button type="button" className="share-primary" onClick={onClose}>{t("sharing.done")}</button>
        </div>
      </div>
    </Modal>
  );
}

type Access = TeamAccess | "none";

/** Add a person, or change a person's name and Spaces. */
function MemberDialog({ member, hosted, listening, onClose, onSaved }: {
  member: TeamMember | null;
  hosted: readonly string[];
  listening: string | null | undefined;
  onClose: () => void;
  onSaved: (key: TeamMemberKey | null) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(member?.name ?? "");
  const [url, setUrl] = useState(() => prefillTeamUrl(listening));
  const [access, setAccess] = useState<Record<string, Access>>(() => {
    const own = new Map(member?.spaces.map((entry) => [entry.space, entry.access]));
    return Object.fromEntries(hosted.map((space) => [space, own.get(space) ?? (member ? "none" : hosted.length === 1 ? "write" : "none")]));
  });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const chosen: TeamSpaceAccess[] = hosted.flatMap((space) => (access[space] && access[space] !== "none" ? [{ space, access: access[space] as TeamAccess }] : []));
  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!name.trim() || pending) return;
    setPending(true);
    setError(null);
    try {
      if (member) {
        // Spaces the server no longer hosts stay as they are.
        const kept = member.spaces.filter((entry) => !hosted.includes(entry.space));
        await teamUpdateMember(member.id, { name: name.trim(), spaces: [...kept, ...chosen] });
        onSaved(null);
      } else {
        const address = url.trim();
        const key = await teamAddMember(name.trim(), chosen, address || undefined);
        if (address) rememberTeamUrl(address);
        onSaved(key);
      }
    } catch (caught) {
      setError(errorText(caught));
      setPending(false);
    }
  };
  return (
    <Modal titleId="team-member-title" title={member ? t("team.editTitle", { name: member.name }) : t("team.addTitle")} onClose={onClose}>
      <form onSubmit={save}>
        <label className="share-field">
          <span className="share-label">{t("team.name")}</span>
          <input className="share-input" value={name} onChange={(event) => setName(event.target.value)} disabled={pending} autoFocus />
        </label>
        <div className="share-field">
          <span className="share-label">{t("team.spacesLabel")}</span>
          {hosted.length === 0 ? <p className="share-hint">{t("team.noHosted")}</p> : (
            <ul className="team-space-pick">
              {hosted.map((space) => (
                <li key={space}>
                  <label>
                    <input
                      type="checkbox"
                      checked={access[space] !== "none"}
                      disabled={pending}
                      onChange={(event) => setAccess({ ...access, [space]: event.target.checked ? "write" : "none" })}
                    />
                    <span>{space}</span>
                  </label>
                  <select
                    className="share-input"
                    aria-label={t("team.accessFor", { space })}
                    value={access[space] === "read" ? "read" : "write"}
                    disabled={pending || access[space] === "none"}
                    onChange={(event) => setAccess({ ...access, [space]: event.target.value as TeamAccess })}
                  >
                    <option value="write">{t("team.canEdit")}</option>
                    <option value="read">{t("team.readOnly")}</option>
                  </select>
                </li>
              ))}
            </ul>
          )}
        </div>
        {!member && (
          <label className="share-field">
            <span className="share-label">{t("team.urlLabel")}</span>
            <input className="share-input" value={url} onChange={(event) => setUrl(event.target.value)} disabled={pending} placeholder={t("team.urlPlaceholder")} spellCheck={false} />
          </label>
        )}
        {error && <p className="share-warning" role="alert">{error}</p>}
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose} disabled={pending}>{t("sharing.cancel")}</button>
          <button type="submit" className="share-primary" disabled={!name.trim() || pending}>{t(member ? "team.save" : "team.add")}</button>
        </div>
      </form>
    </Modal>
  );
}

function HostDialog({ choices, onClose, onHosted }: { choices: readonly string[]; onClose: () => void; onHosted: () => void }) {
  const { t } = useTranslation();
  const [space, setSpace] = useState(choices[0] ?? "");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const host = async (event: FormEvent) => {
    event.preventDefault();
    if (!space || pending) return;
    setPending(true);
    setError(null);
    try {
      await teamHostSpace(space);
      onHosted();
    } catch (caught) {
      setError(errorText(caught));
      setPending(false);
    }
  };
  return (
    <Modal titleId="team-host-title" title={t("team.hostTitle")} onClose={onClose}>
      <form onSubmit={host}>
        <label className="share-field">
          <span className="share-label">{t("team.hostPick")}</span>
          <select className="share-input" value={space} onChange={(event) => setSpace(event.target.value)} disabled={pending}>
            {choices.map((choice) => <option key={choice} value={choice}>{choice}</option>)}
          </select>
        </label>
        <p className="share-hint">{t("team.hostHint")}</p>
        {error && <p className="share-warning" role="alert">{error}</p>}
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose} disabled={pending}>{t("sharing.cancel")}</button>
          <button type="submit" className="share-primary" disabled={!space || pending}>{t("team.hostShare")}</button>
        </div>
      </form>
    </Modal>
  );
}

function MemberMenu({ member, onEdit, onNewCode, onRemove }: {
  member: TeamMember;
  onEdit: () => void;
  onNewCode: () => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (event: MouseEvent) => { if (!ref.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  const pick = (action: () => void) => () => { setOpen(false); action(); };
  return (
    <div className="team-menu" ref={ref}>
      <button type="button" className="team-menu-button" aria-haspopup="menu" aria-expanded={open} aria-label={t("team.moreFor", { name: member.name })} onClick={() => setOpen(!open)}>⋯</button>
      {open && (
        <ul className="team-menu-list" role="menu">
          <li role="none"><button type="button" role="menuitem" onClick={pick(onEdit)}>{t("team.menuEdit")}</button></li>
          <li role="none"><button type="button" role="menuitem" onClick={pick(onNewCode)}>{t("team.menuNewCode")}</button></li>
          <li role="none"><button type="button" role="menuitem" data-danger="true" onClick={pick(onRemove)}>{t("team.menuRemove")}</button></li>
        </ul>
      )}
    </div>
  );
}

export default function TeamSection() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const status = useQuery({ queryKey: TEAM_KEY, queryFn: teamStatus });
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  const [dialog, setDialog] = useState<
    | { kind: "add" }
    | { kind: "edit"; member: TeamMember }
    | { kind: "host" }
    | { kind: "code"; name: string; code: string | null | undefined }
    | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = () => Promise.all([
    queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
    queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
  ]);

  if (status.isPending) return <p className="team-off" role="status">{t("team.loading")}</p>;
  if (status.isError || !status.data) return <p className="team-error" role="alert">{t("team.loadFailed")}</p>;
  const { listening } = status.data;
  if (!listening) {
    return (
      <p className="team-off">
        {t("team.off")}{" "}
        <button type="button" onClick={() => { void Promise.resolve(openExternal(TEAM_DOCS_URL)).catch(() => undefined); }}>{t("team.docs")}</button>
      </p>
    );
  }

  const hosted = status.data.spaces;
  const members = activeMembers(status.data.members);
  const unhosted = (spaces.data ?? []).map((space) => space.name).filter((name) => !hosted.includes(name));

  const describe = (member: TeamMember) => member.spaces.length === 0
    ? t("team.noSpacesForMember")
    : member.spaces.map((entry) => (entry.access === "read" ? `${entry.space} · ${t("team.readOnly")}` : entry.space)).join(", ");

  const removeMember = async (member: TeamMember) => {
    const ok = await confirmAction(t("team.removeConfirm", { name: member.name }), { confirm: t("team.menuRemove"), cancel: t("sharing.cancel") });
    if (!ok) return;
    setError(null);
    try {
      await teamRemoveMember(member.id);
      await refresh();
    } catch (caught) {
      setError(errorText(caught));
    }
  };
  const newCode = async (member: TeamMember) => {
    setError(null);
    try {
      const url = prefillTeamUrl(listening);
      const key = await teamNewMemberKey(member.id, url || undefined);
      if (url) rememberTeamUrl(url);
      setDialog({ kind: "code", name: member.name, code: key.join_code ?? key.key });
      await refresh();
    } catch (caught) {
      setError(errorText(caught));
    }
  };

  return (
    <>
      {error && <p className="team-error" role="alert">{error}</p>}
      <SettingsGroup className="mem-fade-up">
        <SettingRow title={t("team.serverOn", { addr: listening })} description="" control={null} />
      </SettingsGroup>
      <SettingsGroup heading={t("team.hostedHeading")} className="mem-fade-up">
        <SettingRow
          title={hosted.length > 0 ? hosted.join(", ") : t("team.noneHosted")}
          description=""
          controlLayout="inline"
          control={<Button size="sm" disabled={unhosted.length === 0} onClick={() => setDialog({ kind: "host" })}>{t("team.hostAdd")}</Button>}
        />
      </SettingsGroup>
      <SettingsGroup heading={t("team.membersHeading")} className="mem-fade-up">
        {members.map((member) => (
          <SettingRow
            key={member.id}
            title={member.name}
            description={describe(member)}
            controlLayout="inline"
            control={<MemberMenu member={member} onEdit={() => setDialog({ kind: "edit", member })} onNewCode={() => void newCode(member)} onRemove={() => void removeMember(member)} />}
          />
        ))}
        <SettingRow
          title={members.length === 0 ? t("team.noMembers") : ""}
          description=""
          controlLayout="inline"
          control={<Button size="sm" onClick={() => setDialog({ kind: "add" })}>{t("team.addPerson")}</Button>}
        />
      </SettingsGroup>
      {dialog?.kind === "host" && (
        <HostDialog choices={unhosted} onClose={() => setDialog(null)} onHosted={() => { setDialog(null); void refresh(); }} />
      )}
      {(dialog?.kind === "add" || dialog?.kind === "edit") && (
        <MemberDialog
          member={dialog.kind === "edit" ? dialog.member : null}
          hosted={hosted}
          listening={listening}
          onClose={() => setDialog(null)}
          onSaved={(key) => {
            setDialog(key ? { kind: "code", name: key.member.name, code: key.join_code ?? key.key } : null);
            void refresh();
          }}
        />
      )}
      {dialog?.kind === "code" && <CodeDialog name={dialog.name} code={dialog.code} onClose={() => setDialog(null)} />}
    </>
  );
}
