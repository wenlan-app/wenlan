// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { Space } from "../../../lib/tauri";
import { canReorderTogether } from "./spaceHelpers";

type Reorder = (source: Space, target: Space) => void | boolean | Promise<boolean>;

type Drag = {
  pointerId: number;
  sourceId: string;
  startX: number;
  startY: number;
  moved: boolean;
  rects: Map<string, DOMRect>;
  order: readonly Space[];
  element: HTMLElement;
};

type Projection = {
  sourceId: string;
  targetId: string;
  dx: number;
  dy: number;
  order: readonly Space[];
  rects: ReadonlyMap<string, DOMRect>;
};

/** Pixels the pointer travels before a press on a card becomes a drag. */
const DRAG_THRESHOLD = 6;

/** Controls inside a card that keep their own press behaviour. */
const NO_DRAG = ".spaces-menu-anchor, input, textarea, select, [contenteditable='true']";

function hitTarget(drag: Drag, clientX: number, clientY: number): Space | null {
  const source = drag.order.find((space) => space.id === drag.sourceId);
  if (!source) return null;
  for (const space of drag.order) {
    const rect = drag.rects.get(space.id);
    if (!rect || clientX < rect.left || clientX > rect.right || clientY < rect.top || clientY > rect.bottom) continue;
    return space.id === source.id || canReorderTogether(source, space) ? space : null;
  }
  return null;
}

/**
 * Drag to reorder for the Spaces grid. A press anywhere on a card that
 * travels a few pixels picks it up; the other cards slide into the slots they
 * would take, and dropping on a card puts the dragged one in its place.
 * Starred and unstarred Spaces stay in their own groups, as in the list.
 */
