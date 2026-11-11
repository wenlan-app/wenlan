// SPDX-License-Identifier: AGPL-3.0-only
// Settings › Team: whether this computer is a team server, the address people
// reach it at, and the Spaces it shares with the team. People are added and
// removed in each Space's members box, where they are looked for.
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { SettingRow, Input, Field } from "../primitives";
import { SettingsGroup } from "../SettingsView";
import { TEAM_DOCS_URL, prefillTeamUrl, rememberTeamUrl, useTeamStatus } from "../../sharing/team";
import "./teamSection.css";

export default function TeamSection() {
  const { t } = useTranslation();
  const status = useTeamStatus();
  const [address, setAddress] = useState<string | null>(null);

  if (status.isPending) return <p className="team-off" role="status">{t("team.loading")}</p>;
  if (status.isError || !status.data) return <p className="team-error" role="alert">{t("team.loadFailed")}</p>;
  const { listening, spaces } = status.data;
  if (!listening) {
    return (
      <p className="team-off">
        {t("team.off")}{" "}
        <button type="button" onClick={() => { void Promise.resolve(openExternal(TEAM_DOCS_URL)).catch(() => undefined); }}>{t("team.docs")}</button>
      </p>
    );
  }

  const url = address ?? prefillTeamUrl(listening);
  return (
    <>
      <SettingsGroup className="mem-fade-up">
        <SettingRow title={t("team.serverOn", { addr: listening })} description="" control={null} />
        <div className="team-address">
          <Field label={t("team.urlLabel")} htmlFor="team-address" description={t("team.urlHint")}>
            <Input
              value={url}
              placeholder={t("team.urlPlaceholder")}
              spellCheck={false}
              onChange={(event) => { setAddress(event.target.value); rememberTeamUrl(event.target.value.trim()); }}
            />
          </Field>
        </div>
      </SettingsGroup>
      <SettingsGroup heading={t("team.hostedHeading")} className="mem-fade-up">
        {spaces.length === 0
          ? <SettingRow title={t("team.noneHosted")} description="" control={null} />
          : spaces.map((space) => <SettingRow key={space} title={space} description="" control={null} />)}
      </SettingsGroup>
      <p className="team-hint">{t("team.membersHint")}</p>
    </>
  );
}
