// SPDX-License-Identifier: AGPL-3.0-only
import { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import PageDetail from "../PageDetail";
import { PageDraftEditor, type PageDraftEditorHandle } from "./PageDraftEditor";
import { NoteGroupFrame } from "./NoteGroupFrame";
import { NoteTabs } from "./NoteTabs";
import { matchesNote, noteId, noteReadingKey, promoteNoteTab, reorderNoteTab, upsertNoteTab, type NoteTab, type NoteView } from "./noteTabState";
import type { View } from "../navigation/viewState";
import type { MarkdownEditorSelection } from "../editor/MarkdownEditor";
import { ReferenceNavigationProvider } from "../links/ReferenceNavigationContext";
import { useViewScroll } from "../navigation/useViewScroll";
import { recordRecentPageVisit } from "../../../lib/recentPages";

export interface SecondaryNoteGroupHandle {
  flush: () => Promise<boolean>;
  contains: (view: NoteView) => boolean;
  open: (view: NoteView, tab?: NoteTab, commit?: () => boolean, beforeKey?: string | null) => Promise<boolean>;
  reorder: (key: string, beforeKey: string | null) => void;
  move: (key: string, beforeKey: string | null) => Promise<void>;
  removePage: (pageId: string) => void;
}
interface Props {
  host: HTMLElement | null;
  onPresenceChange: (present: boolean) => void;
  onFocus: () => void;
  onNavigationIntent: () => void;
  onNavigate: (view: View) => void;
  onOpenNote: (view: NoteView) => void;
  onMove: (tab: NoteTab, commit: () => boolean, beforeKey?: string | null) => Promise<boolean>;
  onPageDeleted?: (pageId: string) => void;
  movingNote?: boolean;
  readingPositions: Map<string, number>;
  selections: Map<string, MarkdownEditorSelection>;
  allocateDraftSession: () => number;
}

/** An independent editor owner; moving its portal never recreates its draft. */
export const SecondaryNoteGroup = forwardRef<SecondaryNoteGroupHandle, Props>(function SecondaryNoteGroup({ host, onPresenceChange, onFocus, onNavigationIntent, onNavigate, onOpenNote, onMove, onPageDeleted, movingNote = false, allocateDraftSession, readingPositions, selections: sharedSelections }, ref) {
  const { t } = useTranslation();
  const [mount] = useState(() => document.createElement("div"));
  mount.className = "secondary-note-group-mount";
  const root = useRef<HTMLDivElement>(null);
  const [tabs, setTabs] = useState<NoteTab[]>([]);
  const tabsRef = useRef(tabs);
  const [view, setView] = useState<NoteView | null>(null);
  const viewRef = useRef(view);
  const [ready, setReady] = useState<NoteView | null>(null);
  const pageFlush = useRef<(() => Promise<boolean>) | null>(null);
  const draft = useRef<PageDraftEditorHandle>(null);
  const draftIdentityByTab = useRef(new Map<string, string>());
  const dirty = useRef(false);
  const saving = useRef(false);
  const intent = useRef<symbol | null>(null);
  const selections = useRef(new Map<string, MarkdownEditorSelection>());
  if (selections.current !== sharedSelections) {
    for (const [id, selection] of selections.current) sharedSelections.set(id, selection);
    selections.current = sharedSelections;
  }
  const updateTabs = (next: NoteTab[]) => { tabsRef.current = next; setTabs(next); };
  const show = (next: NoteView | null, incoming?: NoteTab) => {
    viewRef.current = next;
    setView(next);
    setReady(null);
    if (next) {
      let updated = upsertNoteTab(tabsRef.current, next);
      if (incoming) updated = updated.map(tab => matchesNote(tab, next) ? { ...tab, title: incoming.title } : tab);
      updateTabs(updated);
    }
    onPresenceChange(next !== null);
  };
  useLayoutEffect(() => {
    if (host) host.appendChild(mount);
    else mount.remove();
    return () => mount.remove();
  }, [host, mount]);
  const reportDirty = useCallback((value: boolean) => { dirty.current = value; }, []);
  const reportSaving = useCallback((value: boolean) => { saving.current = value; }, []);
  const registerFlush = useCallback((flush: (() => Promise<boolean>) | null) => { pageFlush.current = flush; }, []);
  const activeDraftTabKey = view?.kind === "page-draft"
    ? tabsRef.current.find(item => matchesNote(item, view))?.key
    : undefined;
  const reportDraftIdentity = useCallback((draftId: string) => {
    if (activeDraftTabKey && tabsRef.current.some(item => item.key === activeDraftTabKey)) {
      draftIdentityByTab.current.set(activeDraftTabKey, draftId);
    }
  }, [activeDraftTabKey]);
  const findTab = (next: NoteView) => tabsRef.current.find(item =>
    matchesNote(item, next)
    || (next.kind === "page-draft" && !!next.draftId && draftIdentityByTab.current.get(item.key) === next.draftId)
  );
  const flush = async () => {
    try {
      const current = viewRef.current;
      if (!current) return true;
      if (current.kind === "page") return pageFlush.current ? await pageFlush.current() : !dirty.current && !saving.current;
      const editor = draft.current;
      // A load/error/missing wrapper has no hydrated editor and therefore no
      // local changes to save. A mounted dirty editor still owns its flush.
      if (!editor) return true;
      if (!await editor.flush()) return false;
      if (viewRef.current !== current) return false;
      const identity = editor.getIdentity();
      const canonical: NoteView = identity.publishedPage
        ? { kind: "page", pageId: identity.publishedPage.id, inventoryScope: current.inventoryScope, projectionIssue: identity.projectionIssue }
        : { ...current, draftId: identity.draftId ?? undefined };
      const tab = tabsRef.current.find(item => matchesNote(item, current));
      if (tab && identity.draftId) {
        draftIdentityByTab.current.set(tab.key, identity.draftId);
        if (!identity.publishedPage) {
          // Persist the canonical identity on the tab while leaving the active
          // editor's props and session key untouched.
          updateTabs(tabsRef.current.map(item => item.key === tab.key ? { ...item, view: canonical } : item));
        }
      }
      if (identity.publishedPage) {
        if (tab) draftIdentityByTab.current.delete(tab.key);
        viewRef.current = canonical;
        setView(canonical);
        updateTabs(promoteNoteTab(tabsRef.current, current, canonical).map(item => matchesNote(item, canonical) ? { ...item, title: identity.publishedPage!.title } : item));
      }
      return true;
    } catch { return false; }
  };
  const open = async (next: NoteView, incoming?: NoteTab, commit?: () => boolean, beforeKey?: string | null) => {
    onNavigationIntent();
    const token = Symbol("secondary-navigation"); intent.current = token;
    const existing = findTab(next);
    if (!await flush() || intent.current !== token) return false;
    if (existing && !tabsRef.current.some(tab => tab.key === existing.key)) return false;
    if (beforeKey != null && !tabsRef.current.some(tab => tab.key === beforeKey)) return false;
    if (commit && !commit()) return false;
    const target = existing?.view ?? (next.kind === "page-draft" && next.sessionKey == null ? { ...next, sessionKey: allocateDraftSession() } : next);
    if (!viewRef.current || !matchesNote({ key: "", title: "", view: viewRef.current }, target)) show(target, incoming);
    if (beforeKey !== undefined) {
      const inserted = tabsRef.current.find(tab => matchesNote(tab, target));
      if (inserted) updateTabs(reorderNoteTab(tabsRef.current, inserted.key, beforeKey));
    }
    onFocus();
    requestAnimationFrame(() => root.current?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus({ preventScroll: true }));
    return true;
  };
  // Drops every tab showing this note, a published page or a draft (matched by
  // its id or by the identity its editor reported), without flushing it.
  const removePage = (pageId: string) => {
    const current = viewRef.current;
    const isGone = (tab: NoteTab) => noteId(tab.view) === pageId || draftIdentityByTab.current.get(tab.key) === pageId;
    const index = tabsRef.current.findIndex(isGone);
    if (index < 0) return;
    const currentGone = !!current && tabsRef.current.some(tab => isGone(tab) && matchesNote(tab, current));
    const removedKeys = new Set(tabsRef.current.filter(isGone).map(tab => tab.key));
    const remaining = tabsRef.current.filter(tab => !removedKeys.has(tab.key));
    for (const key of removedKeys) draftIdentityByTab.current.delete(key);
    updateTabs(remaining);
    if (currentGone) {
      pageFlush.current = null;
      dirty.current = false;
      saving.current = false;
      show(remaining[Math.min(index, remaining.length - 1)]?.view ?? null);
    }
  };
  useImperativeHandle(ref, () => ({ flush, open, contains: next => !!findTab(next),
    reorder: (key, beforeKey) => updateTabs(reorderNoteTab(tabsRef.current, key, beforeKey)),
    move: async (key, beforeKey) => { const tab = tabsRef.current.find(item => item.key === key); if (tab) await transfer(tab, beforeKey); },
    removePage,
  }));
  useViewScroll(root, view ? noteReadingKey(view) : "empty", view?.kind !== "page" || ready === view, ".wiki-workspace-content", readingPositions);
  const close = async (tab: NoteTab) => {
    if (movingNote) return;
    onNavigationIntent();
    const current = viewRef.current;
    const active = !!current && matchesNote(tab, current);
    const token = Symbol("secondary-close");
    if (active) { intent.current = token; if (!await flush() || intent.current !== token) return; }
    const before = tabsRef.current;
    const index = before.findIndex(item => item.key === tab.key);
    if (index < 0) return;
    const remaining = before.filter(item => item.key !== tab.key);
    draftIdentityByTab.current.delete(tab.key);
    updateTabs(remaining);
    if (active) show(remaining[Math.min(index, remaining.length - 1)]?.view ?? null);
  };
  const transfer = async (tab: NoteTab, beforeKey?: string | null) => {
    if (movingNote) return;
    onNavigationIntent();
    const token = Symbol("secondary-transfer"); intent.current = token;
    if (!await flush() || intent.current !== token) return;
    const current = tabsRef.current.find(item => item.key === tab.key);
    if (!current) return;
    const draftId = draftIdentityByTab.current.get(current.key);
    const outgoing = draftId && current.view.kind === "page-draft"
      ? { ...current, view: { ...current.view, draftId } }
      : current;
    await onMove(outgoing, () => {
      // The destination can be saving while this group receives a newer intent.
      if (intent.current !== token || dirty.current || saving.current || !tabsRef.current.some(item => item.key === tab.key)) return false;
      const active = !!viewRef.current && matchesNote(current, viewRef.current);
      const remaining = tabsRef.current.filter(item => item.key !== tab.key);
      draftIdentityByTab.current.delete(tab.key);
      updateTabs(remaining);
      if (active) show(remaining[0]?.view ?? null);
      return true;
    }, beforeKey);
  };
  const replace = (next: NoteView) => {
    const previous = viewRef.current;
    if (previous) {
      const tab = tabsRef.current.find(item => matchesNote(item, previous));
      if (tab) draftIdentityByTab.current.delete(tab.key);
      updateTabs(promoteNoteTab(tabsRef.current, previous, next));
    }
    show(next);
  };
  const rename = (title: string) => {
    const current = viewRef.current;
    if (current && tabsRef.current.some(tab => matchesNote(tab, current) && tab.title !== title)) updateTabs(tabsRef.current.map(tab => matchesNote(tab, current) ? { ...tab, title } : tab));
  };
  const pageLoaded = useCallback((page: Parameters<NonNullable<React.ComponentProps<typeof PageDetail>["onPageLoaded"]>>[0]) => {
    const current = viewRef.current;
    if (current && tabsRef.current.some(tab => matchesNote(tab, current) && tab.title !== page.title)) {
      const updated = tabsRef.current.map(tab => matchesNote(tab, current) ? { ...tab, title: page.title } : tab);
      tabsRef.current = updated; setTabs(updated);
    }
    recordRecentPageVisit(page);
  }, []);
  const globalNavigate = async (next: View) => {
    if (movingNote) return;
    onNavigationIntent();
    const token = Symbol("secondary-global-navigation"); intent.current = token;
    if (await flush() && intent.current === token) onNavigate(next);
  };
  return createPortal(<div ref={root} className="secondary-note-group-root">
    {view && <ReferenceNavigationProvider onOpenPage={pageId => onOpenNote({ kind: "page", pageId })} onOpenMemory={sourceId => void globalNavigate({ kind: "memory", sourceId })}>
      <NoteGroupFrame id="secondary" label={t("pages.groups.right")} contentId="wiki-secondary-note-content" onFocus={onFocus} tabs={<NoteTabs
        tabs={tabs} activeKey={tabs.find(tab => matchesNote(tab, view))?.key} contentId="wiki-secondary-note-content"
        groupId="secondary" tabListLabel={t("pages.groups.right")} moveLabel={t("pages.groups.moveToCentral")}
        onMoveToOtherGroup={tab => { if (!movingNote) void transfer(tab); }} onSelect={tab => { if (!movingNote) void open(tab.view); }} onClose={tab => void close(tab)}
        onCreate={() => { if (!movingNote) void open({ kind: "page-draft", space: null }); }} />}>
        {view.kind === "page" ? <PageDetail pageId={view.pageId} projectionIssue={view.projectionIssue}
          onDeleted={pageId => onPageDeleted ? onPageDeleted(pageId) : removePage(pageId)}
          onProjectionResolved={() => replace({ ...view, projectionIssue: undefined })}
          initialSelection={selections.current.get(view.pageId)} onSelectionChange={selection => selections.current.set(view.pageId, selection)}
          onEditorReady={() => setReady(view)} initialMode={view.mode ?? "edit"}
          onRegisterFlush={registerFlush} onEditDirtyChange={reportDirty} onSavePendingChange={reportSaving}
          onBack={() => { const tab = tabsRef.current.find(tab => matchesNote(tab, view)); if (tab) void close(tab); }}
          onPageLoaded={pageLoaded}
          onOpenReview={reviewItemId => void globalNavigate({ kind: "distill-review", reviewItemId })}
          onPageClick={pageId => onOpenNote({ kind: "page", pageId })} onMemoryClick={sourceId => void globalNavigate({ kind: "memory", sourceId })}
          onEntityClick={entityId => {
            if (entityId.startsWith("page:")) onOpenNote({ kind: "page", pageId: entityId.slice(5) });
            else if (entityId.startsWith("memory:")) void globalNavigate({ kind: "memory", sourceId: entityId.slice(7) });
            else void globalNavigate(entityId === "__create_profile__" ? { kind: "settings", section: "general" } : { kind: "entity", entityId });
          }} onOpenGraph={focusPageId => void globalNavigate({ kind: "graph", focusPageId })} />
          : <PageDraftEditor key={view.sessionKey ?? view.draftId} ref={draft} draftId={view.draftId} space={view.space} folderPath={view.folderPath}
            onDraftIdentity={reportDraftIdentity}
            onTitleChange={rename} onBack={() => { const tab = tabsRef.current.find(tab => matchesNote(tab, view)); if (tab) void close(tab); }}
            onOpenExisting={pageId => onOpenNote({ kind: "page", pageId })}
            onPublished={(pageId, projectionIssue) => replace({ kind: "page", pageId, inventoryScope: view.inventoryScope, projectionIssue })} />}
      </NoteGroupFrame>
    </ReferenceNavigationProvider>}
  </div>, mount);
});
