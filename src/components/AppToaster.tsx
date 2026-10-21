// SPDX-License-Identifier: AGPL-3.0-only
import { Toaster } from "sonner";

/**
 * Renders `toast()` calls made from the main window. Sonner keeps one toast
 * store per window, and the `#toast` overlay window's Toaster only shows the
 * capture pills raised there, so without this every main-window toast was
 * dropped. Bottom centre keeps clear of the updater (bottom left) and the
 * milestone cards (bottom right).
 */
export function AppToaster() {
  return (
    <Toaster
      position="bottom-center"
      visibleToasts={3}
      duration={5000}
      gap={8}
      offset={24}
      style={{ "--width": "360px" } as React.CSSProperties}
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
