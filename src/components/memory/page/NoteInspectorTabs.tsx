// SPDX-License-Identifier: AGPL-3.0-only
import { useRef, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import "../pages/noteTabs.css";
import "./NoteInspectorTabs.css";

export type NoteInspectorTab = "info" | "canvas" | "pdf";

interface NoteInspectorTabsProps {
  readonly active: NoteInspectorTab;
  readonly onSelect: (tab: NoteInspectorTab) => void;
  readonly disabled?: boolean;
  readonly idPrefix: string;
  readonly includePdf?: boolean;
}

export function NoteInspectorTabs({ active, onSelect, disabled = false, idPrefix, includePdf = false }: NoteInspectorTabsProps) {
  const { t } = useTranslation();
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const tabs: readonly NoteInspectorTab[] = includePdf ? ["info", "canvas", "pdf"] : ["info", "canvas"];
  const labels: Record<NoteInspectorTab, string> = {
    info: t("pageInspector.info"),
    canvas: t("pageCanvas.tabCanvas"),
    pdf: t("pdfReader.tab"),
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLButtonElement>, current: NoteInspectorTab) => {
    if (disabled) return;
    const index = tabs.indexOf(current);
    let nextIndex: number | null = null;
    if (event.key === "ArrowRight") nextIndex = (index + 1) % tabs.length;
    else if (event.key === "ArrowLeft") nextIndex = (index + tabs.length - 1) % tabs.length;
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = tabs.length - 1;
    if (nextIndex === null) return;
    event.preventDefault();
    tabRefs.current[nextIndex]?.focus();
  };

  return (
    <div aria-label={t("pageInspector.label")} className="note-inspector-tabs" role="tablist">
      {tabs.map((tab, index) => (
        <button
          aria-controls={`${idPrefix}-panel`}
          aria-selected={active === tab}
          className="note-tab"
          data-active={active === tab}
          disabled={disabled}
          id={`${idPrefix}-${tab}`}
          key={tab}
          onClick={() => onSelect(tab)}
          onKeyDown={(event) => onKeyDown(event, tab)}
          ref={(element) => { tabRefs.current[index] = element; }}
          role="tab"
          tabIndex={!disabled && active === tab ? 0 : -1}
          type="button"
        >
          <span>{labels[tab]}</span>
        </button>
      ))}
    </div>
  );
}

export default NoteInspectorTabs;
