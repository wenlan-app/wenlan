// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { setTeamPeople } from "../components/memory/sharing/teamPeople";
import { readTeamKey, teamMe } from "./team";
import { TeamAccessProvider, teamAccessFor, type TeamAccess } from "./teamAccess";

type State = { kind: "checking" } | { kind: "no-key" } | { kind: "dead" } | { kind: "ready"; access: TeamAccess };

/**
 * A team server's `/app`: the member's key, kept by the invite page's
 * Accept, is checked once; without one there is one sentence to follow.
 */
export function TeamMemberApp({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [state, setState] = useState<State>(() => (readTeamKey() ? { kind: "checking" } : { kind: "no-key" }));
  useEffect(() => {
    if (state.kind !== "checking") return;
    let live = true;
    void teamMe().then((me) => {
      if (!live) return;
      if (me) setTeamPeople(me.member_id, me.people ?? []);
      setState(me ? { kind: "ready", access: teamAccessFor(me) } : { kind: "dead" });
    });
    return () => {
      live = false;
    };
  }, [state.kind]);
  if (state.kind === "checking") return <div className="w-screen min-h-screen bg-[var(--bg-secondary)]" />;
  if (state.kind !== "ready") {
    return (
      <main className="w-screen min-h-screen bg-[var(--bg-secondary)] flex items-center justify-center px-4">
        <p data-testid="team-web-message" className="text-[var(--text-primary)] text-base text-center">
          {t(state.kind === "no-key" ? "teamWeb.openInvite" : "teamWeb.expired")}
        </p>
      </main>
    );
  }
  return <TeamAccessProvider value={state.access}>{children}</TeamAccessProvider>;
}
