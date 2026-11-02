// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type MouseEvent } from "react";
import { useTranslation } from "react-i18next";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import SourcesList from "../../sources/SourcesSection";
import { ImportFlow } from "../../../ChatImport/ImportFlow";
import { Button, Card, SectionHeader } from "../primitives";
import { SettingsHelp } from "../SettingsHelp";

const CHATGPT_EXPORT_HELP_URL = "https://help.openai.com/en/articles/7260999-exporting-your-chatgpt-history-and-data";
const CLAUDE_EXPORT_HELP_URL = "https://support.claude.com/en/articles/9450526-export-your-claude-data";

export default function SourcesSection({ onImport, onOpenLibrary }: { onImport: () => void; onOpenLibrary?: () => void }) {
  const { t } = useTranslation();
  const [helpLinkFailed, setHelpLinkFailed] = useState(false);
  const openHelpLink = (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault();
    const href = event.currentTarget.href;
    setHelpLinkFailed(false);
    void openExternal(href).catch(() => setHelpLinkFailed(true));
  };
  return (
    <>
      {/* ── Sources ────────────────────────────────────────────── */}
      <section className="mem-fade-up" style={{ animationDelay: "0ms" }}>
        <SectionHeader label={t("settings.sources.connectedTitle")} />
        <SourcesList />
      </section>

      {/* What was added has no place in the navigation: a source is what a
          sentence cites, read from the page that cites it. The full list of
          added files and links stays one step away, here. */}
      {onOpenLibrary && <section className="mem-fade-up" style={{ animationDelay: "15ms" }}>
        <SectionHeader label={t("settings.sources.libraryTitle")} />
        <Card padding="rows">
          <div className="px-5 py-4">
            <div className="settings-import-row flex items-center justify-between gap-4">
              <div className="min-w-0">
                <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)", marginTop: "2px", lineHeight: "1.5" }}>
                  {t("settings.sources.libraryDescription")}
                </p>
              </div>
              <Button variant="secondary" size="sm" className="shrink-0" onClick={onOpenLibrary}>
                {t("settings.sources.openLibrary")}
              </Button>
            </div>
          </div>
        </Card>
      </section>}

      {/* The group title appears once, matching the other settings groups. */}
      <section className="mem-fade-up" style={{ animationDelay: "30ms" }}>
        <SectionHeader label={t("settings.sources.importMemoriesTitle")} />
        <Card padding="rows">
          <div className="px-5 py-4">
            <div className="settings-import-row flex items-center justify-between gap-4">
              <div className="min-w-0">
                <p style={{ fontFamily: "var(--mem-font-body)", fontSize: "var(--mem-text-sm)", color: "var(--mem-text-secondary)", marginTop: "2px", lineHeight: "1.5" }}>
                  {t("settings.sources.importMemoriesDescription")}
                </p>
              </div>
              <Button variant="secondary" size="sm" className="shrink-0" onClick={onImport}>
                {t("settings.sources.pasteOrImport")}
              </Button>
            </div>
          </div>
        </Card>
      </section>

      {/* ── Import Chat History ──────────────────────────────── */}
      <section className="mem-fade-up" style={{ animationDelay: "60ms" }}>
        <SectionHeader label={t("settings.sources.importChatHistoryTitle")} />
        <Card padding="card">
          <p className="settings-chat-import-description">
            {t("settings.sources.importChatHistoryDescription")}
          </p>
          <div className="settings-chat-export-guide">
            <SectionHeader
              label={t("settings.sources.howToExport")}
              help={(
                <SettingsHelp
                  title={t("settings.sources.howToExport")}
                  details={t("settings.sources.chatExportHelp")}
                />
              )}
            />
            <div className="settings-chat-export-rows">
              <div className="settings-chat-export-row">
                <span className="settings-chat-export-provider">{t("settings.sources.chatgptName")}</span>
                <span className="settings-chat-export-steps">{t("settings.sources.chatgptExportSteps")}</span>
                <a className="settings-chat-export-link" href={CHATGPT_EXPORT_HELP_URL} target="_blank" rel="noopener noreferrer" onClick={openHelpLink}>
                  {t("settings.sources.chatgptExportHelpLink")}
                </a>
              </div>
              <div className="settings-chat-export-row">
                <span className="settings-chat-export-provider">{t("settings.sources.claudeName")}</span>
                <span className="settings-chat-export-steps">{t("settings.sources.claudeExportSteps")}</span>
                <a className="settings-chat-export-link" href={CLAUDE_EXPORT_HELP_URL} target="_blank" rel="noopener noreferrer" onClick={openHelpLink}>
                  {t("settings.sources.claudeExportHelpLink")}
                </a>
              </div>
            </div>
            {helpLinkFailed && <p className="settings-chat-export-error" role="alert">{t("settings.sources.helpLinkError")}</p>}
          </div>
          <ImportFlow />
        </Card>
      </section>
    </>
  );
}
