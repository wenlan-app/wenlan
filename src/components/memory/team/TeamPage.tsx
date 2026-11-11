// SPDX-License-Identifier: AGPL-3.0-only
// The team's own page, opened from the team's row in the left navigation.
// One layout for everyone on the team, in this order: what waits for you,
// the team's Spaces and who can use each, its people, and what changed
// lately. The owner (the person at the server's computer) also invites
// people, adds Spaces and opens each one's access box; a member reads the
// same page and can leave the team.
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  getPage,
  listPendingRevisions,
  listSpaces,
  stopSharingSpace,
  teamAddMember,
  teamHostSpace,
  teamNewMemberKey,
  teamRemoveMember,
  teamRename,
  type SharedStatus,
  type TeamInfo,
  type TeamMember,
  type TeamStatus,
} from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";
import { SpaceRecentChanges } from "../space-detail/SpaceRecentChanges";
import { SHARES_KEY, sharedWriter, teamKeyOf, teamLabel, useShares } from "../sharing/sharing";
import { TEAM_KEY, activeMembers, prefillTeamUrl, rememberTeamUrl, sameName, useTeamStatus } from "../sharing/team";
import { IssuedCode, MemberMenu, codeOf, errorText, teamAccessText } from "../sharing/TeamMembers";
import { ShareDialog } from "../sharing/ShareDialog";
import "../sharing/sharing.css";
import "../space-detail/space-brief.css";
import "../space-detail/space-recent-changes.css";
import "./team-page.css";

/** One team this computer is part of: the one it hosts, or one per team it
 *  syncs Spaces from. */
export type TeamGroup = {
  readonly key: string;
  /** The team's id and name, once known. */
  readonly team: TeamInfo | null;
  /** This computer is the team's server: its owner is here. */
  readonly hosted: boolean;
  /** The team server's address, for a team joined from here. Never shown. */
  readonly host: string | null;
  readonly shares: SharedStatus[];
};

/** The teams in `shares`, the hosted one first, each known by its team id.
 *  A computer that hosts a team with people but no Space yet still has it. */
export function teamGroups(shares: readonly SharedStatus[] | undefined, status: TeamStatus | undefined): TeamGroup[] {
  const hosted: SharedStatus[] = [];
  const joined = new Map<string, { team: TeamInfo | null; host: string; shares: SharedStatus[] }>();
  for (const share of shares ?? []) {
    if (share.kind !== "space") continue;
    if (share.hosted) {
      hosted.push(share);
      continue;
    }
    const key = teamKeyOf(share);
    if (!key || !share.host) continue;
    const group = joined.get(key) ?? { team: share.team ?? null, host: share.host, shares: [] };
    group.shares.push(share);
    joined.set(key, group);
  }
  const groups: TeamGroup[] = [];
  if (hosted.length > 0 || (status?.members?.length ?? 0) > 0 || (status?.spaces?.length ?? 0) > 0) {
    groups.push({ key: "hosted", team: status?.team ?? hosted[0]?.team ?? null, hosted: true, host: null, shares: hosted });
  }
  for (const [key, group] of joined) groups.push({ key, team: group.team, hosted: false, host: group.host, shares: group.shares });
  return groups;
}

/** The teams this computer is on, for the left navigation: read from the
 *  shares alone, since the shell asks on every screen and the team server's
 *  own status is asked only on the Team page. */
export function useTeams(): TeamGroup[] {
  const shares = useShares();
  return teamGroups(shares.data, undefined);
}

interface TeamPageProps {
  readonly onOpenSpace: (space: string) => void;
  readonly onOpenPage: (pageId: string) => void;
}

export function TeamPage({ onOpenSpace, onOpenPage }: TeamPageProps) {
  const { t } = useTranslation();
  const shares = useShares();
  const status = useTeamStatus();
  const groups = teamGroups(shares.data, status.data);
  return (
    <div className="team-page">
      {groups.length === 0 && (shares.isPending ? null : <p className="team-page-empty">{t("team.page.none")}</p>)}
      {groups.map((group) => (
        <Team key={group.key} group={group} status={group.hosted ? status.data : undefined} shares={shares.data} onOpenSpace={onOpenSpace} onOpenPage={onOpenPage} />
      ))}
    </div>
  );
}

type Run = (action: () => Promise<void>) => Promise<boolean>;

function useRun(setError: (message: string | null) => void): Run {
  const queryClient = useQueryClient();
  return async (action) => {
    setError(null);
    try {
      await action();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
      ]);
      return true;
    } catch (caught) {
      setError(errorText(caught));
      return false;
    }
  };
}

