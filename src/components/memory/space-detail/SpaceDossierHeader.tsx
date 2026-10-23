// SPDX-License-Identifier: AGPL-3.0-only
import { WorkspaceBackButton } from "../navigation/WorkspaceNavigation";
import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { Space } from "../../../lib/tauri";
import type { SpaceDetailCopy } from "./copy";
import { InlineTextField } from "../editor/InlineTextField";
import { useSpaceIdentityAutosave } from "./useSpaceIdentityAutosave";

type SpaceIdentityActions = {
  readonly onBack: () => void;
  readonly onCreatePage: () => void;
  readonly onDelete: () => void;
  readonly onKeep: () => void;
  readonly onReviewAll?: () => void;
  readonly onSaveIdentity: (value: SpaceIdentityValue) => Promise<void> | void;
};

export type SpaceIdentityValue = {
  readonly description: string;
  readonly name: string;
};

type SpaceDossierHeaderProps = {
  readonly actions: SpaceIdentityActions;
  readonly copy: SpaceDetailCopy;
  readonly mutationError: boolean;
  readonly space: Space;
  /** Shown before New page, such as the GitHub button. */
  readonly leadingAction?: ReactNode;
  readonly onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
};

export function SpaceDossierHeader({
  actions,
  copy,
  mutationError,
  space,
  leadingAction,
  onRegisterFlush,
}: SpaceDossierHeaderProps) {
  const { t } = useTranslation();
  const { draft, status, change, flush, focus, blur, compositionStart, compositionEnd } = useSpaceIdentityAutosave(
    { name: space.name, description: space.description ?? "" }, actions.onSaveIdentity,
  );
  const registeredFlush = useCallback(() => flush(), [flush]);
  useEffect(() => {
    onRegisterFlush?.(registeredFlush);
    return () => onRegisterFlush?.(null);
  }, [onRegisterFlush, registeredFlush]);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const actionsRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggerRef = useRef<HTMLButtonElement>(null);
  const menuInitialFocusRef = useRef<"first" | "last">("first");
  useEffect(() => {
    if (!menuOpen) return;
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')
        ?? [],
    );
    items[menuInitialFocusRef.current === "first" ? 0 : items.length - 1]?.focus();
  }, [menuOpen]);

  useEffect(() => {
    if (!menuOpen) return;
    const closeOutside = (event: PointerEvent) => {
      if (!actionsRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    document.addEventListener("pointerdown", closeOutside);
    return () => document.removeEventListener("pointerdown", closeOutside);
  }, [menuOpen]);

  const handleEditorKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>, title = false) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "Escape" || (event.key === "Enter" && (title || event.metaKey || event.ctrlKey))) {
      event.preventDefault();
      event.stopPropagation();
      void flush();
      if (event.key === "Escape") event.currentTarget.blur();
    }
  };

  const handleMenuKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setMenuOpen(false);
      menuTriggerRef.current?.focus();
      return;
    }

    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'),
    );
    if (items.length === 0) return;
    const currentIndex = items.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Home") {
      items[0].focus();
    } else if (event.key === "End") {
      items[items.length - 1].focus();
    } else if (event.key === "ArrowDown") {
      items[(currentIndex + 1) % items.length].focus();
    } else {
      items[(currentIndex - 1 + items.length) % items.length].focus();
    }
  };

  const openMenu = (initialFocus: "first" | "last") => {
    menuInitialFocusRef.current = initialFocus;
    setMenuOpen(true);
  };

  const handleMenuTriggerKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    event.stopPropagation();
    openMenu(event.key === "ArrowDown" ? "first" : "last");
  };

  return (
    <>
      <WorkspaceBackButton className="space-dossier-parent" onClick={actions.onBack} type="button">
        <svg aria-hidden="true" viewBox="0 0 24 24"><path d="M19 12H5M12 19l-7-7 7-7" /></svg>
        <span>{copy.backToSpaces}</span>
      </WorkspaceBackButton>

      {space.suggested && (
        <div className="space-dossier-suggested">
          <span>{copy.suggestedLabel}</span>
          <div>
            <button className="space-dossier-suggestion-action space-dossier-suggestion-keep" onClick={actions.onKeep} type="button">{copy.keep}</button>
            <button className="space-dossier-suggestion-action space-dossier-suggestion-discard" onClick={actions.onDelete} type="button">{copy.discard}</button>
          </div>
        </div>
      )}

      <header className="space-dossier-header">
        <div className="space-dossier-title-row">
          <div className="space-dossier-title-block">
            <h1 aria-label={draft.name}>
              <InlineTextField
                aria-label={t("spaces.overview.nameLabel")}
                className="space-dossier-title-input"
                value={draft.name}
                onChange={(event) => change({ ...draft, name: event.target.value })}
                onCompositionStart={compositionStart}
                onCompositionEnd={compositionEnd}
                onFocus={focus}
                onBlur={blur}
                onKeyDown={(event) => handleEditorKeyDown(event, true)}
              />
            </h1>
            <InlineTextField
              aria-label={copy.editDescription}
              className="space-dossier-description"
              value={draft.description}
              placeholder={copy.descriptionPlaceholder}
              onChange={(event) => change({ ...draft, description: event.target.value })}
              onCompositionStart={compositionStart}
              onCompositionEnd={compositionEnd}
              onFocus={focus}
              onBlur={blur}
              onKeyDown={handleEditorKeyDown}
            />
          </div>

          {!space.suggested && (
            <div ref={actionsRef} className="space-dossier-actions">
              {leadingAction}
              <button
                className="page-create-action space-dossier-new-page"
                onClick={actions.onCreatePage}
                type="button"
              >
                {t("pages.overview.newPage")}
              </button>
              <button
                ref={menuTriggerRef}
                aria-expanded={menuOpen}
                aria-haspopup="menu"
                aria-label={t("spaces.overview.actionsFor", { name: space.name })}
                className="mem-icon-action space-dossier-overflow-trigger"
                onClick={() => {
                  if (menuOpen) {
                    setMenuOpen(false);
                  } else {
                    openMenu("first");
                  }
                }}
                onKeyDown={handleMenuTriggerKeyDown}
                type="button"
              >
                <svg aria-hidden="true" className="space-dossier-overflow-icon" viewBox="0 0 24 24"><circle cx="5" cy="12" r="1.4" /><circle cx="12" cy="12" r="1.4" /><circle cx="19" cy="12" r="1.4" /></svg>
              </button>
              {menuOpen && (
                <div
                  ref={menuRef}
                  className="mem-popover-surface space-dossier-menu"
                  onKeyDown={handleMenuKeyDown}
                  role="menu"
                >
                  {actions.onReviewAll && (
                    <button role="menuitem" type="button" onClick={() => {
                      setMenuOpen(false);
                      actions.onReviewAll?.();
                    }}>{t("home.reviewPageChanges")}</button>
                  )}
                  <button
                    className="space-dossier-menu-danger"
                    onClick={() => {
                      setConfirmingDelete(true);
                      setMenuOpen(false);
                    }}
                    role="menuitem"
                    type="button"
                  >
                    {copy.deleteSpace}
                  </button>
                </div>
              )}
            </div>
          )}
        </div>

        {confirmingDelete && (
          <div className="space-dossier-delete-confirmation">
            <span>{copy.deleteWarning}</span>
            <button aria-label={copy.confirmDeleteSpace} onClick={actions.onDelete} type="button">{copy.deleteSpace}</button>
            <button onClick={() => setConfirmingDelete(false)} type="button">{copy.cancel}</button>
          </div>
        )}

        {status === "saving" && <span className="sr-only" role="status">{t("pages.editor.saving")}</span>}
        {(mutationError || status === "error") && <div className="space-dossier-error" role="alert">
          <span>{copy.mutationError}</span>
          {status === "error" && <button className="space-dossier-text-action" type="button" onClick={() => void flush(true)}>{t("spaces.overview.retry")}</button>}
        </div>}

      </header>
    </>
  );
}
