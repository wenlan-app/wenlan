// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team: membership and the server's plumbing only. Which team this
// computer is on and as whom; on the team's server, that it is on and the
// address put in invite links. Spaces and people live on the team's page.
// This is the only screen that says "server" or shows an address.
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { SettingRow, Input, Field } from "../primitives";
import { SettingsGroup } from "../SettingsView";
import { TEAM_DOCS_URL, prefillTeamUrl, rememberTeamUrl, useTeamStatus } from "../../sharing/team";
import { teamLabel, useShares } from "../../sharing/sharing";
import { teamGroups } from "../../team/TeamPage";
import "./teamSection.css";

export default function TeamSection() {
  const { t } = useTranslation();
  const status = useTeamStatus();
  const shares = useShares();
  const [address, setAddress] = useState<string | null>(null);

  if (status.isPending || shares.isPending) return <p className="team-off" role="status">{t("team.loading")}</p>;
  const listening = status.data?.listening ?? null;
  const joined = teamGroups(shares.data, undefined).filter((group) => !group.hosted);
  const hosting = Boolean(listening) || (status.data?.spaces.length ?? 0) > 0;
  if (!hosting && joined.length === 0) {
    return (
      <p className="team-off">
        {t("team.settings.none")}{" "}
        <button type="button" onClick={() => { void Promise.resolve(openExternal(TEAM_DOCS_URL)).catch(() => undefined); }}>{t("team.settings.howToStart")}</button>
      </p>
    );
  }

  const url = address ?? prefillTeamUrl(listening);
  return (
    <>
      {joined.map((group) => (
        <SettingsGroup key={group.key} heading={teamLabel(t, group.team)} className="mem-fade-up">
          <SettingRow
            title={t("team.settings.joinedAs", { name: group.shares[0]?.member_name ?? "" })}
            description={group.shares.some((share) => share.last?.paused) ? t("team.pausedRemoved") : t("team.settings.connected")}
            control={null}
          />
        </SettingsGroup>
      ))}
      {hosting && (
        <SettingsGroup heading={teamLabel(t, status.data?.team)} className="mem-fade-up">
          <SettingRow
            title={t("team.settings.thisServer")}
            description={listening ? t("team.settings.on", { addr: listening }) : t("team.settings.off")}
            control={null}
          />
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
    </>
  );
}
