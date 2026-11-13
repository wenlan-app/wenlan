// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team: membership and the server's plumbing only. Which team this
// computer is on and as whom; starting a team on this computer; on the team's
// server, that it is on (and turning it off) and the address put in invite
// links. Spaces and people live in the switch at the top left.
// This is the only screen that says "server" or shows an address.
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { teamFirewallAllow, teamStop, type SharedStatus } from "../../../../lib/tauri";
import { confirmAction } from "../../../../lib/confirmAction";
import { Button, SettingRow, Input, Field } from "../primitives";
import { SettingsGroup } from "../SettingsView";
import { TEAM_FIREWALL_KEY, TEAM_KEY, prefillTeamUrl, rememberTeamUrl, useTeamFirewall, useTeamStatus } from "../../sharing/team";
import { SHARES_KEY, teamLabel, teamPausedText, useShares } from "../../sharing/sharing";
import { teamGroups } from "../../team/TeamPage";
import { JoinSharedDialog } from "../../sharing/JoinSharedDialog";
import { StartTeamDialog } from "../../team/StartTeamDialog";
import { ServerSetupDialog, forgetOwnerKeyDraft } from "../../team/ServerSetupDialog";
import { useScope } from "../../team/scope";
import "./teamSection.css";

export default function TeamSection() {
  const { t } = useTranslation();
  const status = useTeamStatus();
  const shares = useShares();
  const [address, setAddress] = useState<string | null>(null);
  const [joining, setJoining] = useState(false);
  const [starting, setStarting] = useState(false);
  const [onServer, setOnServer] = useState(false);
  // The owner's link of a server set up from "Run it on a server", once
  // that server answers: the join window opens with it filled in.
  const [ownerLink, setOwnerLink] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [allowing, setAllowing] = useState(false);
  const [declined, setDeclined] = useState(false);
  const scope = useScope();
  const queryClient = useQueryClient();
  const firewall = useTeamFirewall(Boolean(!status.data?.remote && status.data?.listening));

  if (status.isPending || shares.isPending) return <p className="team-off" role="status">{t("team.loading")}</p>;
  // A team whose server is elsewhere and that this person owns: its status
  // is read from that server, and this computer hosts nothing.
  const here = status.data?.remote ? undefined : status.data;
  const listening = here?.listening ?? null;
  const joined = teamGroups(shares.data, status.data).filter((group) => !group.hosted);
  const hosting = Boolean(listening) || (here?.spaces.length ?? 0) > 0 || (here?.members.length ?? 0) > 0;
  const pausedOf = (shares: readonly SharedStatus[]) => {
    const paused = shares.find((share) => share.last?.paused);
    return paused ? teamPausedText(t, paused) : null;
  };
  // The dialogs sit outside the section's own text, whose link-like buttons
  // would otherwise style theirs.
  const joinDialog = joining && (
    <JoinSharedDialog
      only="code"
      code={ownerLink ?? undefined}
      onClose={() => { setJoining(false); setOwnerLink(null); }}
      onJoined={(kind, target) => {
        setJoining(false);
        if (ownerLink) forgetOwnerKeyDraft();
        setOwnerLink(null);
        // The new team is shown at once: the switch appears at the top left.
        if (kind === "team") scope.setScope(`team:${target}`);
      }}
    />
  );
  const startDialog = starting && (
    <StartTeamDialog
      status={here}
      onClose={() => setStarting(false)}
      onStarted={() => {
        setStarting(false);
        scope.setScope("hosted");
      }}
    />
  );
  const stop = async () => {
    const name = teamLabel(t, here?.team);
    const ok = await confirmAction(t("team.start.stopConfirm", { team: name }), { confirm: t("team.start.stop"), cancel: t("sharing.cancel") });
    if (!ok) return;
    setError(null);
    try {
      await teamStop();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: TEAM_KEY }),
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
      ]);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  };

  // Windows Firewall keeps teammates out of a team on this computer until
  // Windows' administrator lets them in. Only Windows answers; elsewhere the
  // row never shows.
  const wall = listening ? firewall.data : null;
  const allow = async () => {
    if (!wall) return;
    setAllowing(true);
    setDeclined(false);
    try {
      await teamFirewallAllow(wall);
    } catch {
      setDeclined(true);
    } finally {
      setAllowing(false);
      await queryClient.invalidateQueries({ queryKey: TEAM_FIREWALL_KEY });
    }
  };

  const joinRow = (title: string) => (
    <SettingRow
      title={title}
      description={t("team.settings.joinHint")}
      control={<Button onClick={() => setJoining(true)}>{t("team.settings.joinAction")}</Button>}
    />
  );

  // The only way into teams for someone on none: nothing else in the app
  // mentions them. Three ways, one row each: join, start one here, or run
  // one on a box nobody sits at.
  if (!hosting && joined.length === 0) {
    return (
      <>
        <SettingsGroup className="mem-fade-up">
          {joinRow(t("team.settings.joinTitle"))}
          <SettingRow
            title={t("team.start.title")}
            description={t("team.settings.startHint")}
            control={<Button onClick={() => setStarting(true)}>{t("team.settings.startAction")}</Button>}
          />
          <SettingRow
            title={t("team.settings.boxTitle")}
            description={t("team.settings.boxHint")}
            control={<Button onClick={() => setOnServer(true)}>{t("team.settings.boxAction")}</Button>}
          />
        </SettingsGroup>
        {joinDialog}
        {startDialog}
        {onServer && (
          <ServerSetupDialog
            onClose={() => setOnServer(false)}
            onPaste={() => { setOnServer(false); setJoining(true); }}
            onReady={(link) => { setOnServer(false); setOwnerLink(link); setJoining(true); }}
          />
        )}
      </>
    );
  }

  const url = address ?? (prefillTeamUrl(listening) || here?.url || "");
  return (
    <>
      {error && <p className="team-error" role="alert">{error}</p>}
      {joined.map((group) => (
        <SettingsGroup key={group.key} heading={teamLabel(t, group.team)} className="mem-fade-up">
          <SettingRow
            title={group.owner
              ? t("team.settings.ownerElsewhere")
              : t("team.settings.joinedAs", { name: group.shares[0]?.member_name ?? "" })}
            description={pausedOf(group.shares) ?? t("team.settings.connected")}
            control={null}
          />
        </SettingsGroup>
      ))}
      {!hosting && <SettingsGroup className="mem-fade-up">{joinRow(t("team.settings.joinAnother"))}</SettingsGroup>}
      {hosting && (
        <SettingsGroup heading={teamLabel(t, here?.team)} className="mem-fade-up">
          <SettingRow
            title={t("team.settings.thisServer")}
            description={listening ? t("team.settings.on", { addr: here?.url || listening }) : t("team.settings.off")}
            control={listening
              ? <Button onClick={() => void stop()}>{t("team.start.stop")}</Button>
              : <Button onClick={() => setStarting(true)}>{t("team.start.again")}</Button>}
          />
          {wall && !wall.open && (
            <SettingRow
              title={t("team.firewall.blockedTitle")}
              description={declined
                ? t("team.firewall.declined")
                : wall.public_network ? t("team.firewall.blockedHintPublic") : t("team.firewall.blockedHint")}
              control={<Button onClick={() => void allow()} disabled={allowing}>{allowing ? t("team.firewall.allowing") : t("team.firewall.allow")}</Button>}
            />
          )}
          {listening && (
            <div className="team-address">
              <Field label={t("team.settings.linkAddress")} htmlFor="team-address" description={t("team.urlHint")}>
                <Input
                  id="team-address"
                  value={url}
                  placeholder={t("team.urlPlaceholder")}
                  spellCheck={false}
                  onChange={(event) => { setAddress(event.target.value); rememberTeamUrl(event.target.value.trim()); }}
                />
              </Field>
            </div>
          )}
        </SettingsGroup>
      )}
      {joinDialog}
      {startDialog}
    </>
  );
}
