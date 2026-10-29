// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check } from "@phosphor-icons/react";
import "./turnIntoPages.css";

/** "Use another tool": which connected tool the button hands the pages to.
 *  Shown only with more than one tool connected; the pick is remembered.
 *  `align="end"` opens the menu leftward, for a picker at a row's right edge. */
export function ToolPicker({
  tools,
  current,
  onChoose,
  align = "start",
}: {
  readonly tools: readonly { family: string; name: string }[];
  readonly current: string;
  readonly onChoose: (family: string) => void;
  readonly align?: "start" | "end";
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", escape);
    };
  }, [open]);
  return (
    <div className={`turn-pages-picker${align === "end" ? " turn-pages-picker--end" : ""}`} ref={root} data-testid="tool-picker">
      <button
        type="button"
        className="turn-pages-link"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        {t("askAi.writePages.importCard.useAnother")}
      </button>
      {open ? (
        <ul className="turn-pages-picker-menu" id={menuId} role="menu" aria-label={t("askAi.writePages.importCard.useAnother")}>
          {tools.map((tool) => (
            <li key={tool.family} role="none">
              <button
                type="button"
                role="menuitemradio"
                aria-checked={tool.family === current}
                className="turn-pages-picker-item"
                onClick={() => {
                  onChoose(tool.family);
                  setOpen(false);
                }}
              >
                <span className="turn-pages-picker-check" aria-hidden="true">
                  {tool.family === current ? <Check weight="bold" /> : null}
                </span>
                {tool.name}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
