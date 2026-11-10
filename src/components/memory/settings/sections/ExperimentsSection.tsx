// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { useExperiment } from "../../../../lib/experiments";
import { SettingRow } from "../primitives";
import { SettingsGroup } from "../SettingsView";

/**
 * Screens still being tried, each off until turned on. A tab of its own, so
 * a switch that reshapes Home is found by name instead of at the bottom of
 * General. Turning one off hides a screen; the background runs either way.
 */
export default function ExperimentsSection() {
  const { t } = useTranslation();
  const [things, setThings] = useExperiment("things");
  return (
    <>
      <p className="settings-view-intro">{t("settings.general.experimentsIntro")}</p>
      <SettingsGroup className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SettingRow
          title={t("settings.general.experimentThingsTitle")}
          titleBadge={t("settings.general.experimentBadge")}
          description={t("settings.general.experimentThingsDescription")}
          enabled={things}
          onToggle={() => setThings(!things)}
        />
      </SettingsGroup>
    </>
  );
}
