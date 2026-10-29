// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { useLiveAgents } from "../../connect/connectionState";
import { AskInToolButton, frontDoorFor } from "./askInTool";
import { recentTool } from "./WritePages";
import "./askAiHint.css";

/** Above a blank new page, while the user is still getting started: questions
 *  go to the AI tool, not into a page. A new user once wrote their question
 *  as a page and waited for an answer. Gone once they type. */
export function AskInsteadHint({ blank }: { readonly blank: boolean }) {
  const { t } = useTranslation();
  const agents = useLiveAgents(undefined, blank);
  const tool = agents.isSuccess ? recentTool(agents.data) : null;
  if (!blank || !tool) return null;
  const command = frontDoorFor(tool.family);
  return (
    <div className="ask-ai-next ask-ai-next--quiet" data-testid="ask-instead-hint">
      <p className="ask-ai-next-text">
        {command
          ? t("askAi.hints.askInsteadDoor", { tool: tool.name, command })
          : t("askAi.hints.askInsteadPlain", { tool: tool.name })}
      </p>
      <AskInToolButton
        family={tool.family}
        tool={tool.name}
        text={command ?? t("askAi.hints.connectedPrompt")}
        label={t("askAi.hints.openTool", { tool: tool.name })}
        className="ask-in-tool ask-in-tool--link"
        testId="ask-instead-open"
      />
    </div>
  );
}
