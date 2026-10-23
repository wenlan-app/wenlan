// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { Toaster, useSonner } from "sonner";

const EDGE = 16;

/** Distance from the window's right edge to the right edge of the page area:
 *  the workspace's main column, stopping short of a docked page inspector.
 *  Without a workspace (setup, review) it is just the edge inset. */
function pageAreaRightOffset(): number {
  const main = document.querySelector("main");
  if (!main) return EDGE;
  let right = main.getBoundingClientRect().right;
  const inspector = document.querySelector(".page-info-drawer-docked .page-info-drawer");
  if (inspector) {
    const rect = inspector.getBoundingClientRect();
    if (rect.width > 0 && rect.left < right) right = rect.left;
  }
  return Math.max(EDGE, Math.round(window.innerWidth - right + EDGE));
}

/** Follows the page area while toasts are showing, so the stack moves aside
 *  when the inspector opens. Nothing is watched while the stack is empty. */
function usePageAreaRightOffset(active: boolean): number {
  const [right, setRight] = useState(EDGE);
  useEffect(() => {
    if (!active) return;
    let frame = 0;
    const measure = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setRight(pageAreaRightOffset()));
    };
    setRight(pageAreaRightOffset());
    window.addEventListener("resize", measure);
    const observer = new MutationObserver(measure);
    observer.observe(document.body, { childList: true, subtree: true });
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", measure);
      observer.disconnect();
    };
  }, [active]);
  return right;
}

/**
 * Renders `toast()` calls made from the main window. Sonner keeps one toast
 * store per window, and the `#toast` overlay window's Toaster only shows the
 * capture pills raised there, so without this every main-window toast was
 * dropped. This is the app's one place for one-off messages: milestone cards
 * (`onboarding/MilestoneToaster.tsx`) join the same stack. Lasting states,
 * the service and app updates among them, live in the sidebar footer instead.
 *
 * The stack sits at the bottom right of the page area, not of the window: the
 * window's centre falls on the seam between the list and the page, and its
 * right edge is under the page inspector when that is open.
 */
export function AppToaster() {
  const { toasts } = useSonner();
  const right = usePageAreaRightOffset(toasts.length > 0);
  return (
    <Toaster
      position="bottom-right"
      visibleToasts={3}
      duration={5000}
      gap={8}
      offset={{ bottom: EDGE, right }}
      style={{ "--width": "320px" } as React.CSSProperties}
      toastOptions={{
        unstyled: true,
        classNames: {
          toast: "mem-toast",
          content: "mem-toast-content",
          title: "mem-toast-title",
          description: "mem-toast-description",
          icon: "mem-toast-icon",
        },
      }}
    />
  );
}
