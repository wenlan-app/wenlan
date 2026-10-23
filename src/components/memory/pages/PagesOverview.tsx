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
import { pagesWrittenSince, useWritePages, WritePagesLine } from "../askAi/WritePages";
import { NewPagesList, newestPages } from "./NewPagesList";
import { useDaemonStartupPhase } from "../../../lib/daemonStartup";

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
  onBrowseAll,
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
  // "Use Search to find a page" is advice the workspace cannot honour until
  // the daemon answers; the sidebar says it is starting, so this stays quiet.
  // When it never answers, the page area says so in place of the start panel
  // (a page-sized empty state, not a banner) and points at the sidebar row,
  // which holds the one action.
  const daemonPhase = useDaemonStartupPhase();
  const daemonReady = daemonPhase === "ready";
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
  const open = gettingStarted !== null && checklist.visible && !checklist.dismissed;
  // While the AI tool is writing, the pages step is the place to follow it,
  // so the card stays whole even if writing a page by hand is all that's left.
  const writing = writePages.loaded && writePages.stepView.kind === "working";
  const compact = open && checklist.compact && !writing;
  // While the open checklist is the screen's content the panel above it shrinks
  // to its title, and the new pages sit inside its "write pages" step, so the
  // home is one card that fits the default window.
  const onboarding = open && !compact;
  const newPages = onboarding
    ? null
    : (
      <NewPagesList
        pages={pages}
        onSelectPage={onSelectPage}
        onBrowseAll={onBrowseAll}
        justWritten={new Set(writePages.run ? pagesWrittenSince(pages, writePages.run.startedAt).map((page) => page.id) : [])}
      />
    );
  // With pages to list, "open a page" has nothing to add.
  const listsPages = newPages !== null && newestPages(pages).length > 0;

  return (
    <section
      className="wiki-overview wiki-reading-start mx-auto w-full max-w-[1130px] pb-16"
      data-library-empty={empty ? "true" : undefined}
      data-onboarding={onboarding ? "true" : undefined}
      data-lists-pages={listsPages ? "true" : undefined}
    >
      {onOpenBasisUpdates && <BasisUpdatesSummary onOpen={onOpenBasisUpdates} />}
      {/* The open checklist carries the same offer as a step; one place at a time. */}
      {onboarding ? null : (
        <WritePagesLine
          state={writePages}
          pages={pages}
          onConnectTool={onConnectTool}
          onUseLocalModel={onOpenIntelligence}
        />
      )}
      <FirstPageMilestone pages={pages} onSelectPage={onSelectPage} />
      {daemonReady && (
        <div className="wiki-reading-start-panel">
          <PageIcon className="wiki-reading-start-icon" size={32} />
          <h1>{t("pages.overview.readingStartTitle")}</h1>
          <p>{t("pages.overview.readingStartDescription")}</p>
        </div>
      )}
      {daemonPhase === "failed" && (
        <div className="wiki-reading-start-panel wiki-reading-start-panel--unavailable" data-testid="pages-unavailable">
          <PageIcon className="wiki-reading-start-icon" size={32} />
          <h1>{t("pages.overview.unavailableTitle")}</h1>
          <p>{t("pages.overview.unavailableDescription")}</p>
        </div>
      )}
      {/* The one-line card says what is left above the pages; otherwise the
          pages come first and the card (or its "show" line) follows. */}
      {compact ? gettingStarted : null}
      {newPages}
      {compact ? null : gettingStarted}
      {adding ? <AddSourceMenu start={adding} onClose={() => setAdding(null)} /> : null}
    </section>
  );
}