/** One team, laid out the same for its owner and its members. */
function Team({ group, status, shares, onOpenSpace, onOpenPage }: {
  readonly group: TeamGroup;
  /** The team server's own status, on the computer that hosts it. */
  readonly status: TeamStatus | undefined;
  readonly shares: readonly SharedStatus[] | undefined;
  readonly onOpenSpace: (space: string) => void;
  readonly onOpenPage: (id: string) => void;
}) {
  const { t } = useTranslation();
  const [error, setError] = useState<string | null>(null);
  const [renaming, setRenaming] = useState(false);
  const run = useRun(setError);
  const owner = group.hosted;
  const name = teamLabel(t, group.team);
  const named = Boolean(group.team?.name?.trim());
  const spaces = owner ? (status?.spaces ?? group.shares.map((share) => share.target)) : group.shares.map((share) => share.target);
  const waiting = useWaitingRows(shares, group.shares, onOpenPage);
  const leave = async () => {
    const ok = await confirmAction(t("team.page.leaveConfirm", { team: name }), { confirm: t("team.page.leaveTeam", { team: name }), cancel: t("sharing.cancel") });
    if (!ok) return;
    await run(async () => {
      for (const share of group.shares) await stopSharingSpace(share.target);
    });
  };

  return (
    <section className="team-page-team" aria-label={name}>
      <div className="team-page-head">
        <h1 className="team-page-title">{name}</h1>
        {(named || !owner) && (
          <TeamMenu
            label={t("team.page.moreTeam", { team: name })}
            items={owner
              ? [{ label: t("team.page.rename"), onPick: () => setRenaming(true) }]
              : [{ label: t("team.page.leaveTeam", { team: name }), onPick: () => void leave(), danger: true }]}
          />
        )}
      </div>
      {owner && (!named || renaming) && (
        <NameTeam initial={group.team?.name ?? ""} onDone={() => setRenaming(false)} canCancel={named} />
      )}

      {waiting && (
        <section className="space-recent-changes team-page-waiting-block" aria-label={t("team.page.waitingTitle")}>
          <h2 className="space-brief-heading">{t("team.page.waitingTitle")}</h2>
          <ul>{waiting}</ul>
        </section>
      )}

      <TeamSpaces group={group} status={status} shares={shares} spaces={spaces} owner={owner} teamName={name} run={run} onOpenSpace={onOpenSpace} />
      <TeamPeople group={group} status={status} owner={owner} run={run} />
      {error && <p className="share-error team-page-error" role="alert">{error}</p>}
      <SpaceRecentChanges space={spaces} shares={shares} />
    </section>
  );
}

