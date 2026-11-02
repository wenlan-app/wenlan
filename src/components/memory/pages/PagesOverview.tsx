import { useEffect, useState } from "react";
import { PageIcon } from "../assets/PageIcon";
import { useTranslation } from "react-i18next";
import type { WikiInventoryScope } from "./pageInventory";
import { useLibraryInventory } from "./useLibraryInventory";
import "./wikiInventoryOverview.css";
import { FirstPageMilestone } from "../../onboarding/FirstPageMilestone";
import { GettingStarted } from "../../onboarding/GettingStarted";
import { EverydayUse } from "../../onboarding/EverydayUse";
import { useGettingStartedState } from "../../onboarding/useGettingStartedState";
import AddSourceMenu from "../sources/AddSourceMenu";
import { onStartIntent, peekStartIntent, takeStartIntent } from "../../onboarding/startIntent";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { BasisUpdatesSummary } from "../page/BasisUpdatesSummary";
import type { SourceContainerLocation } from "../sources/SourceContainer";
import { NoteSuggestionsSummary } from "../sources/VaultNotes";
import { pagesWrittenSince, useWritePages, WritePagesLine } from "../askAi/WritePages";
import { NewPagesList, recentPages } from "./NewPagesList";
import { WorkingOn } from "./WorkingOn";
import { useDaemonStartupPhase } from "../../../lib/daemonStartup";
import { useQuery } from "@tanstack/react-query";
import { ARCHIVED_PAGES_QUERY_KEY, EXPLICIT_BROWSE_QUERY_POLICY, isKnowledgePage, listAllArchivedPagesExplicitBrowse } from "./listAllPages";
import "./archivedNotes.css";

interface PagesOverviewProps {
  readonly inventoryScope?: WikiInventoryScope;
  readonly onBrowseAll?: () => void;
  readonly onBrowseArchived?: () => void;
  readonly onBrowseFolder?: (scope: WikiInventoryScope) => void;
  readonly onCreatePage: (space: string | null, folderPath?: string) => void;
  readonly onSelectDraft: (draftId: string, space: string | null) => void;
  readonly onSelectPage: (pageId: string) => void;
  readonly onSelectSpace: (spaceName: string) => void;
  /** Opens one thing from 「正在做的事」 (UX-HOME-1). */
  readonly onOpenThing?: (thingId: string) => void;
  /** Getting-started destinations. The checklist renders only when both are
   *  wired, so no step ever names an action with nothing behind it. Adding a
   *  source opens its dialog in place and needs no destination. */
  readonly onConnectTool?: () => void;
  readonly onSeeExample?: () => void;
  /** Opens the paste-back import, the quick way to bring past AI chats in. */
  readonly onImportChatsQuick?: () => void;
  readonly onOpenIntelligence?: () => void;
  /** Open the review group for notes whose source note changed. */
  readonly onOpenBasisUpdates?: () => void;
  /** Open a vault note an AI wants to change, where Accept and Decline are. */
  readonly onOpenNoteSuggestion?: (location: SourceContainerLocation) => void;
}

