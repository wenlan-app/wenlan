// SPDX-License-Identifier: AGPL-3.0-only
// Who can use a Space of the team this computer hosts (one box, opened from
// the Space and from its row on the team page), and the pieces the team page
// shares with it: a person's menu, the access choice, an invite link.
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  teamSetEveryone,
  teamUpdateMember,
  type TeamAccess,
  type TeamMember,
  type TeamMemberKey,
  type TeamStatus,
} from "../../../lib/tauri";
import { TEAM_KEY, activeMembers, useTeamStatus } from "./team";
import "./sharing.css";

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The code to hand over: the join code, or the bare key when the server has
 *  no address to put in it. */
export function codeOf(key: TeamMemberKey): string {
  return key.join_code ?? key.key;
}

function accessOf(member: TeamMember, space: string): TeamAccess {
  return member.spaces.find((entry) => entry.space === space)?.access ?? "write";
}

export function MemberMenu({ member, onNewCode, onRemoveFromSpace, onRemoveFromTeam }: {
  readonly member: TeamMember;
  readonly onNewCode: () => void;
  /** Absent outside one Space's box. */
  readonly onRemoveFromSpace?: () => void;
  readonly onRemoveFromTeam: () => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (event: MouseEvent) => { if (!ref.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // The box around this menu would close on the same key.
      event.stopPropagation();
      setOpen(false);
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape, true);
    };
  }, [open]);
  const pick = (action: () => void) => () => { setOpen(false); action(); };
  return (
    <div className="team-menu" ref={ref}>
      <button type="button" className="team-menu-button" aria-haspopup="menu" aria-expanded={open} aria-label={t("team.moreFor", { name: member.name })} onClick={() => setOpen(!open)}>⋯</button>
      {open && (
        <ul className="team-menu-list" role="menu">
          <li role="none"><button type="button" role="menuitem" onClick={pick(onNewCode)}>{t("team.page.newLink")}</button></li>
          {onRemoveFromSpace && <li role="none"><button type="button" role="menuitem" onClick={pick(onRemoveFromSpace)}>{t("team.menuRemoveFromSpace")}</button></li>}
          <li role="none"><button type="button" role="menuitem" data-danger="true" onClick={pick(onRemoveFromTeam)}>{t("team.menuRemoveFromTeam")}</button></li>
        </ul>
      )}
    </div>
  );
}

export function AccessSelect({ label, value, disabled, onChange }: {
  readonly label: string;
  readonly value: TeamAccess;
  readonly disabled?: boolean;
  readonly onChange: (access: TeamAccess) => void;
}) {
  const { t } = useTranslation();
  return (
    <select className="share-input team-access" aria-label={label} value={value} disabled={disabled} onChange={(event) => onChange(event.target.value as TeamAccess)}>
      <option value="write">{t("team.canEdit")}</option>
      <option value="read">{t("team.readOnly")}</option>
    </select>
  );
}

export type EveryoneAccess = TeamAccess | "nobody";

/** Whether everyone on the team has `space`, and how. */
export function everyoneOf(everyone: readonly { space: string; access: TeamAccess }[] | undefined, space: string): EveryoneAccess {
  return everyone?.find((entry) => entry.space === space)?.access ?? "nobody";
}

/** Who can use a Space, in a few words: everyone (and how), the people
 *  added, or only the owner. */
export function teamAccessText(t: TFunction, space: string, status: TeamStatus | undefined): string {
  const open = everyoneOf(status?.everyone, space);
  if (open === "write") return t("team.page.everyoneWrite");
  if (open === "read") return t("team.page.everyoneRead");
  const names = activeMembers(status?.members ?? [])
    .filter((member) => member.spaces.some((entry) => entry.space === space))
    .map((member) => member.name);
  return names.length > 0 ? names.join(t("team.page.nameSeparator")) : t("team.page.onlyYou");
}

/** An invite link shown once, in the box it was made in. It is not printed:
 *  the button copies it, so no address or key is on screen. */
