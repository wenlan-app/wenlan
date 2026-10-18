// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useContext, useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from "react";
import { createPortal } from "react-dom";
import { ArrowsIn, ArrowsOut, X } from "@phosphor-icons/react";
import { WorkspaceNoteGroupContext, WorkspacePaneHostContext } from "../navigation/WorkspacePaneHost";
import { clampRightSidebarWidth, readRightSidebarWidth, writeRightSidebarWidth } from "../navigation/rightSidebarPreferences";
import { RightSidebarResizeHandle } from "../navigation/RightSidebarResizeHandle";
import { useReportWorkspaceDemand } from "../navigation/workspaceDemand";
import "./PageInfoDrawer.css";

interface PageInfoDrawerProps {
  open: boolean;
  /** Automatically visible docked context should leave focus in the reader. */
  focusOnOpen?: boolean;
  /**
   * The control that opens this pane. Closing returns focus here, because a
   * pane that opened by default has no focused trigger of its own to go back to.
   */
  returnFocusRef?: RefObject<HTMLElement | null>;
  /** A surface can start compact while preserving an explicitly resized width. */
  defaultWidth?: number;
  headerContent?: ReactNode;
  headerActions?: ReactNode;
  inspector?: boolean;
  docked?: boolean;
  expanded?: boolean;
  expandedHost?: HTMLElement | null;
  onExpandedChange?: (expanded: boolean) => void;
  expandLabel?: string;
  restoreLabel?: string;
  variant?: "info" | "canvas";
  onClose: () => void;
  title: string;
  children: ReactNode;
  closeLabel: string;
}

const FOCUSABLE = 'summary, a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

function focusableControls(panel: HTMLElement): HTMLElement[] {
  return Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE)).filter((element) => {
    if (element.tabIndex < 0 || element.matches(":disabled") || element.closest("[hidden], [inert]")) return false;
    for (let ancestor: HTMLElement | null = element; ancestor && ancestor !== panel; ancestor = ancestor.parentElement) {
      if (ancestor.tagName === "DETAILS" && !ancestor.hasAttribute("open")) {
        const summary = ancestor.querySelector(":scope > summary");
        if (!summary?.contains(element)) return false;
      }
      const style = getComputedStyle(ancestor);
      if (style.display === "none" || style.visibility === "hidden") return false;
    }
    return true;
  }).sort((left, right) => {
    const leftOrder = left.tabIndex > 0 ? left.tabIndex : Infinity;
    const rightOrder = right.tabIndex > 0 ? right.tabIndex : Infinity;
    if (leftOrder !== rightOrder) return leftOrder - rightOrder;
    return left.compareDocumentPosition(right) & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : 1;
  });
}

// The caller's named opener wins over the element focused when the pane
// appeared: a pane that opened on its own captured an incidental element.
function returnTarget(named: RefObject<HTMLElement | null> | undefined, trigger: HTMLElement | null) {
  if (named?.current?.isConnected) return named.current;
  return trigger?.isConnected ? trigger : null;
}

