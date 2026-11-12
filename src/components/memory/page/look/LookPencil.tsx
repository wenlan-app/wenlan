// SPDX-License-Identifier: AGPL-3.0-only
import { PencilSimple } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import "./look.css";

/**
 * The pencil in the page's top bar, shown only on a page with a design
 * (UX-LOOK-2). Pressed, the page shows its text to edit; pressed again, the
 * design comes back with the edits in it. After the design failed to show,
 * pressing it tries the design again.
 */
export function LookPencil({ editing, onChange, label }: {
  /** The text is showing instead of the design. */
  editing: boolean;
  /** Defaults to "Edit page"; home says what it shows instead. */
  label?: string;
  onChange: (mode: "look" | "text") => void;
}) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      className="mem-icon-action workspace-panel-toggle page-look-pencil"
      data-testid="page-look-pencil"
      aria-label={label ?? t("pageLook.editPage")}
      title={label ?? t("pageLook.editPage")}
      aria-pressed={editing}
      onClick={(event) => {
        // WebKit pointer clicks do not focus buttons; keep a stable return target.
        event.currentTarget.focus({ preventScroll: true });
        onChange(editing ? "look" : "text");
      }}
    >
      <PencilSimple aria-hidden="true" size={18} />
    </button>
  );
}
