// SPDX-License-Identifier: AGPL-3.0-only
// @refresh reset
// Rebind document-level drag listeners when this leaf component is hot-updated.
// Editors live outside this provider and keep their state.
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import type { NoteTabDrop } from "./noteTabDragContext";
import "./noteTabs.css";

type Props = { readonly onDrop: (drop: NoteTabDrop) => void; readonly disabled?: boolean };
type Insertion = { readonly group: NoteTabDrop["targetGroup"]; readonly beforeKey: string | null; readonly x: number; readonly y: number; readonly height: number; readonly kind?: "split" | "transfer"; readonly preview?: { readonly left: number; readonly top: number; readonly width: number; readonly height: number } };
type Gesture = {
  readonly sourceGroup: NoteTabDrop["sourceGroup"];
  readonly tabKey: string;
  readonly pointerId: number;
  readonly startX: number;
  readonly startY: number;
  readonly viewportWidth: number;
  readonly viewportHeight: number;
  readonly sourceButton: HTMLButtonElement;
  readonly sourceWrapper: HTMLElement;
  readonly sourceStrip: HTMLElement;
  readonly title: string;
  lastX: number;
  lastY: number;
  moved: boolean;
  insertion: Insertion | null;
};

const dragThreshold = 6;
const autoscrollEdge = 28;
const autoscrollStep = 12;

function visible(element: HTMLElement) {
  if (!element.isConnected || element.getClientRects().length === 0) return false;
  const bounds = element.getBoundingClientRect();
  if (bounds.width <= 0 || bounds.height <= 0) return false;
  const style = window.getComputedStyle(element);
  return style.display !== "none" && style.visibility !== "hidden";
}

function stripForGroup(group: NoteTabDrop["targetGroup"]) {
  return [...document.querySelectorAll<HTMLElement>(`.note-tabs[data-note-tab-group="${group}"]`)].find(strip => visible(strip) && !!frameForStrip(strip, group)) ?? null;
}

function groupForStrip(strip: HTMLElement): NoteTabDrop["targetGroup"] | null {
  const group = strip.dataset.noteTabGroup;
  return group === "primary" || group === "secondary" ? group : null;
}

function frameForStrip(strip: HTMLElement, group: NoteTabDrop["targetGroup"]) {
  const frame = strip.closest<HTMLElement>("[data-note-group-id]");
  return frame?.dataset.noteGroupId === group && visible(frame) ? frame : null;
}

function findInsertion(strip: HTMLElement, group: NoteTabDrop["targetGroup"], draggedKey: string, x: number, y: number): Insertion | null {
  const list = strip.querySelector<HTMLElement>(".note-tabs-list");
  const bounds = (list ?? strip).getBoundingClientRect();
  if (x < bounds.left || x > bounds.right || y < bounds.top || y > bounds.bottom) return null;
  const wrappers = [...strip.querySelectorAll<HTMLElement>(".note-tab[data-note-tab-key]")]
    .filter(wrapper => wrapper.dataset.noteTabKey !== draggedKey);
  let beforeKey: string | null = null;
  for (const wrapper of wrappers) {
    const rect = wrapper.getBoundingClientRect();
    if (x < rect.left + rect.width / 2) {
      beforeKey = wrapper.dataset.noteTabKey ?? null;
      break;
    }
  }
  const reference = beforeKey ? wrappers.find(wrapper => wrapper.dataset.noteTabKey === beforeKey) : null;
  const refBounds = reference?.getBoundingClientRect();
  const lastBounds = wrappers[wrappers.length - 1]?.getBoundingClientRect();
  return { group, beforeKey, x: Math.max(bounds.left, Math.min(bounds.right, refBounds?.left ?? lastBounds?.right ?? bounds.left)), y: bounds.top, height: bounds.height };
}

/** A shared inspector is physically outside both note frames but belongs to
 * the Wiki reading workspace. Treat it as the rightmost drop target: transfer
 * into an existing secondary group, or split the primary group when none is
 * open. There is no third destination in this workspace. */
