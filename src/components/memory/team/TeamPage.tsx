// SPDX-License-Identifier: AGPL-3.0-only
// A team's parts of the app. Home on a team's side is the same home as on
// Private (one home, one layout); it adds what waits for this person and who
// changed what. The team's people, its name, inviting and leaving sit in the
// switch at the top left, so no page grows with the number of people.
import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  getPage,
  listPendingRevisions,
  stopSharingSpace,
  teamAddMember,
  teamForgetMembership,
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
import { IssuedCode, MemberMenu, codeOf, errorText } from "../sharing/TeamMembers";
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
  /** This person owns the team and manages it from here: on its server
   *  (hosted), or from their own computer for a server nobody sits at. */
  readonly owner: boolean;
  /** The team server's address, for a team joined from here. Never shown. */
  readonly host: string | null;
  readonly shares: SharedStatus[];
};

/** The teams in `shares`, the hosted one first, each known by its team id.
 *  A computer that hosts a team with people but no Space yet still has it,
 *  and so does one just started here. A team whose server is elsewhere and
 *  that this person owns (`status.remote`) is theirs to manage from here,
 *  even before it has a Space. */
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
  const here = status && !status.remote ? status : undefined;
  if (hosted.length > 0 || (here?.members?.length ?? 0) > 0 || (here?.spaces?.length ?? 0) > 0 || Boolean(here?.listening)) {
    groups.push({ key: "hosted", team: here?.team ?? hosted[0]?.team ?? null, hosted: true, owner: true, host: null, shares: hosted });
  }
  const owned = status?.remote && status.team?.id ? `team:${status.team.id}` : null;
  for (const [key, group] of joined) groups.push({ key, team: group.team, hosted: false, owner: key === owned, host: group.host, shares: group.shares });
  if (owned && status?.remote && !joined.has(owned)) {
    groups.push({ key: owned, team: status.team ?? null, hosted: false, owner: true, host: status.remote, shares: [] });
  }
  return groups;
}

/** The teams this computer is on, for the left navigation: read from the
 *  shares alone, since the shell asks on every screen and the team server's
 *  own status is asked only on the Team page. */
export function useTeams(): TeamGroup[] {
  const shares = useShares();
  return teamGroups(shares.data, undefined);
}

/** What waits for this person in the shown team: teammates' AI changes on
 *  a locked page. Home shows it first, only when there is something. */
export function TeamWaiting({ group, onOpenPage }: { readonly group: TeamGroup; readonly onOpenPage: (id: string) => void }) {
  const { t } = useTranslation();
  const shares = useShares();
  const waiting = useWaitingRows(shares.data, group.shares, onOpenPage);
  if (!waiting) return null;
  return (
    <section className="space-recent-changes team-home-waiting" aria-label={t("team.page.waitingTitle")}>
      <h2 className="space-brief-heading">{t("team.page.waitingTitle")}</h2>
      <ul>{waiting}</ul>
    </section>
  );
}

/** Who changed what lately in the shown team's Spaces. */
export function TeamChanges({ group }: { readonly group: TeamGroup }) {
  const shares = useShares();
  const status = useTeamStatus();
  const spaces = spacesOfTeam(group, group.hosted ? status.data?.spaces : undefined);
  return <SpaceRecentChanges space={spaces} shares={shares.data} />;
}

/** The team's people, opened from the switch at the top left. */
export function TeamPeopleDialog({ group, inviteFirst = false, onClose }: {
  readonly group: TeamGroup;
  /** Opens on the invite form: the switch's 「邀請人…」. */
  readonly inviteFirst?: boolean;
  readonly onClose: () => void;
}) {
  const { t } = useTranslation();
  const status = useTeamStatus();
  const [error, setError] = useState<string | null>(null);
  const run = useRun(setError);
  const teamStatus = group.owner ? status.data : undefined;
  const { people, members } = usePeople(group, teamStatus, group.owner);
  return (
    <PeopleBox
      teamName={teamLabel(t, group.team)}
      people={people}
      members={members}
      owner={group.owner}
      inviteFirst={inviteFirst}
      url={group.hosted ? prefillTeamUrl(teamStatus?.listening) || teamStatus?.url || "" : teamStatus?.url ?? ""}
      run={run}
      error={error}
      onClose={onClose}
    />
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

/** The owner names the team once, and may rename it; everyone on it sees
 *  the name. */
export function NameTeam({ initial, canCancel, onDone }: { readonly initial: string; readonly canCancel: boolean; readonly onDone: () => void }) {
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

/** Naming or renaming the team, opened from the switch at the top left. */
export function TeamNameDialog({ group, onClose }: { readonly group: TeamGroup; readonly onClose: () => void }) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  return (
    <dialog ref={dialogRef} className="share-dialog team-name-box" aria-label={group.team?.name ?? undefined} onCancel={(event) => { event.preventDefault(); onClose(); }}>
      <NameTeam initial={group.team?.name ?? ""} canCancel onDone={onClose} />
    </dialog>
  );
}

/** A member leaves the team: every Space it shares stops syncing here, and
 *  the pages already here stay. Asks first. */
export function useLeaveTeam(group: TeamGroup | null): () => Promise<boolean> {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  return async () => {
    if (!group) return false;
    const name = teamLabel(t, group.team);
    const ok = await confirmAction(t("team.page.leaveConfirm", { team: name }), { confirm: t("team.page.leaveTeam", { team: name }), cancel: t("sharing.cancel") });
    if (!ok) return false;
    for (const share of group.shares) await stopSharingSpace(share.target);
    if (group.host) await teamForgetMembership(group.host);
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
      queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
    ]);
    return true;
  };
}

