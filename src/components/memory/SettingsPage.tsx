// SPDX-License-Identifier: AGPL-3.0-only
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { useTranslation } from "react-i18next";
import type { SettingsSection } from "./settings/SettingsSidebar";
import { SETTINGS_GROUPS } from "./settings/SettingsSidebar";
import GeneralSection from "./settings/sections/GeneralSection";
import AppearanceSection from "./settings/sections/AppearanceSection";
import SourcesSection from "./settings/sections/SourcesSection";
import AgentsSection from "./settings/sections/AgentsSection";
import IntelligenceSection from "./settings/sections/IntelligenceSection";
import DiagnosticsSection from "./settings/sections/DiagnosticsSection";
import SyncSection from "./settings/sections/SyncSection";
import ExperimentsSection from "./settings/sections/ExperimentsSection";
import { SettingsView } from "./settings/SettingsView";
import "./settings/settingsControls.css";

interface SettingsPageProps {
  /** Which group to display. Driven by the Settings sidebar in Main.tsx. */
  section?: SettingsSection;
  onBack: () => void;
  onSetupAgent?: () => void;
  onOpenConnections?: () => void;
  onOpenIntelligence?: () => void;
  onOpenActivity?: () => void;
  currentSpace?: string;
  /** Required: SourcesSection's only affordance is the import button, so a
   *  caller that omits this would render a card whose entire purpose is a
   *  button that does nothing. Keep it required — that is a compile error,
   *  which is the point. */
  onImport: () => void;
  /** Opens the list of files and links the person added. */
  onOpenLibrary?: () => void;
}

export default function SettingsPage({
  section = "general",
  onBack,
  onSetupAgent,
  onOpenConnections,
  onOpenIntelligence,
  onOpenActivity,
  onImport,
  onOpenLibrary,
  currentSpace,
}: SettingsPageProps) {
  const { t } = useTranslation();

  const activeGroup = SETTINGS_GROUPS.find((g) => g.id === section);
  const content = (
    <>
      {section === "general" && <GeneralSection />}
      {section === "appearance" && <AppearanceSection />}
      {section === "sources" && <SourcesSection onImport={onImport} onOpenLibrary={onOpenLibrary} />}
      {section === "agents" && <AgentsSection onSetupAgent={onSetupAgent} currentSpace={currentSpace} />}
      {section === "sync" && <SyncSection />}
      {section === "intelligence" && <IntelligenceSection delay={0} />}
      {section === "experiments" && <ExperimentsSection />}
      {section === "diagnostics" && (
        <DiagnosticsSection
          onSetupAgent={onSetupAgent}
          onOpenConnections={onOpenConnections}
          onOpenIntelligence={onOpenIntelligence}
          onOpenActivity={onOpenActivity}
        />
      )}
    </>
  );
  const backButton = (
    <WorkspaceBackButton onClick={onBack} aria-label={t("main.back")} title={t("main.back")} className="p-1.5 -ml-1.5 rounded-md transition-colors duration-150 hover:bg-[var(--mem-hover)]" style={{ color: "var(--mem-text-tertiary)", background: "none", border: "none", cursor: "pointer", lineHeight: 0 }}>
      <svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="M19 12H5M12 19l-7-7 7-7" /></svg>
    </WorkspaceBackButton>
  );

  return (
    <SettingsView
      title={activeGroup ? t(activeGroup.labelKey) : t("settings.title")}
      backButton={backButton}
      wide={section === "appearance"}
    >
      {content}
    </SettingsView>
  );
}
