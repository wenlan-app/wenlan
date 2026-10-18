// SPDX-License-Identifier: AGPL-3.0-only
import type { ReactNode } from "react";

interface DetailLayoutProps {
  readonly children: ReactNode;
  /** Content-specific hooks only; shared geometry belongs in the layout CSS. */
  readonly className?: string;
}

/**
 * The reading template used by Memory, Topic, Wiki and Sources.
 * Compact is for atomic content; long-form allows a wider document measure.
 * Slots stay owned by each page: title/tools, notices, and the existing editor.
 * Typography and spacing live in readingSurface.css, not in each page.
 */
export function DetailReadingFrame({
  children,
  className,
  variant = "long-form",
}: DetailLayoutProps & { readonly variant?: "compact" | "long-form" }) {
  return (
    <div
      className={`reading-document-frame${variant === "compact" ? " reading-document-frame--compact" : ""}${className ? ` ${className}` : ""}`}
      data-detail-reading={variant}
    >
      {children}
    </div>
  );
}

/**
 * Put page-specific actions, metadata, connections and history in this slot.
 * PageInfoDrawer owns the shared panel chrome and responsive/focus behavior;
 * detailInfo.css owns section rhythm, headings, metadata grids and text styles.
 */
export function DetailInfoContent({ children, className }: DetailLayoutProps) {
  return (
    <div className={`detail-info-content${className ? ` ${className}` : ""}`} data-detail-info>
      {children}
    </div>
  );
}
