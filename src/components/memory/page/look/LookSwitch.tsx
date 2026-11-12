// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import "./look.css";

/** `Look | Text`, shown only on a page that has a look. */
export function LookSwitch({ showing, onChange }: {
  showing: "look" | "text";
  onChange: (mode: "look" | "text") => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="page-look-switch-row">
      <div className="page-look-switch" role="group" aria-label={t("pageLook.switchLabel")} data-testid="page-look-switch">
        <button type="button" aria-pressed={showing === "look"} onClick={() => onChange("look")}>{t("pageLook.look")}</button>
        <button type="button" aria-pressed={showing === "text"} onClick={() => onChange("text")}>{t("pageLook.text")}</button>
      </div>
    </div>
  );
}
