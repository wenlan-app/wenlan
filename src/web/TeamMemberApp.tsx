// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { setTeamPeople } from "../components/memory/sharing/teamPeople";
import { DevicePage } from "./DevicePage";
import { SignInPage } from "./SignInPage";
import { peekAfter, readTeamKey, rememberAfter, takeAfter, teamMe, teamProviders, type TeamMe } from "./team";
import { TeamAccessProvider, teamAccessFor, type TeamAccess } from "./teamAccess";

type State =
  | { kind: "checking" }
  | { kind: "no-key" }
  | { kind: "sign-in"; team: string | null; providers: string[] }
  | { kind: "dead" }
  | { kind: "ready"; access: TeamAccess; me: TeamMe };

const DEVICE_PATH = "/app/device";

function onDevicePage(): boolean {
  return window.location.pathname.replace(/\/+$/, "") === DEVICE_PATH;
}

/**
 * A team server's `/app`: who this browser is, by its session cookie or the
 * member's key (kept by the invite page's Accept), is checked once. With
 * neither, a server that signs people in shows its sign-in page, and one
 * that does not shows one sentence to follow.
 */
export function TeamMemberApp({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const [state, setState] = useState<State>({ kind: "checking" });
  // What was left to do on `/app/device` before signing in again, read once.
  const [deviceAfter] = useState(() => (onDevicePage() ? takeAfter() : null));
  useEffect(() => {
    if (state.kind !== "checking") return;
    let live = true;
    const hadKey = readTeamKey() !== null;
    void (async () => {
      const me = await teamMe();
      if (!live) return;
      if (me) {
        // Back from signing in (again) on the way to allowing an AI tool.
        if (!onDevicePage() && peekAfter()?.kind === "device") {
          window.location.replace(DEVICE_PATH);
          return;
        }
        setTeamPeople(me.member_id, me.people ?? []);
        setState({ kind: "ready", access: teamAccessFor(me), me });
        return;
      }
      // A key that no longer works keeps its own sentence.
      if (hadKey) {
        setState({ kind: "dead" });
        return;
      }
      const offered = await teamProviders();
      if (!live) return;
      // Signing in from `/app/device` lands on `/app`: come back here after.
      if (onDevicePage() && offered.providers.length > 0) rememberAfter({ kind: "device" });
      setState(offered.providers.length > 0 ? { kind: "sign-in", team: offered.team, providers: offered.providers } : { kind: "no-key" });
    })();
    return () => {
      live = false;
    };
  }, [state.kind]);
  if (state.kind === "checking") return <div className="w-screen min-h-screen bg-[var(--bg-secondary)]" />;
  if (state.kind === "sign-in") return <SignInPage team={state.team} providers={state.providers} />;
  if (state.kind !== "ready") {
    return (
      <main className="w-screen min-h-screen bg-[var(--bg-secondary)] flex items-center justify-center px-4">
        <p data-testid="team-web-message" className="text-[var(--text-primary)] text-base text-center">
          {t(state.kind === "no-key" ? "teamWeb.openInvite" : "teamWeb.expired")}
        </p>
      </main>
    );
  }
  if (onDevicePage()) {
    return <DevicePage me={state.me} initialCode={deviceAfter?.kind === "device" ? deviceAfter.code : undefined} />;
  }
  return <TeamAccessProvider value={state.access}>{children}</TeamAccessProvider>;
}