export function IssuedCode({ name, code, onDone }: { readonly name: string; readonly code: string; readonly onDone: () => void }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      setFailed(true);
    }
  };
  return (
    <div className="share-found team-issued" role="group" aria-label={t("team.page.linkFor", { name })}>
      <div className="team-issued-head">
        <span className="team-issued-title">{t("team.page.linkFor", { name })}</span>
        <button type="button" className="share-primary" onClick={() => void copy()}>{t(copied ? "team.copied" : "team.page.copyLink")}</button>
      </div>
      <span className="share-hint">{t("team.page.linkPrivate")}</span>
      <span className="share-hint">{t("team.page.linkGets", { name })}</span>
      {/* When the clipboard is refused, the link is shown to copy by hand. */}
      {failed && <input className="share-input team-code" readOnly value={code} aria-label={t("team.page.linkFor", { name })} onFocus={(event) => event.currentTarget.select()} />}
      <div className="team-issued-done"><button type="button" className="share-link" onClick={onDone}>{t("team.codeDone")}</button></div>
    </div>
  );
}

/** Who can use one Space of the team this computer hosts: everyone on the
 *  team, or only some people, picked from the team. New people come in only
 *  through the team page's invite. */
export function TeamSpaceAccess({ space, team, onError }: { readonly space: string; readonly team: string; readonly onError: (message: string | null) => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const status = useTeamStatus();
  const [pending, setPending] = useState(false);
  // The owner has every Space already, so is never added to one.
  const people = activeMembers(status.data?.members ?? []).filter((member) => member.role !== "owner");
  const open = everyoneOf(status.data?.everyone, space);
  const added = people.filter((member) => member.spaces.some((entry) => entry.space === space));
  const others = people.filter((member) => !added.includes(member));

  const run = async (action: () => Promise<void>) => {
    onError(null);
    setPending(true);
    try {
      await action();
      await queryClient.invalidateQueries({ queryKey: TEAM_KEY });
    } catch (caught) {
      onError(errorText(caught));
    }
    setPending(false);
  };
  const setOpen = (next: EveryoneAccess) => run(async () => { await teamSetEveryone(space, next); });
  const changeAccess = (member: TeamMember, next: TeamAccess) => run(async () => {
    await teamUpdateMember(member.id, {
      spaces: member.spaces.map((entry) => (entry.space === space ? { ...entry, access: next } : entry)),
    });
  });
  const addPerson = (id: string) => {
    const member = others.find((candidate) => candidate.id === id);
    if (!member) return;
    void run(async () => {
      await teamUpdateMember(member.id, { spaces: [...member.spaces, { space, access: "write" }] });
    });
  };
  const removePerson = (member: TeamMember) => run(async () => {
    await teamUpdateMember(member.id, { spaces: member.spaces.filter((entry) => entry.space !== space) });
  });

  return (
    <div className="team-space-access" role="radiogroup" aria-label={t("team.page.whoCanUse", { space })}>
      <label className="team-space-access-choice">
        <input type="radio" name={`team-access-${space}`} checked={open !== "nobody"} disabled={pending} onChange={() => void setOpen("write")} />
        <span>{t("team.page.everyoneRow")}</span>
        {open !== "nobody" && <AccessSelect label={t("team.accessOf", { name: t("team.page.everyoneRow") })} value={open} disabled={pending} onChange={(next) => void setOpen(next)} />}
      </label>
      <label className="team-space-access-choice">
        <input type="radio" name={`team-access-${space}`} checked={open === "nobody"} disabled={pending} onChange={() => void setOpen("nobody")} />
        <span>{t("team.access.some")}</span>
      </label>
      {open === "nobody" && (
        <ul className="team-space-access-people">
          {added.map((member) => (
            <li key={member.id}>
              <span className="team-member-name">{member.name}</span>
              <AccessSelect label={t("team.accessOf", { name: member.name })} value={accessOf(member, space)} disabled={pending} onChange={(next) => void changeAccess(member, next)} />
              <button type="button" className="share-link" disabled={pending} aria-label={t("team.access.removeName", { name: member.name })} onClick={() => void removePerson(member)}>{t("team.access.remove")}</button>
            </li>
          ))}
          <li>
            {others.length > 0 ? (
              <select className="share-input team-space-access-add" aria-label={t("team.access.addFrom", { team })} value="" disabled={pending} onChange={(event) => addPerson(event.target.value)}>
                <option value="">{t("team.access.addFrom", { team })}</option>
                {others.map((member) => <option key={member.id} value={member.id}>{member.name}</option>)}
              </select>
            ) : (
              <span className="share-hint">{added.length === 0 ? t("team.access.noPeople") : t("team.access.everyoneAdded")}</span>
            )}
          </li>
        </ul>
      )}
    </div>
  );
}
