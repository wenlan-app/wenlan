// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import LibrarySyncSettings from "./LibrarySyncSettings";

/** Keeping one library in step across the person's computers. */
export default function SyncSection() {
  const { t } = useTranslation();
  return (
    <>
      <p className="settings-view-intro">{t("settings.general.syncIntro")}</p>
      <LibrarySyncSettings />
    </>
  );
}
