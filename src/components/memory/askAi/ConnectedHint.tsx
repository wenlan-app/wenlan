// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Sparkle } from "@phosphor-icons/react";
import { clientTypeFamily, resolveAgentActionName } from "../../../lib/agents";
import type { AgentConnection } from "../../../lib/tauri";
import { useLiveAgents } from "../../connect/connectionState";
import { AskAiDialog } from "./AskAiDialog";
import { AskInToolButton, frontDoorFor } from "./askInTool";
import { recentTool } from "./WritePages";
import "./askAiHint.css";

/** On the connect flow's last step: where to ask now that a tool is
 *  connected, with one button that opens that tool ready to ask, and the way
 *  to the full list. Plain text, so nothing here looks like a box to type in. */
export function ConnectedHint({ justConnected = [] }: {
  /** The tools this flow just set up, as client types or agent names. One of
   *  them is named first: it may not have reached the roster yet. */
  readonly justConnected?: readonly string[];
} = {}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const agents = useLiveAgents();
  const tool = pickTool(justConnected, agents.data ?? []) ?? (agents.isSuccess ? recentTool(agents.data) : null);
  if (!tool) return null;
  const command = frontDoorFor(tool.family);
  return (
    <>
      <div className="ask-ai-next" data-testid="ask-ai-hint-connected">
        <Sparkle aria-hidden="true" weight="regular" className="ask-ai-hint-icon" />
        <p className="ask-ai-next-text">
          {command
            ? t("askAi.hints.connectedDoor", { tool: tool.name, command })
            : t("askAi.hints.connectedPlain", { tool: tool.name })}
        </p>
        <div className="ask-ai-next-actions">
          <AskInToolButton
            family={tool.family}
            tool={tool.name}
            text={command ?? t("askAi.hints.connectedPrompt")}
            label={t("askAi.hints.openTool", { tool: tool.name })}
            testId="ask-ai-hint-open"
          />
          <button type="button" className="ask-ai-hint-more" onClick={() => setOpen(true)}>{t("askAi.hints.seeAll")}</button>
        </div>
      </div>
      {open ? <AskAiDialog onClose={() => setOpen(false)} /> : null}
    </>
  );
}

/** A tool the flow just set up, the ones with the plugin's front door first. */
function pickTool(ids: readonly string[], agents: readonly AgentConnection[]) {
  const families = ids.map((id) => clientTypeFamily(id) || id);
  const family = families.find((candidate) => frontDoorFor(candidate) !== null) ?? families[0];
  return family ? { family, name: resolveAgentActionName(family, [...agents]) } : null;
}