function sharedWorkspaceInsertion(current: Gesture, root: HTMLElement, x: number, y: number): Insertion | null {
  const rootRect = root.getBoundingClientRect();
  if (x < rootRect.left || x > rootRect.right || y < rootRect.top || y > rootRect.bottom || current.sourceGroup !== "primary") return null;
  const primary = root.querySelector<HTMLElement>('[data-note-group-id="primary"]');
  if (!primary || !visible(primary)) return null;
  const secondary = root.querySelector<HTMLElement>('[data-note-group-id="secondary"]');
  if (secondary && !secondary.closest("[hidden]") && visible(secondary)) {
    const bounds = secondary.getBoundingClientRect();
    return { group: "secondary", beforeKey: null, x: bounds.left, y: bounds.top, height: bounds.height, kind: "transfer",
      preview: { left: bounds.left + 8, top: bounds.top + 8, width: Math.max(0, bounds.width - 16), height: Math.max(0, bounds.height - 16) } };
  }

  // The root's measured box is the right target geometry; use the Wiki
  // container only for the same stacked-vs-side-by-side breakpoint as CSS.
  const mainWidth = root.closest<HTMLElement>(".memory-main-content--wiki")?.clientWidth || rootRect.width;
  const stacked = mainWidth <= 640;
  const left = stacked ? rootRect.left : rootRect.left + rootRect.width / 2;
  const top = stacked ? rootRect.top + rootRect.height / 2 : rootRect.top;
  const width = stacked ? rootRect.width : rootRect.width / 2;
  const height = stacked ? rootRect.height / 2 : rootRect.height;
  return { group: "secondary", beforeKey: null, x: left, y: top, height, kind: "split",
    preview: { left: left + 8, top: top + 8, width: Math.max(0, width - 16), height: Math.max(0, height - 16) } };
}

/** A visible note-group frame is a generous target, including its own inspector. */
function documentInsertion(current: Gesture, x: number, y: number, point: Element | null): Insertion | null {
  if (!point) return null;
  if (point.closest('[role="menu"]')) return null;
  const sharedSurface = point.closest<HTMLElement>(".wiki-shared-tools-rail, .page-info-drawer-group-overlay, .page-info-drawer-expanded-group");
  const sharedRoot = sharedSurface?.closest<HTMLElement>(".wiki-workspace-reading");
  if (sharedRoot && visible(sharedRoot)) {
    if (point.closest('[role="dialog"]')) return null;
    return sharedWorkspaceInsertion(current, sharedRoot, x, y);
  }
  const frame = point.closest<HTMLElement>("[data-note-group-id]");
  const group = frame?.dataset.noteGroupId;
  if (!frame || !visible(frame) || (group !== "primary" && group !== "secondary")) return null;
  const dialog = point.closest<HTMLElement>('[role="dialog"]');
  if (dialog) {
    const inspector = dialog.classList.contains("page-info-drawer")
      ? dialog.closest<HTMLElement>(".page-info-drawer-hosted, .page-info-drawer-group-overlay, .page-info-drawer-expanded-group")
      : null;
    if (!inspector || inspector.closest("[data-note-group-id]") !== frame) return null;
  }
  const frameRect = frame.getBoundingClientRect();
  if (x < frameRect.left || x > frameRect.right || y < frameRect.top || y > frameRect.bottom) return null;
  if (group !== current.sourceGroup) {
    return { group, beforeKey: null, x: frameRect.left, y: frameRect.top, height: frameRect.height, kind: "transfer",
      preview: { left: frameRect.left + 8, top: frameRect.top + 8, width: Math.max(0, frameRect.width - 16), height: Math.max(0, frameRect.height - 16) } };
  }
  if (group !== "primary" || stripForGroup("secondary")) return null;
  const width = Math.min(280, Math.max(96, frameRect.width * 0.3));
  const content = frame.querySelector<HTMLElement>(".note-group-content");
  const contentRect = content?.getBoundingClientRect();
  const inFrameSplitZone = x >= frameRect.right - width;
  const inLegacyBodySplitZone = !!content && visible(content) && !!contentRect
    && x >= contentRect.right - Math.min(280, Math.max(96, contentRect.width * 0.3))
    && x <= contentRect.right && y >= contentRect.top && y <= contentRect.bottom;
  if (!inFrameSplitZone && !inLegacyBodySplitZone) return null;
  // Preview the actual future group, including its header. Compact layouts stack.
  const bounds = frameRect;
  const mainWidth = frame.closest<HTMLElement>(".memory-main-content--wiki")?.clientWidth ?? bounds.width;
  const stacked = mainWidth <= 640;
  const left = stacked ? bounds.left : bounds.left + bounds.width / 2;
  const top = stacked ? bounds.top + bounds.height / 2 : bounds.top;
  const previewWidth = stacked ? bounds.width : bounds.width / 2;
  const height = stacked ? bounds.height / 2 : bounds.height;
  return { group: "secondary", beforeKey: null, x: left, y: top, height, kind: "split",
    preview: { left: left + 8, top: top + 8, width: Math.max(0, previewWidth - 16), height: Math.max(0, height - 16) } };
}