/** A small "⋯" menu beside the team's name. */
function TeamMenu({ label, items }: {
  readonly label: string;
  readonly items: readonly { label: string; onPick: () => void; danger?: boolean }[];
}) {
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
  return (
    <div className="team-menu" ref={ref}>
      <button type="button" className="team-menu-button" aria-haspopup="menu" aria-expanded={open} aria-label={label} onClick={() => setOpen(!open)}>⋯</button>
      {open && (
        <ul className="team-menu-list" role="menu">
          {items.map((item) => (
            <li key={item.label} role="none">
              <button type="button" role="menuitem" data-danger={item.danger ? "true" : undefined} onClick={() => { setOpen(false); item.onPick(); }}>{item.label}</button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/** The owner names the team once, and may rename it; everyone on it sees
 *  the name. */
function NameTeam({ initial, canCancel, onDone }: { readonly initial: string; readonly canCancel: boolean; readonly onDone: () => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const run = useRun(setError);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const typed = name.trim();
    if (!typed || pending) return;
    setPending(true);
    const ok = await run(async () => { await teamRename(typed); });
    setPending(false);
    if (ok) onDone();
  };
  return (
    <form className="team-page-name-team" onSubmit={(event) => void submit(event)}>
      <label className="share-label" htmlFor="team-name">{t("team.page.nameTeam")}</label>
      <div className="team-page-name-row">
        <input id="team-name" className="share-input" placeholder={t("team.page.nameTeamPlaceholder")} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} maxLength={60} autoFocus={canCancel} />
        <button type="submit" className="share-primary" disabled={!name.trim() || pending}>{t("team.page.nameTeamSave")}</button>
        {canCancel && <button type="button" className="share-link" onClick={onDone}>{t("sharing.cancel")}</button>}
      </div>
      {error && <p className="share-error team-page-error" role="alert">{error}</p>}
    </form>
  );
}

/** AI changes from teammates that wait on a locked page of these Spaces:
 *  one row each, opening the page where it is reviewed. */
function useWaitingRows(shares: readonly SharedStatus[] | undefined, teamShares: readonly SharedStatus[], onOpenPage: (id: string) => void) {
  const { t } = useTranslation();
  const pending = useQuery({
    queryKey: ["team-pending-revisions"],
    queryFn: () => listPendingRevisions(50),
    staleTime: 15_000,
    refetchInterval: 30_000,
    retry: false,
  });
  const spaces = new Set(teamShares.map((share) => share.target));
  const items = (pending.data ?? []).filter((item) => item.target_kind === "page" && sharedWriter(item.edited_by, teamShares, t));
  const pages = useQueries({
    queries: items.map((item) => ({
      queryKey: ["page", item.target_source_id],
      queryFn: () => getPage(item.target_source_id),
      staleTime: 60_000,
    })),
  });
  const rows = items.flatMap((item, index) => {
    const page = pages[index]?.data;
    if (!page || !spaces.has(page.space ?? "")) return [];
    const who = sharedWriter(item.edited_by, shares, t) ?? "";
    return [(
      <li key={item.revision_source_id}>
        <button type="button" onClick={() => onOpenPage(item.target_source_id)}>
          <span className="space-recent-changes-what">{t("team.page.wants", { who, title: page.title })}</span>
          <span className="space-recent-changes-meta team-page-waiting">{t("team.page.review")}</span>
        </button>
      </li>
    )];
  });
  return rows.length > 0 ? <>{rows}</> : undefined;
}

function SectionHeading({ title, action }: { readonly title: string; readonly action?: React.ReactNode }) {
  return (
    <div className="team-page-heading">
      <h2 className="space-brief-heading">{title}</h2>
      {action}
    </div>
  );
}

function TeamSpaces({ group, status, shares, spaces, owner, teamName, run, onOpenSpace }: {
  readonly group: TeamGroup;
  readonly status: TeamStatus | undefined;
  readonly shares: readonly SharedStatus[] | undefined;
  readonly spaces: readonly string[];
  readonly owner: boolean;
  readonly teamName: string;
  readonly run: Run;
  readonly onOpenSpace: (space: string) => void;
}) {
  const { t } = useTranslation();
  const [boxFor, setBoxFor] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const allSpaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces, enabled: owner });
  // The owner's Private Spaces: not shared any way yet.
  const addable = (allSpaces.data ?? [])
    .map((space) => space.name)
    .filter((name) => !(shares ?? []).some((share) => share.kind === "space" && share.target === name));
  const add = async (space: string) => {
    setAdding(false);
    if (!space) return;
    const ok = await confirmAction(t("team.page.addSpaceConfirm", { space, team: teamName }), { confirm: t("team.page.addSpaceAction"), cancel: t("sharing.cancel") });
    if (ok) await run(async () => { await teamHostSpace(space); });
  };

  return (
    <div className="team-page-block">
      <SectionHeading title={t("team.page.spaces")} />
      {spaces.length === 0 ? (
        <p className="team-page-quiet">{t("team.noneHosted")}</p>
      ) : (
        <ul className="team-page-rows">
          {spaces.map((space) => {
            const share = group.shares.find((candidate) => candidate.target === space);
            return (
              <li key={space}>
                <button type="button" className="team-page-name team-page-open" onClick={() => onOpenSpace(space)}>{space}</button>
                {owner ? (
                  <button type="button" className="team-page-meta team-page-access" aria-label={t("team.page.whoCanUse", { space })} onClick={() => setBoxFor(space)}>
                    {teamAccessText(t, space, status)}
                  </button>
                ) : (
                  <span className="team-page-meta">{t(share?.read_only ? "team.readOnly" : "team.canEdit")}</span>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {owner && addable.length > 0 && (adding ? (
        <select className="share-input team-page-add-space" aria-label={t("team.page.addSpacePick")} value="" autoFocus onChange={(event) => void add(event.target.value)} onBlur={() => setAdding(false)}>
          <option value="">{t("team.page.addSpacePick")}</option>
          {addable.map((name) => <option key={name} value={name}>{name}</option>)}
        </select>
      ) : (
        <div><button type="button" className="share-link team-page-add" onClick={() => setAdding(true)}>{t("team.page.addSpace")}</button></div>
      ))}
      {boxFor && <ShareDialog kind="space" target={boxFor} title={boxFor} onClose={() => setBoxFor(null)} />}
    </div>
  );
}

function TeamPeople({ group, status, owner, run }: {
  readonly group: TeamGroup;
  readonly status: TeamStatus | undefined;
  readonly owner: boolean;
  readonly run: Run;
}) {
  const { t } = useTranslation();
  const [inviting, setInviting] = useState(false);
  const [issued, setIssued] = useState<{ name: string; code: string } | null>(null);
  const url = prefillTeamUrl(status?.listening);
  const me = group.shares[0];
  // The owner's own name: what the team sees for edits made on the server.
  const ownerName = me?.member_name;
  const newLink = (member: TeamMember) => run(async () => {
    const key = await teamNewMemberKey(member.id, url.trim() || undefined);
    setIssued({ name: member.name, code: codeOf(key) });
  });
  const remove = async (member: TeamMember) => {
    const ok = await confirmAction(t("team.removeConfirm", { name: member.name }), { confirm: t("team.menuRemoveFromTeam"), cancel: t("sharing.cancel") });
    if (ok) await run(async () => { await teamRemoveMember(member.id); });
  };

  // A member's computer knows people from the Spaces' member lists.
  const joined = new Map<string, string>();
  if (!owner) for (const share of group.shares) for (const member of share.members) joined.set(member.id, member.name);
  const members = owner ? activeMembers(status?.members ?? []) : [];

  return (
    <div className="team-page-block">
      <SectionHeading
        title={t("team.page.people")}
        action={owner && !inviting && <button type="button" className="share-secondary team-page-action" onClick={() => { setInviting(true); setIssued(null); }}>{t("team.page.invite")}</button>}
      />
      {inviting && (
        <InviteForm
          members={members}
          initialUrl={url}
          onCancel={() => setInviting(false)}
          onIssued={(next) => { setInviting(false); setIssued(next); }}
          run={run}
        />
      )}
      {issued && <IssuedCode name={issued.name} code={issued.code} onDone={() => setIssued(null)} />}
      <ul className="team-page-rows">
        {owner ? (
          <>
            {ownerName && <li><span className="team-page-name">{t("team.page.youOwner", { name: ownerName })}</span></li>}
            {members.map((member) => (
              <li key={member.id}>
                <span className="team-page-name">{member.name}</span>
                <span className="team-page-meta" />
                <MemberMenu member={member} onNewCode={() => void newLink(member)} onRemoveFromTeam={() => void remove(member)} />
              </li>
            ))}
          </>
        ) : (
          [...joined].map(([id, name]) => (
            <li key={id}><span className="team-page-name">{id === me?.member_id ? t("team.page.you", { name }) : name}</span></li>
          ))
        )}
      </ul>
      {owner && members.length === 0 && !inviting && !issued && <p className="team-page-quiet">{t("team.page.noMembers")}</p>}
    </div>
  );
}

/** Invite one person: their name, then a link to copy. They get every
 *  Space open to everyone; a Space for some people is opened to them from
 *  that Space. Someone already on the team gets a new link. */
function InviteForm({ members, initialUrl, onCancel, onIssued, run }: {
  readonly members: readonly TeamMember[];
  readonly initialUrl: string;
  readonly onCancel: () => void;
  readonly onIssued: (issued: { name: string; code: string }) => void;
  readonly run: Run;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [address, setAddress] = useState(initialUrl);
  const [pending, setPending] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const typed = name.trim();
    if (!typed || pending) return;
    setPending(true);
    const url = address.trim();
    await run(async () => {
      const existing = members.find((member) => sameName(member.name, typed));
      const key = existing
        ? await teamNewMemberKey(existing.id, url || undefined)
        : await teamAddMember(typed, [], url || undefined);
      if (url) rememberTeamUrl(url);
      onIssued({ name: key.member.name, code: codeOf(key) });
    });
    setPending(false);
  };

  return (
    <form className="team-invite" onSubmit={(event) => void submit(event)}>
      <div className="team-invite-row">
        <input className="share-input" aria-label={t("team.name")} placeholder={t("team.namePlaceholder")} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} autoFocus />
        <button type="submit" className="share-primary" disabled={!name.trim() || pending}>{t("team.page.createLink")}</button>
        <button type="button" className="share-link" onClick={onCancel}>{t("sharing.cancel")}</button>
      </div>
      {!initialUrl && (
        <label className="share-field">
          <span className="share-label">{t("team.urlLabel")}</span>
          <input className="share-input" value={address} onChange={(event) => setAddress(event.target.value)} placeholder={t("team.urlPlaceholder")} spellCheck={false} />
        </label>
      )}
    </form>
  );
}
