// SPDX-License-Identifier: AGPL-3.0-only
import { SidebarSimple } from "@phosphor-icons/react";
import { useContext, type Ref } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { WorkspaceDocumentToolsHostContext } from "../navigation/WorkspacePaneHost";

export function InfoToggle({ open, onClick, ref }: { open: boolean; onClick: () => void; ref?: Ref<HTMLButtonElement> }) {
  const { t } = useTranslation();
  const documentToolsHost = useContext(WorkspaceDocumentToolsHostContext);
  const toggle = <button ref={ref} type="button" className="mem-icon-action workspace-panel-toggle" aria-label={t("knowledgeContext.memoryPanel")}
    title={t("knowledgeContext.memoryPanel")} aria-expanded={open} onClick={(event) => {
      // Safari does not focus pointer-clicked buttons; the drawer needs a
      // concrete return target when the reader closes it with Escape.
      event.currentTarget.focus({ preventScroll: true });
      onClick();
    }}>
    <SidebarSimple aria-hidden="true" size={18} style={{ transform: "scaleX(-1)" }} />
  </button>;
  return documentToolsHost ? createPortal(toggle, documentToolsHost) : toggle;
}
