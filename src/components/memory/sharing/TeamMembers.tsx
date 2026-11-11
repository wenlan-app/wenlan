// SPDX-License-Identifier: AGPL-3.0-only
// The people of a Space this computer hosts for a team: who can use it, with
// what access, and a row to add someone. Lives in the Space's members box.
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  teamAddMember,
  teamNewMemberKey,
  teamRemoveMember,
  teamSetEveryone,
  teamUpdateMember,
  type TeamAccess,
  type TeamMember,
  type TeamMemberKey,
} from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";
import { TEAM_KEY, activeMembers, prefillTeamUrl, rememberTeamUrl, sameName, useTeamStatus } from "./team";
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
          <li role="none"><button type="button" role="menuitem" onClick={pick(onNewCode)}>{t("team.menuNewCode")}</button></li>
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

/** Who can use a hosted Space without being added one by one: everyone on
 *  the team (editing or reading), or only the people added. */
export function EveryoneSelect({ label, value, disabled, onChange }: {
  readonly label: string;
  readonly value: EveryoneAccess;
  readonly disabled?: boolean;
  readonly onChange: (access: EveryoneAccess) => void;
}) {
  const { t } = useTranslation();
  return (
    <select className="share-input team-access team-everyone" aria-label={label} value={value} disabled={disabled} onChange={(event) => onChange(event.target.value as EveryoneAccess)}>
      <option value="write">{t("team.page.everyoneWrite")}</option>
      <option value="read">{t("team.page.everyoneRead")}</option>
      <option value="nobody">{t("team.page.onlyAdded")}</option>
    </select>
  );
}

/** A join code shown once, in the box it was made in. */
export function IssuedCode({ name, code, onDone }: { readonly name: string; readonly code: string; readonly onDone: () => void }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      setCopied(true);
    } catch {
      // The field stays selectable to copy by hand.
    }
  };
  return (
    <div className="share-found team-issued" role="group" aria-label={t("team.codeField")}>
      <span>{t("team.codeTitle", { name })}</span>
      <div className="team-code-row">
        <input className="share-input team-code" readOnly value={code} aria-label={t("team.codeField")} onFocus={(event) => event.currentTarget.select()} />
        <button type="button" className="share-secondary" onClick={() => void copy()}>{t(copied ? "team.copied" : "team.copy")}</button>
      </div>
      <div><button type="button" className="share-link" onClick={onDone}>{t("team.codeDone")}</button></div>
    </div>
  );
}

export function TeamMembers({ space, onError }: { readonly space: string; readonly onError: (message: string | null) => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const status = useTeamStatus();
  const listening = status.data?.listening;
  const [issued, setIssued] = useState<{ name: string; code: string } | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [access, setAccess] = useState<TeamAccess>("write");
  const [address, setAddress] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const everyone = activeMembers(status.data?.members ?? []);
  const members = everyone.filter((member) => member.spaces.some((entry) => entry.space === space));
  // Nothing remembered and nothing the listener says: ask once, here.
  const knownUrl = prefillTeamUrl(listening);
  const url = address ?? knownUrl;
  const askForAddress = Boolean(status.data) && !knownUrl;

  const refresh = () => queryClient.invalidateQueries({ queryKey: TEAM_KEY });
  const run = async (action: () => Promise<void>) => {
    onError(null);
    setNote(null);
    try {
      await action();
      await refresh();
    } catch (caught) {
      onError(errorText(caught));
    }
  };

  const changeEveryone = (next: EveryoneAccess) => run(async () => {
    await teamSetEveryone(space, next);
  });
  const changeAccess = (member: TeamMember, next: TeamAccess) => run(async () => {
    await teamUpdateMember(member.id, {
      spaces: member.spaces.map((entry) => (entry.space === space ? { ...entry, access: next } : entry)),
    });
  });
  const newCode = (member: TeamMember) => run(async () => {
    const key = await teamNewMemberKey(member.id, url.trim() || undefined);
    if (url.trim()) rememberTeamUrl(url.trim());
    setIssued({ name: member.name, code: codeOf(key) });
  });
  const removeFromSpace = async (member: TeamMember) => {
    const ok = await confirmAction(t("team.removeFromSpaceConfirm", { name: member.name, space }), { confirm: t("team.menuRemoveFromSpace"), cancel: t("sharing.cancel") });
    if (!ok) return;
    await run(async () => {
      await teamUpdateMember(member.id, { spaces: member.spaces.filter((entry) => entry.space !== space) });
    });
  };
  const removeFromTeam = async (member: TeamMember) => {
    const ok = await confirmAction(t("team.removeConfirm", { name: member.name }), { confirm: t("team.menuRemoveFromTeam"), cancel: t("sharing.cancel") });
    if (!ok) return;
    await run(async () => { await teamRemoveMember(member.id); });
  };

  const add = async (event: FormEvent) => {
    event.preventDefault();
    const typed = name.trim();
    if (!typed || pending) return;
    setPending(true);
    await run(async () => {
      const existing = everyone.find((member) => sameName(member.name, typed));
      if (existing) {
        // Same person: they keep the code they have and gain this Space.
        await teamUpdateMember(existing.id, {
          spaces: [...existing.spaces.filter((entry) => entry.space !== space), { space, access }],
        });
        setIssued(null);
        setNote(t("team.addedExisting", { name: existing.name, space }));
      } else {
        const address = url.trim();
        const key = await teamAddMember(typed, [{ space, access }], address || undefined);
        if (address) rememberTeamUrl(address);
        setIssued({ name: key.member.name, code: codeOf(key) });
      }
      setName("");
    });
    setPending(false);
  };

  return (
    <>
      <ul className="share-member-list team-member-list">
        <li>
          <span className="team-member-name">{t("team.page.everyoneRow")}</span>
          <span className="team-member-controls">
            <EveryoneSelect label={t("team.page.whoCanUse", { space })} value={everyoneOf(status.data?.everyone, space)} onChange={(next) => void changeEveryone(next)} />
          </span>
        </li>
        {members.map((member) => (
          <li key={member.id}>
            <span className="team-member-name">{member.name}</span>
            <span className="team-member-controls">
              <AccessSelect label={t("team.accessOf", { name: member.name })} value={accessOf(member, space)} onChange={(next) => void changeAccess(member, next)} />
              <MemberMenu
                member={member}
                onNewCode={() => void newCode(member)}
                onRemoveFromSpace={() => void removeFromSpace(member)}
                onRemoveFromTeam={() => void removeFromTeam(member)}
              />
            </span>
          </li>
        ))}
      </ul>
      {issued && <IssuedCode name={issued.name} code={issued.code} onDone={() => setIssued(null)} />}
      {note && <p className="share-hint" role="status">{note}</p>}
      {askForAddress && (
        <label className="share-field">
          <span className="share-label">{t("team.urlLabel")}</span>
          <input className="share-input" value={url} onChange={(event) => setAddress(event.target.value)} placeholder={t("team.urlPlaceholder")} spellCheck={false} />
        </label>
      )}
      <form className="team-add-row" onSubmit={add}>
        <input className="share-input" aria-label={t("team.name")} placeholder={t("team.namePlaceholder")} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} />
        <AccessSelect label={t("team.accessNew")} value={access} disabled={pending} onChange={setAccess} />
        <button type="submit" className="share-primary" disabled={!name.trim() || pending}>{t("team.addShort")}</button>
      </form>
    </>
  );
}
