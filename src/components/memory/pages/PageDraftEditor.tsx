import { createPortal } from "react-dom";
import { SidebarSimple } from "@phosphor-icons/react";
import {
  forwardRef,
  useCallback,
  useContext,
  useId,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";

import { WorkspaceDocumentToolsHostContext, WorkspaceNoteGroupContext } from "../navigation/WorkspacePaneHost";
import { WorkspaceBackButton } from "../navigation/WorkspaceNavigation";
import PageInfoDrawer from "../page/PageInfoDrawer";
import { getPage, publishPageDraft, type Page } from "../../../lib/tauri";
import {
  usePageDraftAutosave,
  type PageDraftSnapshot,
} from "./usePageDraftAutosave";
import type { PageProjectionIssue } from "./pageInventory";
import "./pageActions.css";
import "./pageDraftEditor.css";

export type PageDraftEditorHandle = {
  readonly flush: () => Promise<boolean>;
  readonly getIdentity: () => {
    readonly draftId: string | null;
    readonly version: number | null;
    readonly publishedPage?: Page;
    readonly projectionIssue?: PageProjectionIssue;
  };
  readonly requestBack: () => Promise<void>;
};

type PageDraftEditorProps = {
  readonly draftId?: string;
  readonly folderPath?: string;
  readonly onBack: () => void;
  readonly onDraftIdentity?: (draftId: string) => void;
  readonly onTitleChange?: (title: string) => void;
  readonly onEscapeBeforeLeave?: () => boolean;
  readonly onOpenExisting: (pageId: string) => void;
  readonly onPublished: (pageId: string, projectionIssue?: PageProjectionIssue) => void;
  readonly space: string | null;
};

function errorProperty(error: unknown, ...keys: string[]): string | null {
  if (!error || typeof error !== "object") return null;
  const record = error as Record<string, unknown>;
  for (const key of keys) {
    if (typeof record[key] === "string") return record[key] as string;
  }
  return null;
}

function errorCode(error: unknown): string | null {
  return errorProperty(error, "code");
}

type HydratedEditorProps = PageDraftEditorProps & {
  readonly initialPage: Page | null;
};

const HydratedPageDraftEditor = forwardRef<PageDraftEditorHandle, HydratedEditorProps>(
  function HydratedPageDraftEditor({
    initialPage,
    folderPath,
    onBack,
    onDraftIdentity,
    onTitleChange,
    onEscapeBeforeLeave,
    onPublished,
    space: initialSpace,
  }, ref) {
    const { t } = useTranslation();
    const noteGroup = useContext(WorkspaceNoteGroupContext);
    const documentToolsHost = useContext(WorkspaceDocumentToolsHostContext);
    const headingId = useId();
    const queryClient = useQueryClient();
    const titleRef = useRef<HTMLInputElement>(null);
    const inspectorToggleRef = useRef<HTMLButtonElement>(null);
    const [inspectorOpen, setInspectorOpen] = useState(false);
    const [title, setTitle] = useState(initialPage?.title ?? "");
    useEffect(() => { onTitleChange?.(title); }, [title, onTitleChange]);
    const [content, setContent] = useState(initialPage?.content ?? "");
    const [space, setSpace] = useState(initialPage?.space ?? initialSpace ?? "");
    const [operationKind, setOperationKind] = useState<"idle" | "finalizing" | "uncertain" | "finalized">("idle");
    const [publishError, setPublishError] = useState<Error | null>(null);
    const [publishConflict, setPublishConflict] = useState(false);
    const [publishVersionConflict, setPublishVersionConflict] = useState(false);
    const [reloadError, setReloadError] = useState(false);
    const finalizePromiseRef = useRef<Promise<boolean> | null>(null);
    const leavePromiseRef = useRef<Promise<void> | null>(null);
    const publishedPageRef = useRef<Page | null>(null);
    const projectionIssueRef = useRef<PageProjectionIssue | undefined>(undefined);
    const uncertainPublishRef = useRef(false);
    const blockedVersionRef = useRef(false);

    const snapshot = useMemo<PageDraftSnapshot>(() => ({
      title,
      content,
      space: space || null,
    }), [content, space, title]);
    const initialSnapshot = useMemo<PageDraftSnapshot>(() => ({
      title: initialPage?.title ?? "",
      content: initialPage?.content ?? "",
      space: initialPage?.space ?? initialSpace ?? null,
    }), [initialPage, initialSpace]);
    const reconcileSpace = useCallback((reconciledSpace: string | null) => {
      setSpace(reconciledSpace ?? "");
    }, []);
    const autosave = usePageDraftAutosave({
      draftId: initialPage?.id,
      folderPath: initialPage?.folder_path ?? folderPath,
      initial: initialSnapshot,
      initialVersion: initialPage?.version,
      onDraftIdentity,
      onSpaceReconciled: reconcileSpace,
      snapshot,
    });

    const finishPublished = useCallback(async (published: Page) => {
      publishedPageRef.current = published;
      projectionIssueRef.current = published.projection_status === "pending" && published.projection_error
        ? { expectedVersion: published.version - 1 }
        : undefined;
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["pages"] }),
        queryClient.invalidateQueries({ queryKey: ["pages", "active"] }),
        queryClient.invalidateQueries({ queryKey: ["pages", "draft"] }),
        queryClient.invalidateQueries({ queryKey: ["page", published.id] }),
        queryClient.invalidateQueries({ queryKey: ["spaces-page-counts"] }),
        queryClient.invalidateQueries({ queryKey: ["sidebar-space-page-counts"] }),
      ]);
    }, [queryClient]);

    const finalize = useCallback((): Promise<boolean> => {
      if (publishedPageRef.current) return Promise.resolve(true);
      if (finalizePromiseRef.current) return finalizePromiseRef.current;

      setOperationKind("finalizing");
      setPublishError(null);
      setPublishConflict(false);
      setPublishVersionConflict(false);
      const finalizing = (async (): Promise<boolean> => {
        let identity = autosave.getIdentity();
        let publishId: string | null = identity.draftId;
        let publishVersion: number | null = identity.version;
        let attemptedPublish = false;
        try {
          if (uncertainPublishRef.current) {
            if (!publishId || publishVersion == null) {
              throw new Error("The saved note identity is unavailable.");
            }
            const reconciled = await getPage(publishId);
            if (reconciled?.status === "active") {
              await finishPublished(reconciled);
              return true;
            }
            if (!reconciled || reconciled.status !== "draft") {
              throw new Error("The saved note could not be reconciled.");
            }
            if (reconciled.version !== publishVersion) {
              blockedVersionRef.current = true;
              setPublishVersionConflict(true);
              throw Object.assign(new Error("This draft changed elsewhere."), {
                code: "draft_version_conflict",
              });
            }
            uncertainPublishRef.current = false;
          }

          if (!await autosave.flush()) return false;
          if (snapshot.title.trim().length === 0 && snapshot.content.trim().length === 0) return true;

          identity = autosave.getIdentity();
          if (!identity.draftId || identity.version == null) {
            throw new Error("The saved note identity is unavailable.");
          }
          publishId = identity.draftId;
          publishVersion = identity.version;
          attemptedPublish = true;
          const published = await publishPageDraft({
            id: publishId,
            expectedVersion: publishVersion,
          });
          uncertainPublishRef.current = false;
          blockedVersionRef.current = false;
          await finishPublished(published);
          return true;
        } catch (cause) {
          const nextError = cause instanceof Error ? cause : new Error(String(cause));
          const code = errorCode(cause);
          if (code === "page_title_conflict") {
            setPublishConflict(true);
          } else if (attemptedPublish && publishId && publishVersion != null) {
            uncertainPublishRef.current = true;
            try {
              const reconciled = await getPage(publishId);
              if (reconciled?.status === "active") {
                uncertainPublishRef.current = false;
                blockedVersionRef.current = false;
                await finishPublished(reconciled);
                return true;
              }
              if (reconciled?.status === "draft") {
                if (reconciled.version === publishVersion) {
                  uncertainPublishRef.current = false;
                } else {
                  blockedVersionRef.current = true;
                  setPublishVersionConflict(true);
                }
              }
            } catch {
              // Keep the fields locked until an explicit retry can reconcile the result.
            }
            if (code === "draft_version_conflict") {
              blockedVersionRef.current = true;
              setPublishVersionConflict(true);
            }
          } else if (code === "draft_version_conflict") {
            blockedVersionRef.current = true;
            setPublishVersionConflict(true);
          }
          setPublishError(nextError);
          return false;
        }
      })();

      finalizePromiseRef.current = finalizing;
      void finalizing.then((succeeded) => {
        if (finalizePromiseRef.current === finalizing) finalizePromiseRef.current = null;
        if (succeeded && publishedPageRef.current) setOperationKind("finalized");
        else if (uncertainPublishRef.current || blockedVersionRef.current) setOperationKind("uncertain");
        else setOperationKind("idle");
      }, () => {
        if (finalizePromiseRef.current === finalizing) finalizePromiseRef.current = null;
        setOperationKind(uncertainPublishRef.current || blockedVersionRef.current ? "uncertain" : "idle");
      });
      return finalizing;
    }, [autosave.flush, autosave.getIdentity, finishPublished, snapshot]);

    const retryFinalize = useCallback(async () => {
      if (!await finalize()) return;
      const published = publishedPageRef.current;
      if (published) onPublished(published.id, projectionIssueRef.current);
    }, [finalize, onPublished]);

    const requestBack = useCallback((): Promise<void> => {
      if (leavePromiseRef.current) return leavePromiseRef.current;
      const leaving = (async () => {
        if (await finalize()) onBack();
      })();
      leavePromiseRef.current = leaving;
      const clear = () => {
        if (leavePromiseRef.current === leaving) leavePromiseRef.current = null;
      };
      void leaving.then(clear, clear);
      return leaving;
    }, [finalize, onBack]);

    const getIdentity = useCallback(() => ({
      ...autosave.getIdentity(),
      ...(publishedPageRef.current ? { publishedPage: publishedPageRef.current } : {}),
      ...(projectionIssueRef.current ? { projectionIssue: projectionIssueRef.current } : {}),
    }), [autosave.getIdentity]);

    useImperativeHandle(ref, () => ({ flush: finalize, getIdentity, requestBack }), [
      finalize,
      getIdentity,
      requestBack,
    ]);

    useEffect(() => {
      const handleEscape = (event: KeyboardEvent) => {
        if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return;
        if (noteGroup && !noteGroup.element?.contains(event.target as Node)) return;
        // Let an open sidebar popup consume Escape before draft navigation or
        // the narrow drawer's close guard sees it.
        if (event.target instanceof Element && event.target.closest("[data-sidebar-escape-scope]")) return;
        event.preventDefault();
        event.stopPropagation();
        if (inspectorOpen) {
          setInspectorOpen(false);
          inspectorToggleRef.current?.focus({ preventScroll: true });
          return;
        }
        if (onEscapeBeforeLeave?.()) return;
        void requestBack();
      };
      window.addEventListener("keydown", handleEscape, true);
      return () => window.removeEventListener("keydown", handleEscape, true);
    }, [inspectorOpen, onEscapeBeforeLeave, requestBack, noteGroup]);

    const reloadLatest = async () => {
      const identity = autosave.getIdentity();
      if (!identity.draftId) return;
      setReloadError(false);
      try {
        const latest = await getPage(identity.draftId);
        if (!latest || latest.status !== "draft") {
          setReloadError(true);
          return;
        }
        const latestSnapshot = {
          title: latest.title,
          content: latest.content,
          space: latest.space ?? null,
        };
        setTitle(latest.title);
        setContent(latest.content);
        setSpace(latest.space ?? "");
        autosave.adoptRemote({
          draftId: latest.id,
          version: latest.version,
          snapshot: latestSnapshot,
        });
        setPublishError(null);
        setPublishVersionConflict(false);
        uncertainPublishRef.current = false;
        blockedVersionRef.current = false;
        setOperationKind("idle");
      } catch {
        setReloadError(true);
      }
    };

    const renameDraft = () => {
      setPublishConflict(false);
      setPublishError(null);
      titleRef.current?.focus();
      titleRef.current?.select();
    };

    const locked = operationKind !== "idle";
    const autosaveLabel = autosave.status === "saving"
      ? t("pages.editor.saving")
      : autosave.status === "saved"
        ? t("pages.editor.saved")
        : autosave.status === "error"
          ? t("pages.editor.saveError")
          : autosave.status === "conflict"
            ? t("pages.editor.versionConflict")
            : title.trim() || content.trim()
              ? t("pageDetail.editor.unsaved")
              : t("pages.overview.draft");
    const inspectorToggle = (
      <button
        aria-expanded={inspectorOpen}
        aria-label={t(inspectorOpen ? "pageInspector.close" : "pageInspector.open")}
        className="mem-icon-action workspace-panel-toggle note-inspector-toggle"
        onClick={(event) => {
          event.currentTarget.focus({ preventScroll: true });
          setInspectorOpen((open) => !open);
        }}
        ref={inspectorToggleRef}
        title={t(inspectorOpen ? "pageInspector.close" : "pageInspector.open")}
        type="button"
      >
        <SidebarSimple aria-hidden="true" size={18} style={{ transform: "scaleX(-1)" }} />
      </button>
    );

    return (
      <>
      {documentToolsHost ? createPortal(inspectorToggle, documentToolsHost) : null}
      <section className="page-draft-editor" aria-labelledby={headingId}>
        <div className="page-draft-editor-axis">
          {!documentToolsHost && <div className="page-draft-tools">{inspectorToggle}</div>}
          <h1 className="sr-only" id={headingId}>
            {t("pages.editor.heading")}
          </h1>
          <div className="page-draft-notices">
            {autosave.status === "error" && (
              <div className="page-draft-notice page-draft-notice-error" role="alert">
                <span>{t("pages.editor.saveError")}</span>
                <button onClick={() => void retryFinalize()} type="button">
                  {t("pages.editor.retrySave")}
                </button>
              </div>
            )}

            {(autosave.status === "conflict" || publishVersionConflict) && (
              <div className="page-draft-notice page-draft-notice-error" role="alert">
                <span>
                  {reloadError
                    ? t("pages.editor.reloadError")
                    : t("pages.editor.versionConflict")}
                </span>
                <button onClick={() => void reloadLatest()} type="button">
                  {t("pages.editor.reloadLatest")}
                </button>
              </div>
            )}

            {publishConflict && (
              <div className="page-draft-notice page-draft-notice-error" role="alert">
                <span>{t("pages.editor.titleConflict")}</span>
                <button
                  className="page-draft-conflict-action"
                  onClick={renameDraft}
                  type="button"
                >
                  {t("pages.editor.renameDraft")}
                </button>
              </div>
            )}

            {publishError && !publishConflict && !publishVersionConflict && (
              <div className="page-draft-notice page-draft-notice-error" role="alert">
                <span>{t("pages.editor.publishError")}</span>
                <button onClick={() => void retryFinalize()} type="button">
                  {t("pages.editor.retrySave")}
                </button>
              </div>
            )}
          </div>
          <input
            aria-label={t("pages.editor.titleLabel")}
            autoFocus
            className="page-draft-title"
            disabled={locked}
            onChange={(event) => {
              setTitle(event.target.value);
              setPublishConflict(false);
              setPublishError(null);
              setPublishVersionConflict(false);
            }}
            placeholder={t("pages.editor.titlePlaceholder")}
            ref={titleRef}
            value={title}
          />

          <textarea
            aria-label={t("pages.editor.contentLabel")}
            className="page-draft-content"
            disabled={locked}
            onChange={(event) => {
              setContent(event.target.value);
              setPublishConflict(false);
              setPublishError(null);
              setPublishVersionConflict(false);
            }}
            placeholder={t("pages.editor.contentPlaceholder")}
            value={content}
          />
        </div>
      </section>
      <PageInfoDrawer
        docked
        inspector
        open={inspectorOpen}
        onClose={() => setInspectorOpen(false)}
        returnFocusRef={inspectorToggleRef}
        title={t("pageInspector.info")}
        closeLabel={t("common.close")}
      >
        <div
          className="page-draft-inspector-info"
        >
          <dl className="page-draft-inspector-details">
            <div><dt>{t("pages.editor.titleLabel")}</dt><dd>{title.trim() || t("pages.editor.titlePlaceholder")}</dd></div>
            <div><dt>{t("pages.overview.reviewStatusLabel")}</dt><dd aria-live="polite" role="status">{autosaveLabel}</dd></div>
            <div><dt>{t("pages.editor.spaceLabel")}</dt><dd>{space || t("pages.editor.noSpace")}</dd></div>
          </dl>
        </div>
      </PageInfoDrawer>
      </>
    );
  },
);

