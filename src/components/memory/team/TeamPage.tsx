// SPDX-License-Identifier: AGPL-3.0-only
// The team's own page, opened from "Team" in the left navigation: who is on
// the team, which Spaces it has and who can use each, and what changed in
// them lately. On the computer that hosts the team it also invites people
// and opens Spaces to them; on a member's computer it only shows.
import { useState, type FormEvent } from "react";
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
  teamSetEveryone,
  teamUpdateMember,
  type SharedStatus,
  type TeamAccess,
  type TeamMember,
  type TeamSpaceAccess,
  type TeamStatus,
} from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";
import { SpaceRecentChanges } from "../space-detail/SpaceRecentChanges";
import { SHARES_KEY, sharedWriter, useShares } from "../sharing/sharing";
import { TEAM_KEY, activeMembers, hostName, prefillTeamUrl, rememberTeamUrl, sameName, useTeamStatus } from "../sharing/team";
import { AccessSelect, EveryoneSelect, IssuedCode, MemberMenu, codeOf, errorText, everyoneOf, type EveryoneAccess } from "../sharing/TeamMembers";
import "../sharing/sharing.css";
import "../space-detail/space-brief.css";
import "../space-detail/space-recent-changes.css";
import "./team-page.css";

/** One team this computer is part of: the one it hosts, or one per server
 *  it syncs Spaces through. */
export type TeamGroup = {
  readonly key: string;
  /** The server's address as people read it; `null` for this computer. */
  readonly host: string | null;
  readonly shares: SharedStatus[];
};

/** The teams in `shares`, the hosted one first. A computer that hosts a
 *  team with people but no Space yet still has it. */
export function teamGroups(shares: readonly SharedStatus[] | undefined, status: TeamStatus | undefined): TeamGroup[] {
  const hosted: SharedStatus[] = [];
  const byHost = new Map<string, SharedStatus[]>();
  for (const share of shares ?? []) {
    if (share.kind !== "space") continue;
    if (share.hosted) hosted.push(share);
    else if (share.host) byHost.set(share.host, [...(byHost.get(share.host) ?? []), share]);
  }
  const groups: TeamGroup[] = [];
  if (hosted.length > 0 || (status?.members?.length ?? 0) > 0 || (status?.spaces?.length ?? 0) > 0) {
    groups.push({ key: "hosted", host: null, shares: hosted });
  }
  for (const [url, list] of byHost) groups.push({ key: url, host: hostName(url), shares: list });
  return groups;
}

/** Whether the left navigation shows "Team": this computer is part of one. */
export function useInTeam(): boolean {
  const shares = useShares();
  const status = useTeamStatus();
  return teamGroups(shares.data, status.data).length > 0;
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
        <section key={group.key} className="team-page-team" aria-label={group.host ? t("team.groupTeam", { host: group.host }) : t("team.groupHosted")}>
          <h1 className="team-page-title">{group.host ? t("team.groupTeam", { host: group.host }) : t("team.groupHosted")}</h1>
          {group.host === null
            ? <HostedTeam group={group} status={status.data} shares={shares.data} onOpenSpace={onOpenSpace} onOpenPage={onOpenPage} />
            : <JoinedTeam group={group} shares={shares.data} onOpenSpace={onOpenSpace} onOpenPage={onOpenPage} />}
        </section>
      ))}
    </div>
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
          <span className="space-recent-changes-meta team-page-waiting">{t("team.page.waiting")}</span>
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