export function PagesOverview({
  onBrowseAll,
  onBrowseArchived,
  onCreatePage,
  onSelectPage,
  onSelectSpace,
  onOpenThing,
  onConnectTool,
  onSeeExample,
  onImportChatsQuick,
  onOpenIntelligence,
  onOpenBasisUpdates,
  onOpenNoteSuggestion,
}: PagesOverviewProps) {
  const { t } = useTranslation();
  const { pages, empty } = useLibraryInventory();
  const checklist = useGettingStartedState();
  const [adding, setAdding] = useState<false | "menu" | "folder" | "chats">(false);
  // What the user picked on the welcome screen happens here, once: the
  // folder picker, the past-chats choice, or a new page. Leaving the example
  // wiki sets one while this home may stay mounted or be replaced, so it is
  // read on arrival and whenever one is set, and a dialog's intent is only
  // let go when that dialog closes.
  const [intentSet, setIntentSet] = useState(0);
  useEffect(() => onStartIntent(() => setIntentSet((count) => count + 1)), []);
  useEffect(() => {
    const intent = peekStartIntent();
    if (intent === "empty") {
      takeStartIntent();
      onCreatePage(null);
    } else if (intent === "material") setAdding("menu");
    else if (intent) setAdding(intent);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- on arrival and when an intent is set
  }, [intentSet]);
  const closeAdding = () => {
    takeStartIntent();
    setAdding(false);
  };
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
      onSeeExample={onSeeExample}
      onTurnOnSteeping={onOpenIntelligence}
      writePages={writePages}
      onSelectPage={onSelectPage}
    />
  ) : null;
  const open = gettingStarted !== null && checklist.visible && !checklist.dismissed;
  // Once the three steps are done, a still-new library gets the everyday
  // sentences, once, as an overlay.
  const example = useExampleWikiActive();
  // The way back to archived pages shows only when there is one to go back to.
  const archived = useQuery({
    queryKey: ARCHIVED_PAGES_QUERY_KEY,
    queryFn: listAllArchivedPagesExplicitBrowse,
    ...EXPLICIT_BROWSE_QUERY_POLICY,
    enabled: !!onBrowseArchived && !example && daemonReady,
  });
  const hasArchived = !example && (archived.data ?? []).some(isKnowledgePage);
  const everyday = !example && gettingStarted !== null && checklist.loaded && checklist.newLibrary && !checklist.visible && writePages.tool
    ? <EverydayUse tool={writePages.tool} />
    : null;
  // While the open checklist is the screen's content the panel above it shrinks
  // to its title, and the new pages sit inside its "write pages" step, so the
  // home is one card that fits the default window.
  const onboarding = open;
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
  const listsPages = newPages !== null && recentPages(pages).length > 0;

  return (
    <section
      className="wiki-overview wiki-reading-start mx-auto w-full max-w-[1130px] pb-16"
      data-library-empty={empty ? "true" : undefined}
      data-onboarding={onboarding ? "true" : undefined}
      data-lists-pages={listsPages ? "true" : undefined}
      data-home="true"
    >
      {onOpenBasisUpdates && <BasisUpdatesSummary onOpen={onOpenBasisUpdates} />}
      {onOpenNoteSuggestion && <NoteSuggestionsSummary onOpen={onOpenNoteSuggestion} />}
      {/* The open checklist carries the same offer as a step; one place at a time. */}
      {onboarding ? null : (
        <WritePagesLine
          state={writePages}
          pages={pages}
          onConnectTool={onConnectTool}
          onUseLocalModel={onOpenIntelligence}
        />
      )}
      {example ? null : <FirstPageMilestone pages={pages} onSelectPage={onSelectPage} />}
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
      {/* The pages come first and the card (or its "show" line) follows;
          the everyday overlay floats over both. */}
      {/* What you're working on leads (UX-HOME-1); the pages edited last sit
          under it as one row of cards that scrolls sideways, so neither pushes
          the other down. */}
      <div className="home-columns">
        {example ? null : <WorkingOn onSelectSpace={onSelectSpace} onOpenThing={onOpenThing} />}
        {newPages}
        {/* With no AI tool connected there is nothing to be working on yet:
            the pages lead, and one line says what connecting adds. */}
        {!example && !onboarding && onConnectTool && writePages.loaded && writePages.tools.length === 0 ? (
          <p className="home-connect-line">
            <span>{t("pages.overview.workingOnConnect")}</span>
            <button type="button" onClick={onConnectTool}>{t("pages.overview.workingOnConnectAction")}</button>
          </p>
        ) : null}
      </div>
      {gettingStarted}
      {everyday}
      {/* No count beside it: home counts nothing (UX-HOME-2). */}
      {onBrowseArchived && hasArchived && (
        <button className="wiki-archive-entry" type="button" onClick={onBrowseArchived}>
          {t("pages.archive.title")}
        </button>
      )}
      {adding ? <AddSourceMenu start={adding} onQuickChats={onImportChatsQuick} onClose={closeAdding} /> : null}
    </section>
  );
}

/**
 * The Wiki beside its directory. Every page is in the directory, by folder;
 * what is in motion lives on Home, so this side only invites opening a page.
 */
export function WikiStart() {
  const { t } = useTranslation();
  const daemonPhase = useDaemonStartupPhase();
  const unavailable = daemonPhase === "failed";
  return (
    <section className="wiki-overview wiki-reading-start mx-auto w-full max-w-[1130px] pb-16" data-testid="wiki-start">
      <div className={`wiki-reading-start-panel${unavailable ? " wiki-reading-start-panel--unavailable" : ""}`}>
        <PageIcon className="wiki-reading-start-icon" size={32} />
        <h1>{t(unavailable ? "pages.overview.unavailableTitle" : "pages.overview.readingStartTitle")}</h1>
        <p>{t(unavailable ? "pages.overview.unavailableDescription" : "pages.overview.readingStartDescription")}</p>
      </div>
    </section>
  );
}
