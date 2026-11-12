// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { getPageLookData, reportPageLookError, type PageLook } from "../../../../lib/tauri";
import { LookFrame } from "./LookFrame";
import { loadingKey, loadingValue } from "./usePageLook";

function mark(pageId: string, lookId: number | null) {
  try {
    if (lookId === null) window.localStorage.removeItem(loadingKey(pageId));
    else window.localStorage.setItem(loadingKey(pageId), loadingValue(lookId));
  } catch { /* the guard only works where storage does */ }
}

/**
 * Shows a page's look filled with the page's current data. If the data cannot
 * be read, or the look errors or stalls before it is ready, it calls
 * `onFailed` and the page shows its text (UX-LOOK-3).
 */
export function LookView({ pageId, title, look, version, onFailed, onOpenPage }: {
  pageId: string;
  title: string;
  look: PageLook;
  /** Changes when the page's text does, so the data is read again. */
  version: number | string;
  onFailed: () => void;
  onOpenPage?: (pageId: string) => void;
}) {
  const query = useQuery({
    queryKey: ["page-look-data", pageId, version],
    queryFn: () => getPageLookData(pageId),
    retry: false,
    staleTime: 0,
  });

  // Marked while loading, so an app that stops mid-load does not loop on it.
  useEffect(() => {
    mark(pageId, look.id);
    return () => mark(pageId, null);
  }, [pageId, look.id]);

  const failed = query.isError;
  useEffect(() => {
    if (!failed) return;
    mark(pageId, null);
    onFailed();
  }, [failed, onFailed, pageId]);

  if (!query.data) return null;
  const report = (message: string) => {
    void reportPageLookError(pageId, message).catch(() => undefined);
  };
  return (
    <LookFrame
      data={query.data}
      html={look.html}
      title={title}
      onReady={() => mark(pageId, null)}
      onError={(message, beforeReady) => {
        report(message);
        if (beforeReady) {
          mark(pageId, null);
          onFailed();
        }
      }}
      onOpenPage={onOpenPage}
    />
  );
}