function HostedTeam({ group, status, shares, onOpenSpace, onOpenPage }: {
  readonly group: TeamGroup;
  readonly status: TeamStatus | undefined;
  readonly shares: readonly SharedStatus[] | undefined;
  readonly onOpenSpace: (space: string) => void;
  readonly onOpenPage: (id: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const [issued, setIssued] = useState<{ name: string; code: string } | null>(null);
  const [inviting, setInviting] = useState(false);
  const [sharing, setSharing] = useState("");
  const members = activeMembers(status?.members ?? []);
  const hostedSpaces = status?.spaces ?? group.shares.map((share) => share.target);
  const everyone = status?.everyone ?? [];
  const url = prefillTeamUrl(status?.listening);
  const allSpaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  const shareable = (allSpaces.data ?? [])
    .map((space) => space.name)
    .filter((name) => !(shares ?? []).some((share) => share.kind === "space" && share.target === name));
  const waiting = useWaitingRows(shares, group.shares, onOpenPage);

  const run = async (action: () => Promise<void>) => {
    setError(null);
    try {
      await action();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
      ]);
    } catch (caught) {
      setError(errorText(caught));
    }
  };
  const spacesOf = (member: TeamMember) => new Set([
    ...member.spaces.map((entry) => entry.space),
    ...everyone.map((entry) => entry.space),
  ]).size;
  const newCode = (member: TeamMember) => run(async () => {
    const key = await teamNewMemberKey(member.id, url.trim() || undefined);
    setIssued({ name: member.name, code: codeOf(key) });
  });
  const removeFromTeam = async (member: TeamMember) => {
    const ok = await confirmAction(t("team.removeConfirm", { name: member.name }), { confirm: t("team.menuRemoveFromTeam"), cancel: t("sharing.cancel") });
    if (ok) await run(async () => { await teamRemoveMember(member.id); });
  };
  const changeEveryone = (space: string, next: EveryoneAccess) => run(async () => { await teamSetEveryone(space, next); });
  const share = async (event: FormEvent) => {
    event.preventDefault();
    if (!sharing) return;
    await run(async () => {
      await teamHostSpace(sharing);
      setSharing("");
    });
  };

  return (
    <>
      <div className="team-page-block">
        <SectionHeading
          title={t("team.page.members")}
          action={!inviting && <button type="button" className="share-secondary team-page-action" onClick={() => { setInviting(true); setIssued(null); }}>{t("team.page.invite")}</button>}
        />
        {inviting && (
          <InviteForm
            members={members}
            spaces={hostedSpaces}
            everyone={everyone}
            initialUrl={url}
            onCancel={() => setInviting(false)}
            onIssued={(next) => { setInviting(false); setIssued(next); }}
            onDone={() => setInviting(false)}
            run={run}
          />
        )}
        {issued && <IssuedCode name={issued.name} code={issued.code} onDone={() => setIssued(null)} />}
        {members.length === 0 && !inviting
          ? <p className="team-page-quiet">{t("team.page.noMembers")}</p>
          : (
            <ul className="team-page-rows">
              {members.map((member) => (
                <li key={member.id}>
                  <span className="team-page-name">{member.name}</span>
                  <span className="team-page-meta">{t("team.page.spaceCount", { count: spacesOf(member) })}</span>
                  <MemberMenu member={member} onNewCode={() => void newCode(member)} onRemoveFromTeam={() => void removeFromTeam(member)} />
                </li>
              ))}
            </ul>
          )}
      </div>

      <div className="team-page-block">
        <SectionHeading title={t("team.page.spaces")} />
        {hostedSpaces.length === 0
          ? <p className="team-page-quiet">{t("team.noneHosted")}</p>
          : (
            <ul className="team-page-rows">
              {hostedSpaces.map((space) => {
                const open = everyoneOf(everyone, space);
                // People added by name, unless everyone has the same access anyway.
                const names = members
                  .filter((member) => member.spaces.some((entry) => entry.space === space && entry.access !== open))
                  .map((member) => member.name);
                return (
                  <li key={space}>
                    <button type="button" className="team-page-name team-page-open" onClick={() => onOpenSpace(space)}>{space}</button>
                    <span className="team-page-meta">{names.length > 0 ? names.join(t("team.page.nameSeparator")) : open === "nobody" ? t("team.page.nobodyYet") : ""}</span>
                    <EveryoneSelect label={t("team.page.whoCanUse", { space })} value={open} onChange={(next) => void changeEveryone(space, next)} />
                  </li>
                );
              })}
            </ul>
          )}
        {shareable.length > 0 && (
          <form className="team-page-share" onSubmit={(event) => void share(event)}>
            <select className="share-input" aria-label={t("team.page.shareSpace")} value={sharing} onChange={(event) => setSharing(event.target.value)}>
              <option value="">{t("team.page.shareSpace")}</option>
              {shareable.map((name) => <option key={name} value={name}>{name}</option>)}
            </select>
            <button type="submit" className="share-secondary" disabled={!sharing}>{t("team.page.shareAction")}</button>
          </form>
        )}
      </div>

      {error && <p className="share-error team-page-error" role="alert">{error}</p>}
      <SpaceRecentChanges space={hostedSpaces} shares={shares} waiting={waiting} />
    </>
  );
}

/** Invite one person: their name and the Spaces they can use. Spaces open
 *  to everyone come with it. Someone already on the team keeps their code
 *  and gains the Spaces. */
