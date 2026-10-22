// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AskAiDialog } from "./AskAiDialog";
import { AskAiHint } from "./AskAiHint";

/** On the connect flow's last step: the first thing to type in the tool just
 *  connected, and the way to the full list. */
export function ConnectedHint() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <>
      <AskAiHint
        task="brief"
        testId="ask-ai-hint-connected"
        lead={t("askAi.hints.connected")}
        onSeeAll={() => setOpen(true)}
      />
      {open ? <AskAiDialog onClose={() => setOpen(false)} /> : null}
    </>
  );
}