export function useSpaceCardDrag(
  spaces: readonly Space[],
  disabled: boolean,
  identity: string,
  onReorder: Reorder,
) {
  const drag = useRef<Drag | null>(null);
  const [projection, setProjection] = useState<Projection | null>(null);
  const cards = useRef(new Map<string, HTMLDivElement>());
  const settling = useRef(false);
  const generation = useRef(0);
  const suppressClick = useRef(false);
  const identityRef = useRef(identity);
  const orderKey = JSON.stringify(spaces.map((space) => space.id));
  const renderedOrder = useRef(orderKey);
  const orderChanged = renderedOrder.current !== orderKey;
  const reorderRef = useRef(onReorder);
  const spacesRef = useRef(spaces);
  reorderRef.current = onReorder;
  spacesRef.current = spaces;

  const cancel = useCallback((pointerId?: number) => {
    if (pointerId !== undefined && drag.current?.pointerId !== pointerId) return;
    drag.current = null;
    settling.current = false;
    generation.current += 1;
    setProjection(null);
  }, []);

  useLayoutEffect(() => {
    if (orderChanged) {
      // The cards are already in their saved order: commit the zero offsets
      // without animating them back from the old projection.
      cards.current.values().next().value?.getBoundingClientRect();
      renderedOrder.current = orderKey;
    }
  }, [orderChanged, orderKey]);

  useLayoutEffect(() => {
    if (identityRef.current !== identity || (disabled && !settling.current)) cancel();
    identityRef.current = identity;
  }, [identity, disabled, cancel]);

  useEffect(() => {
    const onMove = (event: PointerEvent) => {
      const active = drag.current;
      if (!active || event.pointerId !== active.pointerId) return;
      const dx = event.clientX - active.startX;
      const dy = event.clientY - active.startY;
      if (!active.moved) {
        if (Math.hypot(dx, dy) <= DRAG_THRESHOLD) return;
        active.moved = true;
        try { active.element.setPointerCapture(active.pointerId); } catch { /* unavailable in some test DOMs */ }
      }
      event.preventDefault();
      const target = hitTarget(active, event.clientX, event.clientY);
      setProjection((current) => ({
        sourceId: active.sourceId,
        targetId: target?.id ?? current?.targetId ?? active.sourceId,
        dx,
        dy,
        order: active.order,
        rects: active.rects,
      }));
    };

    const onUp = (event: PointerEvent) => {
      const active = drag.current;
      if (!active || event.pointerId !== active.pointerId) return;
      drag.current = null;
      if (!active.moved) return;
      suppressClick.current = true;
      window.setTimeout(() => { suppressClick.current = false; }, 0);
      const source = active.order.find((space) => space.id === active.sourceId);
      const target = hitTarget(active, event.clientX, event.clientY);
      if (!source || !target || target.id === source.id) { setProjection(null); return; }
      const sourceRect = active.rects.get(source.id);
      const targetRect = active.rects.get(target.id);
      if (!sourceRect || !targetRect) { setProjection(null); return; }
      const token = ++generation.current;
      settling.current = true;
      setProjection({
        sourceId: source.id,
        targetId: target.id,
        dx: targetRect.left - sourceRect.left,
        dy: targetRect.top - sourceRect.top,
        order: active.order,
        rects: active.rects,
      });
      let result: void | boolean | Promise<boolean>;
      try { result = reorderRef.current(source, target); } catch { result = false; }
      const finish = () => {
        if (generation.current !== token) return;
        settling.current = false;
        setProjection(null);
      };
      if (result && typeof (result as Promise<boolean>).then === "function") {
        void (result as Promise<boolean>).then(finish, finish);
      } else finish();
    };

    const onPointerCancel = (event: PointerEvent) => cancel(event.pointerId);
    const onBlur = () => cancel();
    const onResize = () => cancel();
    const onScroll = () => { if (drag.current?.moved) cancel(); };
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && drag.current) cancel(); };
    window.addEventListener("pointermove", onMove, { passive: false });
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onPointerCancel);
    window.addEventListener("blur", onBlur);
    window.addEventListener("resize", onResize);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("keydown", onKey);
    return () => {
      drag.current = null;
      settling.current = false;
      generation.current += 1;
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onPointerCancel);
      window.removeEventListener("blur", onBlur);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("keydown", onKey);
    };
  }, [cancel]);

  const begin = useCallback((event: ReactPointerEvent<HTMLDivElement>, space: Space) => {
    if (disabled || event.button !== 0 || !event.isPrimary || drag.current || settling.current) return;
    if (event.target instanceof Element && event.target.closest(NO_DRAG)) return;
    const element = cards.current.get(space.id);
    if (!element) return;
    const rects = new Map<string, DOMRect>();
    for (const item of spacesRef.current) {
      const card = cards.current.get(item.id);
      if (card) rects.set(item.id, card.getBoundingClientRect());
    }
    drag.current = {
      pointerId: event.pointerId,
      sourceId: space.id,
      startX: event.clientX,
      startY: event.clientY,
      moved: false,
      rects,
      order: [...spacesRef.current],
      element,
    };
  }, [disabled]);

  const cardProps = useCallback((space: Space) => {
    let dx = 0;
    let dy = 0;
    if (projection && space.id === projection.sourceId) {
      dx = projection.dx;
      dy = projection.dy;
    } else if (projection && projection.targetId !== projection.sourceId) {
      // Move the source out of the order and back in at the target's index:
      // each other card slides to the slot its new index occupies.
      const from = projection.order.findIndex((item) => item.id === projection.sourceId);
      const to = projection.order.findIndex((item) => item.id === projection.targetId);
      const index = projection.order.findIndex((item) => item.id === space.id);
      let nextIndex = index;
      if (from < to && index > from && index <= to) nextIndex = index - 1;
      if (to < from && index >= to && index < from) nextIndex = index + 1;
      const own = projection.rects.get(space.id);
      const slot = projection.rects.get(projection.order[nextIndex]?.id ?? "");
      if (own && slot && nextIndex !== index) {
        dx = slot.left - own.left;
        dy = slot.top - own.top;
      }
    }
    const activelyDragging = drag.current?.sourceId === space.id && projection?.sourceId === space.id && !settling.current;
    const settlingSource = projection?.sourceId === space.id && settling.current;
    const classes = ["space-card-wrap"];
    if (activelyDragging) classes.push("is-dragging");
    if (settlingSource) classes.push("is-settling");
    return {
      ref: (node: HTMLDivElement | null) => { if (node) cards.current.set(space.id, node); else cards.current.delete(space.id); },
      style: {
        transform: !orderChanged && (dx || dy) ? `translate(${dx}px, ${dy}px)` : undefined,
        transition: orderChanged ? "none" : undefined,
      } as CSSProperties,
      className: classes.join(" "),
      onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => begin(event, space),
    };
  }, [projection, orderChanged, begin]);

  const onClickCapture = useCallback((event: ReactMouseEvent<HTMLDivElement>) => {
    if (!suppressClick.current) return;
    event.preventDefault();
    event.stopPropagation();
    suppressClick.current = false;
  }, []);

  return { cardProps, onClickCapture, dragging: projection !== null };
}
