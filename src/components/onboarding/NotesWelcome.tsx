// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useRef, useState } from "react";
import { ChatsCircle, Eye, PencilSimpleLine } from "@phosphor-icons/react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { setExampleWikiActive } from "../../lib/exampleWiki/mode";
import { detectMcpClients, type McpClient } from "../../lib/tauri";
import { readingIsYes } from "../../lib/reading";
import { familyDisplayName } from "../../lib/agents";
import { familyOfClient } from "../connect/connectionState";
import { connectClient } from "../connect/connectClient";
import { pickWritePagesTool } from "../memory/askAi/WritePages";
import { PAGE_WRITER_CLIENTS, rememberLocalWriter, type WriterChoice } from "./writerChoice";
import { dragStripHeight } from "../../lib/windowChrome";
import { SourceTypeIcon } from "../memory/sources/SourceTypeIcon";
import { setStartIntent } from "./startIntent";
import "../memory/sources/source-access.css";
import "./notesWelcome.css";

/** The four ways in, as option rows: each row does its thing directly. */
const STARTS = [
  { id: "folder", icon: <SourceTypeIcon kind="folder" /> },
  { id: "chats", icon: <ChatsCircle aria-hidden="true" size={20} weight="regular" data-resource-icon="chats" /> },
  { id: "empty", icon: <PencilSimpleLine aria-hidden="true" size={20} weight="regular" data-resource-icon="empty" /> },
  { id: "example", icon: <Eye aria-hidden="true" size={20} weight="regular" data-resource-icon="example" /> },
] as const;

type Start = (typeof STARTS)[number]["id"];

interface NotesWelcomeProps {
  onComplete: () => void | Promise<void>;
  onContinueWithoutDaemon?: () => void;
}

/** The AI tool on this computer that writes pages, best first; null when
 *  none of them is installed. */
export function pageWriterTool(clients: readonly McpClient[] | undefined): McpClient | null {
  for (const type of PAGE_WRITER_CLIENTS) {
    const client = clients?.find((candidate) => candidate.client_type === type);
    if (client && readingIsYes(client.detected)) return client;
  }
  return null;
}

export function NotesWelcome({ onComplete, onContinueWithoutDaemon }: NotesWelcomeProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const writerId = useId();
  const clients = useQuery({ queryKey: ["mcp-clients"], queryFn: detectMcpClients, retry: false });
  const tool = pageWriterTool(clients.data);
  const toolName = tool ? familyDisplayName(familyOfClient(tool.client_type)) : null;
  // Picked once here. The tool found on this computer is the default; with
  // none found it is "not now", which only stores: nothing downloads unless
  // the person picks the local model and then presses to download it.
  const [picked, setPicked] = useState<WriterChoice | null>(null);
  const writer: WriterChoice = picked ?? (tool ? "tool" : "none");

  // Applies the choice. A tool that fails to connect never traps the person
  // here: home's first step still offers to connect it.
  // Only connecting a tool waits; the rest is immediate, so the app opens in
  // the same turn as before.
  function applyWriter(): Promise<void> | null {
    if (writer === "tool" && tool) {
      pickWritePagesTool(familyOfClient(tool.client_type));
      if (!readingIsYes(tool.already_configured)) {
        return connectClient(tool, "setup").then(
          () => undefined,
          (error: unknown) => console.warn("[welcome] connecting the AI tool failed", error),
        );
      }
    } else if (writer === "local") {
      rememberLocalWriter();
    }
    return null;
  }
  const inFlight = useRef(false);
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);

  async function start(way: Start) {
    // State updates may be batched, so guard before invoking the callback.
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(true);
    setFailed(false);
    // The example opens as its own wiki; the other ways happen once the app
    // is open (the folder picker, past chats, a new page).
    if (way === "example") setExampleWikiActive(true);
    else setStartIntent(way);
    try {
      const connecting = applyWriter();
      if (connecting) await connecting;
      await onComplete();
    } catch {
      if (way === "example") setExampleWikiActive(false);
      setFailed(true);
    } finally {
      inFlight.current = false;
      setPending(false);
    }
  }

  return (
    <div className="notes-welcome" data-testid="notes-welcome">
      <div
        data-tauri-drag-region
        style={{ height: dragStripHeight(), flexShrink: 0 }}
      />
      <main className="notes-welcome__main" aria-labelledby={titleId}>
        <div className="notes-welcome__content">
          <h1 id={titleId} className="notes-welcome__title">
            {t("setup.welcomeTitle")}
          </h1>
          <h2 id={writerId} className="notes-welcome__question">{t("setup.notesFirst.writer.question")}</h2>
          <div className="notes-welcome__writers" role="radiogroup" aria-labelledby={writerId}>
            {(["tool", "local", "none"] as const).map((choice) => choice === "tool" && !tool ? null : (
              <button
                key={choice}
                type="button"
                role="radio"
                aria-checked={writer === choice}
                className="source-add-option notes-welcome__writer"
                data-testid={`notes-welcome-writer-${choice}`}
                disabled={pending}
                onClick={() => setPicked(choice)}
              >
                <span className="notes-welcome__radio" aria-hidden="true" />
                <span className="source-add-option-copy">
                  <strong>{choice === "tool"
                    ? t("setup.notesFirst.writer.tool.title", { tool: toolName })
                    : t(`setup.notesFirst.writer.${choice}.title`)}</strong>
                  <span>{choice === "tool"
                    ? t(tool?.client_type === "claude_code" ? "setup.notesFirst.writer.tool.hintNextSession" : "setup.notesFirst.writer.tool.hint", { tool: toolName })
                    : t(`setup.notesFirst.writer.${choice}.hint`)}</span>
                </span>
              </button>
            ))}
          </div>
          <h2 className="notes-welcome__question">{t("setup.notesFirst.welcome.lead")}</h2>
          {failed && (
            <p className="notes-welcome__error" role="alert">
              {t("setup.notesFirst.openFailed")}
            </p>
          )}
          {failed && onContinueWithoutDaemon && (
            <button type="button" className="source-add-option" onClick={onContinueWithoutDaemon}>
              {t("setup.done.openWenlan")}
            </button>
          )}
          <div className="notes-welcome__starts">
            {STARTS.map(({ id, icon }) => (
              <button
                key={id}
                type="button"
                className="source-add-option"
                data-testid={`notes-welcome-${id}`}
                disabled={pending}
                aria-busy={pending}
                onClick={() => void start(id)}
              >
                {icon}
                <span className="source-add-option-copy">
                  <strong>{t(`setup.notesFirst.welcome.${id}.title`)}</strong>
                  <span>{t(`setup.notesFirst.welcome.${id}.hint`)}</span>
                </span>
              </button>
            ))}
          </div>
          <p className="notes-welcome__foot">{t("setup.notesFirst.welcome.foot")}</p>
        </div>
      </main>
    </div>
  );
}