export const PageDraftEditor = forwardRef<PageDraftEditorHandle, PageDraftEditorProps>(
  function PageDraftEditor({ draftId, ...props }, ref) {
    const { t } = useTranslation();
    const notifiedActivePageId = useRef<string | null>(null);
    const draftQuery = useQuery({
      queryKey: ["page-draft", draftId],
      queryFn: () => getPage(draftId!),
      enabled: Boolean(draftId),
      retry: false,
    });
    useEffect(() => {
      const page = draftQuery.data;
      if (!page || page.status !== "active" || notifiedActivePageId.current === page.id) return;
      notifiedActivePageId.current = page.id;
      const projectionIssue = page.projection_status === "pending" && page.projection_error
        ? { expectedVersion: page.version - 1 }
        : undefined;
      props.onPublished(page.id, projectionIssue);
    }, [draftQuery.data, props.onPublished]);

    if (draftId && draftQuery.isPending) {
      return (
        <div className="page-draft-load-state">
          <p>{t("pages.editor.loading")}</p>
          <WorkspaceBackButton onClick={props.onBack} type="button">{t("pages.editor.back")}</WorkspaceBackButton>
        </div>
      );
    }
    if (draftId && draftQuery.isError) {
      return (
        <div className="page-draft-load-state page-draft-notice-error" role="alert">
          <p>{t("pages.editor.loadError")}</p>
          <button onClick={() => void draftQuery.refetch()} type="button">
            {t("pages.editor.tryAgain")}
          </button>
          <WorkspaceBackButton onClick={props.onBack} type="button">{t("pages.editor.back")}</WorkspaceBackButton>
        </div>
      );
    }
    if (draftId && !draftQuery.data) {
      return (
        <div className="page-draft-load-state">
          <p>{t("pages.editor.missing")}</p>
          <WorkspaceBackButton onClick={props.onBack} type="button">{t("pages.editor.back")}</WorkspaceBackButton>
        </div>
      );
    }
    if (draftId && draftQuery.data?.status === "active") {
      return (
        <div className="page-draft-load-state">
          <p>{t("pages.editor.loading")}</p>
          <WorkspaceBackButton onClick={props.onBack} type="button">{t("pages.editor.back")}</WorkspaceBackButton>
        </div>
      );
    }
    if (draftId && draftQuery.data?.status !== "draft") {
      return (
        <div className="page-draft-load-state">
          <p>{t("pages.editor.notDraft")}</p>
          <WorkspaceBackButton onClick={props.onBack} type="button">{t("pages.editor.back")}</WorkspaceBackButton>
        </div>
      );
    }

    const initialPage = draftId ? draftQuery.data ?? null : null;
    return (
      <HydratedPageDraftEditor
        {...props}
        initialPage={initialPage}
        ref={ref}
      />
    );
  },
);
