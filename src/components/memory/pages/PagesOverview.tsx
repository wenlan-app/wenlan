import { useState } from "react";
import { PageIcon } from "../assets/PageIcon";
import { useTranslation } from "react-i18next";
import type { WikiInventoryScope } from "./pageInventory";
import { useLibraryInventory } from "./useLibraryInventory";
import "./wikiInventoryOverview.css";
import { FirstPageMilestone } from "../../onboarding/FirstPageMilestone";
import { GettingStarted } from "../../onboarding/GettingStarted";
import { useGettingStartedState } from "../../onboarding/useGettingStartedState";
import AddSourceMenu from "../sources/AddSourceMenu";
import { BasisUpdatesSummary } from "../page/BasisUpdatesSummary";
import { useWritePages, WritePagesLine } from "../askAi/WritePages";

interface PagesOverviewProps {
  readonly inventoryScope?: WikiInventoryScope;
  readonly onBrowseAll?: () => void;
  readonly onBrowseFolder?: (scope: WikiInventoryScope) => void;
  readonly onCreatePage: (space: string | null, folderPath?: string) => void;
  readonly onSelectDraft: (draftId: string, space: string | null) => void;
  readonly onSelectPage: (pageId: string) => void;
  readonly onSelectSpace: (spaceName: string) => void;
  /** Getting-started destinations. The checklist renders only when both are
   *  wired, so no step ever names an action with nothing behind it. Adding a
   *  source opens its dialog in place and needs no destination. */
  readonly onConnectTool?: () => void;
  readonly onSeeExample?: () => void;
  readonly onOpenIntelligence?: () => void;
  /** Open the review group for notes whose source note changed. */
  readonly onOpenBasisUpdates?: () => void;
}

export function PagesOverview({
  onCreatePage,
  onSelectPage,
  onConnectTool,
  onSeeExample,
  onOpenIntelligence,
  onOpenBasisUpdates,
}: PagesOverviewProps) {
  const { t } = useTranslation();
  const { pages, empty } = useLibraryInventory();
  const checklist = useGettingStartedState();
  const [adding, setAdding] = useState<"menu" | "chats" | null>(null);
  const writePages = useWritePages(pages);
  const gettingStarted = onConnectTool && onSeeExample ? (
    <GettingStarted
      onWriteNote={() => onCreatePage(null)}
      onConnectTool={onConnectTool}
      onAddSources={() => setAdding("menu")}
      onImportChats={() => setAdding("chats")}
      onSeeExample={onSeeExample}
      onTurnOnSteeping={onOpenIntelligence}
      writePages={writePages}
      onSelectPage={onSelectPage}
    />
  ) : null;
  // While the open checklist is the screen's content the panel above it shrinks
  // to its title, so the whole home fits the default window.
  const onboarding = gettingStarted !== null && checklist.visible && !checklist.dismissed;

  return (
    <section
      className="wiki-overview wiki-reading-start mx-auto w-full max-w-[1130px] pb-16"
      data-library-empty={empty ? "true" : undefined}
      data-onboarding={onboarding ? "true" : undefined}
    >
      {onOpenBasisUpdates && <BasisUpdatesSummary onOpen={onOpenBasisUpdates} />}
      {/* The open checklist carries the same offer as a step; one place at a time. */}
      {onboarding ? null : (
        <WritePagesLine
          state={writePages}
          pages={pages}
          onSelectPage={onSelectPage}
          onConnectTool={onConnectTool}
          onUseLocalModel={onOpenIntelligence}
        />
      )}
      <FirstPageMilestone pages={pages} onSelectPage={onSelectPage} />
      <div className="wiki-reading-start-panel">
        <PageIcon className="wiki-reading-start-icon" size={32} />
        <h1>{t("pages.overview.readingStartTitle")}</h1>
        <p>{t("pages.overview.readingStartDescription")}</p>
      </div>
      {gettingStarted}
      {adding ? <AddSourceMenu start={adding} onClose={() => setAdding(null)} /> : null}
    </section>
  );
}
