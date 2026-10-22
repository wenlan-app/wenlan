// SPDX-License-Identifier: AGPL-3.0-only
import { useRef, useState } from "react";
import { Sparkle } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { AskAiDialog } from "./AskAiDialog";

interface AskAiEntryProps {
  readonly compact?: boolean;
  readonly onConnectTool?: () => void;
}

/** Sidebar footer entry that opens "What your AI can do". Always there, so a
 *  user can find the commands again after the getting-started card is gone. */
export function AskAiEntry({ compact = false, onConnectTool }: AskAiEntryProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };
  const label = t("askAi.entry");

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        data-testid="ask-ai-entry"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={label}
        title={label}
        onClick={() => setOpen(true)}
        className={`mem-activity-status${compact ? " mem-activity-status--compact" : ""}`}
      >
        <Sparkle aria-hidden="true" className="mem-activity-status-icon" size={22} weight="regular" />
        {!compact && <span aria-hidden="true" className="mem-activity-status-label">{label}</span>}
      </button>
      {open ? (
        <AskAiDialog
          onClose={close}
          onConnectTool={onConnectTool && (() => {
            setOpen(false);
            onConnectTool();
          })}
        />
      ) : null}
    </>
  );
}