/** The Spaces one team holds: the hosted team's from its server's own list
 *  too, since a Space just added may not have synced a first time yet. */
export function spacesOfTeam(group: TeamGroup, hostedSpaces: readonly string[] | undefined): string[] {
  const names = new Set(group.shares.map((share) => share.target));
  // Only this computer's own list: a server elsewhere names its Spaces as it
  // calls them, and one here may be a private Space of the same name.
  if (group.hosted) for (const space of hostedSpaces ?? []) names.add(space);
  return [...names];
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

/** Everyone on the team, the owner first: what the page's "N people"
 *  button counts and the people box lists. */
type Person = { readonly id: string; readonly name: string; readonly label: string; readonly member?: TeamMember };

function usePeople(group: TeamGroup, status: TeamStatus | undefined, owner: boolean): { people: Person[]; members: TeamMember[] } {
  const { t } = useTranslation();
  const me = group.shares[0];
  if (owner) {
    const all = activeMembers(status?.members ?? []);
    // The owner manages a server nobody sits at from here: their own
    // record, never removed. On the server itself, the owner is the person
    // at it, named as the team sees edits made there.
    const ownerRecord = all.find((member) => member.role === "owner");
    const members = all.filter((member) => member.role !== "owner");
    const ownerName = ownerRecord?.name ?? me?.member_name;
    const people: Person[] = [
      ...(ownerName ? [{ id: ownerRecord?.id ?? "owner", name: ownerName, label: t("team.page.youOwner", { name: ownerName }) }] : []),
      ...members.map((member) => ({ id: member.id, name: member.name, label: member.name, member })),
    ];
    return { people, members };
  }
  // A member's computer knows people from the Spaces' member lists.
  const joined = new Map<string, string>();
  for (const share of group.shares) for (const member of share.members) joined.set(member.id, member.name);
  const people = [...joined].map(([id, name]) => ({ id, name, label: id === me?.member_id ? t("team.page.you", { name }) : name }));
  return { people, members: [] };
}

function initialOf(name: string): string {
  return [...name.trim()][0]?.toUpperCase() ?? "?";
}

/** Above this many people the box offers a search field. */
const SEARCH_FROM = 8;

function PeopleBox({ teamName, people, members, owner, inviteFirst, url, run, error, onClose }: {
  readonly teamName: string;
  readonly people: readonly Person[];
  readonly members: readonly TeamMember[];
  readonly owner: boolean;
  readonly inviteFirst: boolean;
  readonly url: string;
  readonly run: Run;
  readonly error?: string | null;
  readonly onClose: () => void;
}) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [inviting, setInviting] = useState(inviteFirst);
  const [issued, setIssued] = useState<{ name: string; code: string } | null>(null);
  const [query, setQuery] = useState("");
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  const newLink = (member: TeamMember) => run(async () => {
    const key = await teamNewMemberKey(member.id, url.trim() || undefined);
    setIssued({ name: member.name, code: codeOf(key) });
  });
  const remove = async (member: TeamMember) => {
    const ok = await confirmAction(t("team.removeConfirm", { name: member.name }), { confirm: t("team.menuRemoveFromTeam"), cancel: t("sharing.cancel") });
    if (ok) await run(async () => { await teamRemoveMember(member.id); });
  };
  const needle = query.trim().toLocaleLowerCase();
  const shown = needle ? people.filter((person) => person.name.toLocaleLowerCase().includes(needle)) : people;

  return (
    <dialog
      ref={dialogRef}
      className="share-dialog team-people-box"
      aria-labelledby="team-people-title"
      onCancel={(event) => { event.preventDefault(); onClose(); }}
    >
      <div className="team-people-head">
        <h2 id="team-people-title">{t("team.page.peopleTitle", { team: teamName })}</h2>
        {owner && !inviting && <button type="button" className="share-secondary team-page-action" onClick={() => { setInviting(true); setIssued(null); }}>{t("team.page.invite")}</button>}
      </div>
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
      {people.length > SEARCH_FROM && (
        <input className="share-input team-people-search" type="search" aria-label={t("team.page.peopleSearch")} placeholder={t("team.page.peopleSearch")} value={query} onChange={(event) => setQuery(event.target.value)} />
      )}
      <ul className="team-page-rows team-people-list">
        {shown.map((person) => (
          <li key={person.id}>
            <span className="team-page-face" aria-hidden="true">{initialOf(person.name)}</span>
            <span className="team-page-name team-people-name">{person.label}</span>
            {owner && person.member && (() => {
              const member = person.member;
              return <MemberMenu member={member} onNewCode={() => void newLink(member)} onRemoveFromTeam={() => void remove(member)} />;
            })()}
          </li>
        ))}
      </ul>
      {needle && shown.length === 0 && <p className="team-page-quiet">{t("team.page.peopleNone")}</p>}
      {owner && members.length === 0 && !inviting && !issued && <p className="team-page-quiet">{t("team.page.noMembers")}</p>}
      {error && <p className="share-error" role="alert">{error}</p>}
      <div className="team-people-foot">
        <button type="button" className="share-primary" onClick={onClose}>{t("sharing.done")}</button>
      </div>
    </dialog>
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