function InviteForm({ members, spaces, everyone, initialUrl, onCancel, onIssued, onDone, run }: {
  readonly members: readonly TeamMember[];
  readonly spaces: readonly string[];
  readonly everyone: readonly TeamSpaceAccess[];
  readonly initialUrl: string;
  readonly onCancel: () => void;
  readonly onIssued: (issued: { name: string; code: string }) => void;
  readonly onDone: () => void;
  readonly run: (action: () => Promise<void>) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [address, setAddress] = useState(initialUrl);
  const [picked, setPicked] = useState<Record<string, TeamAccess | undefined>>({});
  const [pending, setPending] = useState(false);
  const forEveryone = (space: string) => everyoneOf(everyone, space) !== "nobody";

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const typed = name.trim();
    if (!typed || pending) return;
    const chosen: TeamSpaceAccess[] = spaces
      .filter((space) => !forEveryone(space) && picked[space])
      .map((space) => ({ space, access: picked[space] as TeamAccess }));
    setPending(true);
    await run(async () => {
      const existing = members.find((member) => sameName(member.name, typed));
      if (existing) {
        const kept = existing.spaces.filter((entry) => !chosen.some((next) => next.space === entry.space));
        await teamUpdateMember(existing.id, { spaces: [...kept, ...chosen] });
        onDone();
        return;
      }
      const url = address.trim();
      const key = await teamAddMember(typed, chosen, url || undefined);
      if (url) rememberTeamUrl(url);
      onIssued({ name: key.member.name, code: codeOf(key) });
    });
    setPending(false);
  };

  return (
    <form className="team-invite" onSubmit={(event) => void submit(event)}>
      <input className="share-input" aria-label={t("team.name")} placeholder={t("team.namePlaceholder")} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} autoFocus />
      {spaces.length > 0 && (
        <fieldset className="team-invite-spaces">
          <legend className="share-label">{t("team.page.inviteSpaces")}</legend>
          {spaces.map((space) => {
            const open = forEveryone(space);
            const access = picked[space];
            return (
              <div key={space} className="team-invite-space">
                <label>
                  <input
                    type="checkbox"
                    checked={open || Boolean(access)}
                    disabled={open || pending}
                    onChange={(event) => setPicked({ ...picked, [space]: event.target.checked ? "write" : undefined })}
                  />
                  <span>{space}</span>
                </label>
                {open
                  ? <span className="team-page-meta">{t("team.page.inviteEveryone")}</span>
                  : access && <AccessSelect label={t("team.accessOf", { name: space })} value={access} disabled={pending} onChange={(next) => setPicked({ ...picked, [space]: next })} />}
              </div>
            );
          })}
        </fieldset>
      )}
      {!initialUrl && (
        <label className="share-field">
          <span className="share-label">{t("team.urlLabel")}</span>
          <input className="share-input" value={address} onChange={(event) => setAddress(event.target.value)} placeholder={t("team.urlPlaceholder")} spellCheck={false} />
        </label>
      )}
      <div className="team-invite-actions">
        <button type="submit" className="share-primary" disabled={!name.trim() || pending}>{t("team.page.inviteAction")}</button>
        <button type="button" className="share-link" onClick={onCancel}>{t("sharing.cancel")}</button>
      </div>
    </form>
  );
}

function JoinedTeam({ group, shares, onOpenSpace, onOpenPage }: {
  readonly group: TeamGroup;
  readonly shares: readonly SharedStatus[] | undefined;
  readonly onOpenSpace: (space: string) => void;
  readonly onOpenPage: (id: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const waiting = useWaitingRows(shares, group.shares, onOpenPage);
  const me = group.shares[0]?.member_id;
  // Everyone in any of the team's Spaces, by person.
  const people = new Map<string, { name: string; spaces: number }>();
  for (const share of group.shares) {
    for (const member of share.members) {
      const seen = people.get(member.id);
      people.set(member.id, { name: member.name, spaces: (seen?.spaces ?? 0) + 1 });
    }
  }
  const leave = async () => {
    const ok = await confirmAction(t("team.page.leaveConfirm", { host: group.host ?? "" }), { confirm: t("team.page.leave"), cancel: t("sharing.cancel") });
    if (!ok) return;
    setError(null);
    try {
      for (const share of group.shares) await stopSharingSpace(share.target);
      await queryClient.invalidateQueries({ queryKey: SHARES_KEY });
    } catch (caught) {
      setError(errorText(caught));
    }
  };

  return (
    <>
      <div className="team-page-block">
        <SectionHeading title={t("team.page.members")} />
        <ul className="team-page-rows">
          {[...people].map(([id, person]) => (
            <li key={id}>
              <span className="team-page-name">{id === me ? t("team.page.you", { name: person.name }) : person.name}</span>
              <span className="team-page-meta">{t("team.page.spaceCount", { count: person.spaces })}</span>
            </li>
          ))}
        </ul>
      </div>
      <div className="team-page-block">
        <SectionHeading title={t("team.page.spaces")} />
        <ul className="team-page-rows">
          {group.shares.map((share) => (
            <li key={share.target}>
              <button type="button" className="team-page-name team-page-open" onClick={() => onOpenSpace(share.target)}>{share.target}</button>
              <span className="team-page-meta">{t(share.read_only ? "team.readOnly" : "team.canEdit")}</span>
            </li>
          ))}
        </ul>
        <p className="team-page-quiet">{t("team.page.joinedHint")}</p>
      </div>
      {error && <p className="share-error team-page-error" role="alert">{error}</p>}
      <SpaceRecentChanges space={group.shares.map((share) => share.target)} shares={shares} waiting={waiting} />
      <div className="team-page-leave">
        <button type="button" className="share-link" onClick={() => void leave()}>{t("team.page.leave")}</button>
      </div>
    </>
  );
}
