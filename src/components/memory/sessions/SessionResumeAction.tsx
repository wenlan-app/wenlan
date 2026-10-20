// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { clipboardWrite, type AgentSession } from "../../../lib/tauri";
import { sessionResumeCommand, sessionToolLabel } from "../../../lib/sessionSource";

async function copyText(text: string): Promise<boolean> {
  try {
    await clipboardWrite(text);
    return true;
  } catch {
    // Outside Tauri (browser preview) the plugin is missing; try the web API.
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

interface SessionResumeActionProps {
  readonly session: AgentSession;
  /** The transcript file is still on this machine (the daemon checked). */
  readonly transcriptAvailable: boolean;
}

/**
 * What the session card offers for getting back into the conversation. This
 * is the one place that decides how: today a button that copies the tool's
 * resume command, never the transcript's location. To show the stored path
 * instead, swap this component's body and keep its props.
 *
 * No stored transcript (an import) shows nothing. A stored transcript that is
 * gone from disk (the tool prunes old ones) says so rather than offering a
 * command that cannot work.
 */
export function SessionResumeAction({ session, transcriptAvailable }: SessionResumeActionProps) {
  const { t } = useTranslation();
  const tool = sessionToolLabel(session.tool);
  const command = sessionResumeCommand(session.tool, session.external_id);
  const [outcome, setOutcome] = useState<"copied" | "failed" | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  if (!session.transcript_path) return null;
  if (!transcriptAvailable) {
    return <p className="session-card-muted">{t("sessionSource.transcriptGone", { tool })}</p>;
  }
  if (!command) return null;

  const copy = async () => {
    setOutcome(null);
    const ok = await copyText(command);
    if (mounted.current) setOutcome(ok ? "copied" : "failed");
  };

  return (
    <div className="session-card-resume">
      <button type="button" className="session-card-resume-button" onClick={() => { void copy(); }}>
        {t("sessionSource.continueIn", { tool })}
      </button>
      {outcome && (
        <p role="status" className="session-card-muted">
          {t(outcome === "copied" ? "sessionSource.copied" : "sessionSource.copyFailed")}
        </p>
      )}
    </div>
  );
}