export function NoteTabDragProvider({ onDrop, disabled = false }: Props) {
  const { t } = useTranslation();
  const gesture = useRef<Gesture | null>(null);
  const suppressedClick = useRef<HTMLButtonElement | null>(null);
  const dropRef = useRef(onDrop);
  dropRef.current = onDrop;
  const disabledRef = useRef(disabled);
  disabledRef.current = disabled;
  const [overlay, setOverlay] = useState<{ title: string; x: number; y: number; insertion: Insertion | null } | null>(null);

  useEffect(() => {
    let frameId: number | null = null;
    const cancel = (suppressClick = true) => {
      const current = gesture.current;
      if (!current) return;
      if (suppressClick && current.moved) suppressedClick.current = current.sourceButton;
      delete current.sourceWrapper.dataset.dragging;
      document.body.classList.remove("note-tab-dragging");
      gesture.current = null;
      if (frameId !== null) cancelAnimationFrame(frameId);
      frameId = null;
      setOverlay(null);
      try {
        if (current.sourceButton.hasPointerCapture(current.pointerId)) current.sourceButton.releasePointerCapture(current.pointerId);
      } catch { /* Pointer capture may already have been released by the browser. */ }
    };
    const onPointerDown = (event: PointerEvent) => {
      if (gesture.current) return;
      const target = event.target;
      if (!(target instanceof Element) || !target.closest(".memory-shell")) return;
      suppressedClick.current = null;
      if (disabledRef.current || event.button !== 0 || event.pointerType === "touch") return;
      const button = target.closest<HTMLButtonElement>('.note-tabs[data-note-tab-group] [role="tab"]');
      const wrapper = button?.closest<HTMLElement>(".note-tab[data-note-tab-key]");
      const strip = button?.closest<HTMLElement>(".note-tabs[data-note-tab-group]");
      const group = strip && groupForStrip(strip);
      const tabKey = wrapper?.dataset.noteTabKey;
      if (!button || !wrapper || !strip || !group || !tabKey || !visible(strip) || !frameForStrip(strip, group)) return;
      gesture.current = {
        sourceGroup: group, tabKey, pointerId: event.pointerId,
        startX: event.clientX, startY: event.clientY, viewportWidth: innerWidth, viewportHeight: innerHeight, sourceButton: button,
        sourceWrapper: wrapper, sourceStrip: strip,
        title: button.textContent?.trim() ?? "", lastX: event.clientX, lastY: event.clientY, moved: false, insertion: null,
      };
      try { button.setPointerCapture(event.pointerId); } catch { /* Window listeners still track browsers without capture support. */ }
    };
    const updateDrag = (current: Gesture, x: number, y: number) => {
      const point = document.elementFromPoint(x, y);
      const targetStrip = point?.closest<HTMLElement>(".note-tabs[data-note-tab-group]") ?? null;
      let insertion: Insertion | null = null;
      if (targetStrip && visible(targetStrip)) {
        const group = groupForStrip(targetStrip);
        if (group && frameForStrip(targetStrip, group)) insertion = findInsertion(targetStrip, group, current.tabKey, x, y);
      }
      insertion ??= documentInsertion(current, x, y, point);
      const targetGroup = targetStrip && visible(targetStrip) ? groupForStrip(targetStrip) : null;
      const scrollStrip = targetStrip && targetGroup && frameForStrip(targetStrip, targetGroup) ? targetStrip : null;
      const list = scrollStrip?.querySelector<HTMLElement>(".note-tabs-list");
      let keepScrolling = false;
      if (list && scrollStrip && list.scrollWidth > list.clientWidth) {
        const rect = list.getBoundingClientRect();
        const localX = x - rect.left;
        if (localX < autoscrollEdge && list.scrollLeft > 0) { list.scrollLeft -= autoscrollStep; keepScrolling = true; }
        else if (localX > rect.width - autoscrollEdge && list.scrollLeft + list.clientWidth < list.scrollWidth) { list.scrollLeft += autoscrollStep; keepScrolling = true; }
        const validTargetGroup = groupForStrip(scrollStrip);
        if (validTargetGroup) insertion = findInsertion(scrollStrip, validTargetGroup, current.tabKey, x, y);
      }
      current.insertion = insertion;
      setOverlay({ title: current.title, x, y, insertion });
      return keepScrolling;
    };
    const scheduleScroll = () => {
      if (frameId !== null) return;
      frameId = requestAnimationFrame(() => {
        frameId = null;
        const current = gesture.current;
        if (current?.moved && updateDrag(current, current.lastX, current.lastY)) scheduleScroll();
      });
    };
    const onPointerMove = (event: PointerEvent) => {
      const current = gesture.current;
      if (!current || current.pointerId !== event.pointerId) return;
      if (disabledRef.current || current.viewportWidth !== innerWidth || current.viewportHeight !== innerHeight) { cancel(); return; }
      current.lastX = event.clientX;
      current.lastY = event.clientY;
      if (!current.moved && Math.hypot(event.clientX - current.startX, event.clientY - current.startY) < dragThreshold) return;
      if (!current.moved) {
        current.moved = true;
        current.sourceWrapper.dataset.dragging = "true";
        document.body.classList.add("note-tab-dragging");
        suppressedClick.current = current.sourceButton;
      }
      event.preventDefault();
      if (updateDrag(current, event.clientX, event.clientY)) scheduleScroll();
    };
    const onPointerUp = (event: PointerEvent) => {
      const current = gesture.current;
      if (!current || current.pointerId !== event.pointerId) return;
      if (disabledRef.current || current.viewportWidth !== innerWidth || current.viewportHeight !== innerHeight) { cancel(); return; }
      if (current.moved) {
        const point = document.elementFromPoint(event.clientX, event.clientY);
        const strip = point?.closest<HTMLElement>(".note-tabs[data-note-tab-group]") ?? null;
        let insertion = strip && visible(strip)
          ? (() => { const group = groupForStrip(strip); return group && frameForStrip(strip, group) ? findInsertion(strip, group, current.tabKey, event.clientX, event.clientY) : null; })()
          : null;
        insertion ??= documentInsertion(current, event.clientX, event.clientY, point);
        const sourceStillValid = current.sourceButton.isConnected && current.sourceWrapper.isConnected && current.sourceStrip.isConnected
          && current.sourceWrapper.dataset.noteTabKey === current.tabKey && groupForStrip(current.sourceStrip) === current.sourceGroup;
        if (!disabledRef.current && sourceStillValid && insertion) {
          dropRef.current({ sourceGroup: current.sourceGroup, targetGroup: insertion.group, tabKey: current.tabKey, beforeKey: insertion.beforeKey });
        }
        cancel();
      } else {
        gesture.current = null;
        try { if (current.sourceButton.hasPointerCapture(current.pointerId)) current.sourceButton.releasePointerCapture(current.pointerId); } catch { /* Already released. */ }
      }
    };
    const onClick = (event: MouseEvent) => {
      const button = suppressedClick.current;
      if (!button) return;
      if (event.detail !== 0) {
        event.preventDefault();
        event.stopImmediatePropagation();
        suppressedClick.current = null;
      }
    };
    const onCancel = () => cancel();
    const onLostCapture = (event: PointerEvent) => { if (gesture.current?.pointerId === event.pointerId) cancel(); };
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape" && gesture.current) { event.preventDefault(); event.stopPropagation(); event.stopImmediatePropagation(); cancel(); } };
    const onResize = () => { if (gesture.current) cancel(); };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("pointermove", onPointerMove, true);
    document.addEventListener("pointerup", onPointerUp, true);
    document.addEventListener("pointercancel", onCancel, true);
    document.addEventListener("click", onClick, true);
    document.addEventListener("lostpointercapture", onLostCapture, true);
    window.addEventListener("blur", onCancel);
    window.addEventListener("resize", onResize);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("pointermove", onPointerMove, true);
      document.removeEventListener("pointerup", onPointerUp, true);
      document.removeEventListener("pointercancel", onCancel, true);
      document.removeEventListener("click", onClick, true);
      document.removeEventListener("lostpointercapture", onLostCapture, true);
      window.removeEventListener("blur", onCancel);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", onKeyDown, true);
      cancel();
    };
  }, []);

  const sourceGroup = gesture.current?.sourceGroup;
  const label = overlay?.insertion?.group && overlay.insertion.group !== sourceGroup
    ? overlay.insertion.group === "secondary" ? t("pages.groups.moveToRight") : t("pages.groups.moveToCentral")
    : "";
  return <>
    {overlay && createPortal(<div className="note-tab-drag-ghost" aria-hidden="true" style={{ left: Math.max(8, Math.min(overlay.x + 12, innerWidth - 284)), top: Math.max(8, Math.min(overlay.y + 12, innerHeight - 68)) }}>
      <span>{overlay.title}</span>{label && !overlay.insertion?.preview && <span className="note-tab-drag-feedback">{label}</span>}
    </div>, document.body)}
    {overlay?.insertion && !overlay.insertion.preview && createPortal(<div className="note-tab-drop-marker" aria-hidden="true" style={{
      left: Math.max(0, Math.min(innerWidth - 2, overlay.insertion.x - 1)),
      top: Math.max(0, Math.min(innerHeight - 28, overlay.insertion.y + (overlay.insertion.height - 28) / 2)),
    }} />, document.body)}
    {overlay?.insertion?.preview && createPortal(<div className="note-tab-split-preview" data-kind={overlay.insertion.kind} aria-hidden="true" style={overlay.insertion.preview}>
      <span className="note-tab-drop-label">{label}</span>
    </div>, document.body)}
  </>;
}