export default function PageInfoDrawer({ open, focusOnOpen = true, returnFocusRef, defaultWidth: preferredWidth, onClose, title, children, closeLabel, headerContent, headerActions, inspector = false, docked = false, expanded = false, expandedHost, onExpandedChange, expandLabel, restoreLabel, variant = "info" }: PageInfoDrawerProps) {
  const workspacePaneHost = useContext(WorkspacePaneHostContext);
  const noteGroup = useContext(WorkspaceNoteGroupContext);
  const [groupWidth, setGroupWidth] = useState(0);
  const [wide, setWide] = useState(() => typeof window.matchMedia === "function" && window.matchMedia("(min-width: 1100px)").matches);
  useEffect(() => {
    if (noteGroup) return;
    if (typeof window.matchMedia !== "function") return;
    const media = window.matchMedia("(min-width: 1100px)");
    const update = () => {
      setWide(media.matches);
      if (!media.matches) onExpandedChange?.(false);
    };
    media.addEventListener?.("change", update);
    return () => media.removeEventListener?.("change", update);
  }, [onExpandedChange, noteGroup]);
  const canExpand = inspector && onExpandedChange !== undefined && expandedHost !== undefined && expandLabel !== undefined && restoreLabel !== undefined;
  const groupScoped = noteGroup !== null;
  const groupDocked = groupScoped && groupWidth >= 720;
  const expandedDestination = groupScoped ? noteGroup.element : expandedHost;
  const expandedWorkspace = canExpand && expanded && (groupScoped ? noteGroup.element !== null : wide && expandedHost !== null);
  const nonModal = groupScoped || (docked && wide);
  const hosted = groupScoped
    ? false
    : nonModal && workspacePaneHost !== null && !expandedWorkspace;
  // Group inspectors stay anchored to one frame at every width. Reserving the
  // same space below the header avoids reparenting a live map during resize.
  const groupOverlay = groupScoped && docked && !expandedWorkspace;
  const portalMountRef = useRef<HTMLDivElement | null>(null);
  const workspaceToggleRef = useRef<HTMLButtonElement>(null);
  const restoreWorkspaceFocusRef = useRef(false);
  if ((canExpand || groupScoped) && portalMountRef.current === null) {
    portalMountRef.current = document.createElement("div");
    // Keep the stable portal target out of layout so the hosted pane still
    // receives the workspace host's full height.
    portalMountRef.current.style.display = "contents";
  }
  const portalMount = portalMountRef.current;
  useEffect(() => () => { portalMount?.remove(); }, [portalMount]);
  const portalDestination = expandedWorkspace
    ? expandedDestination
    : hosted ? workspacePaneHost : groupOverlay ? noteGroup?.element : document.body;
  useLayoutEffect(() => {
    if ((!canExpand && !groupScoped) || !portalMount) return;
    if (!open) {
      portalMount.remove();
      return;
    }
    // appendChild moves this same mount node, keeping the portal subtree and
    // its React state alive while the workspace changes parents.
    portalDestination?.appendChild(portalMount);
    if (restoreWorkspaceFocusRef.current) {
      restoreWorkspaceFocusRef.current = false;
      workspaceToggleRef.current?.focus({ preventScroll: true });
    }
  }, [canExpand, groupScoped, open, portalDestination, portalMount]);
  const [committedWidth, setCommittedWidth] = useState<number | undefined>(() => readRightSidebarWidth(noteGroup?.id));
  const [previewWidth, setPreviewWidth] = useState<number | null>(null);
  const [maxWidth, setMaxWidth] = useState(600);
  const titleId = useId();
  const panelRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const closeFromResize = () => {
    const active = document.activeElement;
    const target = returnTarget(returnFocusRef, triggerRef.current);
    if (active instanceof HTMLElement && active.closest(".sidebar-resize-handle--right") && target) {
      target.focus({ preventScroll: true });
    }
    onCloseRef.current();
  };

  useEffect(() => {
    if (!groupScoped) return;
    setCommittedWidth(readRightSidebarWidth(noteGroup?.id));
    setPreviewWidth(null);
  }, [groupScoped, noteGroup?.id]);

  // Metadata stays secondary to the reading lane; maps can use a wider pane.
  const defaultWidth = preferredWidth ?? (variant === "canvas"
    ? Math.max(360, Math.min(460, Math.round(window.innerWidth * 0.32)))
    : 320);
  const baseWidth = committedWidth ?? defaultWidth;
  const displayWidth = previewWidth === 0 ? 0 : clampRightSidebarWidth(previewWidth ?? baseWidth, maxWidth);
  // The shell weighs the preferred width, not the width this window allows,
  // so folding the rail to make room does not change the answer.
  useReportWorkspaceDemand("info", open && !expandedWorkspace && (hosted || groupOverlay) ? clampRightSidebarWidth(baseWidth, 600) : 0);
  const effectiveGroupWidth = groupWidth || noteGroup?.element?.getBoundingClientRect().width || 0;
  // Keep enough room for a readable selected note title, its close/menu
  // controls, the new-note button, and the global search action.
  const minimumNoteHeaderLane = typeof window !== "undefined"
    && typeof window.matchMedia === "function"
    && window.matchMedia("(pointer: coarse)").matches
    ? 228
    : 204;
  // Keep the note and its inspector as two visible regions at every width.
  // A narrow group shares its space; only the explicit expand action may
  // cover the note. The header and body use this same boundary.
  const minimumNoteContentWidth = Math.max(minimumNoteHeaderLane, Math.min(320, effectiveGroupWidth / 2));
  const groupOverlayWidth = Math.min(displayWidth, Math.max(0, effectiveGroupWidth - minimumNoteContentWidth));
  const groupOverlayCssWidth = `min(${displayWidth}px, max(0px, calc(100% - max(${minimumNoteHeaderLane}px, min(320px, 50%)))))`;
  const groupOverlayBodyReserved = groupOverlay;
  const expandedHeaderWidth = Math.min(320, Math.max(0, effectiveGroupWidth - minimumNoteHeaderLane));
  const groupHeaderReserveWidth = groupOverlay
    ? groupOverlayCssWidth
    : groupScoped && expandedWorkspace ? `${expandedHeaderWidth}px` : null;

  useLayoutEffect(() => {
    const frame = groupScoped ? noteGroup?.element : null;
    if (!frame) return;
    const property = "--note-group-inspector-header-width";
    const previous = frame.style.getPropertyValue(property);
    if (groupHeaderReserveWidth === null) frame.style.removeProperty(property);
    else frame.style.setProperty(property, groupHeaderReserveWidth);
    return () => {
      if (previous) frame.style.setProperty(property, previous);
      else frame.style.removeProperty(property);
    };
  }, [groupScoped, noteGroup?.element, groupHeaderReserveWidth]);

  const measureWidthLimit = useCallback(() => {
    if (groupScoped) {
      const width = noteGroup.element?.getBoundingClientRect().width ?? 0;
      setGroupWidth(current => current === width ? current : width);
      const available = groupDocked ? Math.floor(width - 360) : 320;
      const next = Math.min(600, Math.max(320, available));
      setMaxWidth(current => current === next ? current : next);
      return;
    }
    if (!workspacePaneHost || !hosted) return;
    const shell = workspacePaneHost.closest<HTMLElement>(".memory-shell");
    if (!shell) return;
    const shellWidth = shell.getBoundingClientRect().width || shell.clientWidth;
    const sidebar = shell.querySelector<HTMLElement>(".memory-sidebar");
    const sidebarWidth = sidebar?.getBoundingClientRect().width ?? 0;
    const hostStyle = getComputedStyle(workspacePaneHost);
    const hostPadding = (Number.parseFloat(hostStyle.paddingLeft) || 0) + (Number.parseFloat(hostStyle.paddingRight) || 0);
    const available = Math.floor(shellWidth - sidebarWidth - 480 - hostPadding);
    const next = Math.min(600, Math.max(320, available));
    setMaxWidth(current => current === next ? current : next);
  }, [workspacePaneHost, hosted, groupScoped, noteGroup, groupDocked]);

  useEffect(() => {
    if (groupScoped) {
      const groupElement = noteGroup.element;
      if (!groupElement) return;
      measureWidthLimit();
      // ResizeObserver runs before paint. Commit pane geometry in the next
      // frame so reflowing the note does not resize descendants in its delivery loop.
      let frame: number | null = null;
      const scheduleMeasure = () => {
        if (frame !== null) return;
        frame = requestAnimationFrame(() => { frame = null; measureWidthLimit(); });
      };
      const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(scheduleMeasure);
      observer?.observe(groupElement);
      return () => {
        observer?.disconnect();
        if (frame !== null) cancelAnimationFrame(frame);
      };
    }
    if (!hosted || !workspacePaneHost) return;
    measureWidthLimit();
    const shell = workspacePaneHost.closest<HTMLElement>(".memory-shell");
    const sidebar = shell?.querySelector<HTMLElement>(".memory-sidebar");
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measureWidthLimit);
    if (shell) observer?.observe(shell);
    if (sidebar) observer?.observe(sidebar);
    observer?.observe(workspacePaneHost);
    window.addEventListener("resize", measureWidthLimit);
    return () => {
      observer?.disconnect();
      window.removeEventListener("resize", measureWidthLimit);
    };
  }, [hosted, workspacePaneHost, measureWidthLimit, groupScoped, noteGroup?.element]);

  const commitWidth = (width: number) => {
    const sanitized = clampRightSidebarWidth(width, maxWidth);
    setCommittedWidth(sanitized);
    writeRightSidebarWidth(sanitized, noteGroup?.id);
    setPreviewWidth(null);
  };

  useEffect(() => {
    if (!open) return;
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    triggerRef.current = trigger;
    if (!groupScoped && (!nonModal || focusOnOpen)) closeRef.current?.focus();
    // Only a reader who was working in the pane is sent to its opener. Effect
    // re-runs while the pane stays open must leave the reader's focus alone.
    let focusInside = !!panelRef.current?.contains(document.activeElement);
    const trackFocus = (event: FocusEvent) => {
      focusInside = event.target instanceof Node && !!panelRef.current?.contains(event.target);
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (groupScoped && !(event.target instanceof Node && noteGroup?.element?.contains(event.target))) return;
      if (!groupScoped && nonModal && !(event.target instanceof Node && panelRef.current?.contains(event.target))) return;
      // Map node editors, context menus, and shortcut sheets get the first
      // chance to handle Escape. A document capture listener closes the pane
      // before those nested controls can cancel their own operation.
      if (event.defaultPrevented || event.isComposing) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (groupScoped || nonModal || event.key !== "Tab" || !panelRef.current) return;
      const controls = focusableControls(panelRef.current);
      const first = controls[0];
      const last = controls[controls.length - 1];
      const active = document.activeElement;
      if (inspector && controls.length) {
        // Safari can skip native buttons in the system's default Tab mode.
        // Keep every inspector control reachable within this modal pane.
        const index = controls.findIndex((control) => control === active);
        const adjacent = index >= 0
          ? controls[(index + (event.shiftKey ? -1 : 1) + controls.length) % controls.length]
          : (event.shiftKey ? [...controls].reverse() : controls).find((control) =>
              active instanceof Node && !!(active.compareDocumentPosition(control) &
                (event.shiftKey ? Node.DOCUMENT_POSITION_PRECEDING : Node.DOCUMENT_POSITION_FOLLOWING)),
            );
        event.preventDefault();
        (adjacent ?? (event.shiftKey ? last : first))?.focus();
        return;
      }
      const inside = active instanceof HTMLElement && (
        controls.includes(active) ||
        (active.getAttribute("role") === "tab" && active.closest('[role="tablist"]') !== null && panelRef.current.contains(active))
      );
      if (event.shiftKey && (!inside || active === first)) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && (!inside || active === last)) {
        event.preventDefault();
        first?.focus();
      }
    };
    const containFocus = (event: FocusEvent) => {
      const target = event.target;
      // A portalled preview remains part of the source reader that owns its
      // anchor. Other panes and unrelated portals still stay outside this modal.
      const previewOwner = target instanceof Element
        ? target.closest("[data-reference-preview]")?.getAttribute("data-reference-owner") : null;
      if (previewOwner && previewOwner === panelRef.current?.getAttribute("aria-labelledby")) return;
      if (target instanceof Node && !panelRef.current?.contains(target)) closeRef.current?.focus();
    };
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("focusin", trackFocus);
    if (!groupScoped && !nonModal) document.addEventListener("focusin", containFocus);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("focusin", containFocus);
      document.removeEventListener("focusin", trackFocus);
      const target = returnTarget(focusInside ? returnFocusRef : undefined, trigger);
      if (!groupScoped && target && (!nonModal || document.activeElement === document.body || panelRef.current?.contains(document.activeElement))) target.focus();
    };
  }, [open, nonModal, inspector, groupScoped, noteGroup, focusOnOpen, returnFocusRef]);

  if (!open) return null;

  const hostClass = expandedWorkspace
    ? ` page-info-drawer-expanded-workspace${groupScoped ? " page-info-drawer-expanded-group" : ""}`
    : hosted ? ` page-info-drawer-hosted${variant === "canvas" ? " page-info-drawer-hosted--canvas" : ""}${inspector ? " page-info-drawer-hosted--inspector" : ""}` : "";
  const bodyReservedOverlayClass = groupOverlayBodyReserved ? " page-info-drawer-group-overlay-body-reserved" : "";
  const overlayStyle = hosted
    ? { width: `${displayWidth}px` }
    : groupOverlay
      ? { width: "100%", "--note-inspector-panel-width": groupOverlayCssWidth } as CSSProperties
      : groupScoped && expandedWorkspace
        ? { width: "100%", "--note-inspector-header-width": `${expandedHeaderWidth}px` } as CSSProperties
        : undefined;
  const panelStyle = hosted
    ? { width: `${displayWidth}px` }
    : groupOverlay
      ? { width: groupOverlayCssWidth } as CSSProperties
      : expandedWorkspace
        ? { width: "100%", ...(groupScoped ? { "--note-inspector-header-width": `${expandedHeaderWidth}px` } : {}) } as CSSProperties
        : undefined;
  const drawer = (
    <div className={`page-info-drawer-overlay${nonModal ? " page-info-drawer-docked" : ""}${groupOverlay ? ` page-info-drawer-group-overlay${bodyReservedOverlayClass}` : ""}${hostClass}`} style={overlayStyle} onClick={(event) => {
      if (!groupScoped && event.target === event.currentTarget) onClose();
      }}>
      {(hosted || groupOverlay) && <RightSidebarResizeHandle
        enabled={open}
        maxWidth={maxWidth}
        onClose={closeFromResize}
        onCommit={commitWidth}
        onPreview={setPreviewWidth}
        width={groupOverlay ? groupOverlayWidth : displayWidth}
      />}
      <aside
        aria-labelledby={titleId}
        aria-modal={nonModal ? undefined : true}
        className={`page-info-drawer${variant === "canvas" ? " page-info-drawer--canvas" : ""}${inspector ? " page-info-drawer--inspector" : ""}${groupOverlay ? " page-info-drawer-group-overlay-panel" : ""}`}
        ref={panelRef}
        role={nonModal ? "complementary" : "dialog"}
        style={panelStyle}
      >
        <header className="page-info-drawer-header" data-tauri-drag-region={hosted ? "" : undefined}>
          {headerContent != null ? (
            <>
              <h2 className="sr-only" data-tauri-drag-region={hosted ? "" : undefined} id={titleId}>{title}</h2>
              {headerContent}
            </>
          ) : (
            <h2 data-tauri-drag-region={hosted ? "" : undefined} id={titleId} title={title}>{title}</h2>
          )}
          {headerActions}
          {canExpand && (groupScoped || wide) && (
            <button aria-label={expandedWorkspace ? restoreLabel : expandLabel}
              className="page-info-drawer-workspace-toggle" onClick={() => {
                restoreWorkspaceFocusRef.current = true;
                onExpandedChange?.(!expandedWorkspace);
              }} ref={workspaceToggleRef}
              title={expandedWorkspace ? restoreLabel : expandLabel} type="button">
              {expandedWorkspace ? <ArrowsIn aria-hidden="true" size={18} /> : <ArrowsOut aria-hidden="true" size={18} />}
            </button>
          )}
          <button aria-label={closeLabel} className="page-info-drawer-close" onClick={onClose} ref={closeRef} type="button">
            <X aria-hidden="true" size={18} />
          </button>
        </header>
        <div className="page-info-drawer-content">{children}</div>
      </aside>
    </div>
  );
  return createPortal(drawer, portalMount ?? (hosted ? workspacePaneHost! : (groupOverlay ? noteGroup!.element! : document.body)));
}
