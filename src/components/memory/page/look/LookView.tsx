// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { getPageLookData, reportPageLookError, type PageLook } from "../../../../lib/tauri";
import { withLookImages } from "./lookImages";
import { LookFrame } from "./LookFrame";
import { allowedSites, allowSites, declaredSites } from "./lookSites";
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
export function LookView({ pageId, title, look, version, onFailed, onOpenPage, onCheck }: {
  pageId: string;
  title: string;
  look: PageLook;
  /** Changes when the page's text does, so the data is read again. */
  version: number | string;
  onFailed: () => void;
  onOpenPage?: (pageId: string) => void;
  onCheck?: (text: string, occurrence: number, done: boolean) => void;
}) {
  const query = useQuery({
    queryKey: ["page-look-data", pageId, version],
    queryFn: async () => withLookImages(await getPageLookData(pageId)),
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

  // UX-LOOK-9: a look that names sites runs offline until the person allows
  // them once for this page.
  const { t } = useTranslation();
  const declared = useMemo(() => declaredSites(look.html), [look.html]);
  const [allowed, setAllowed] = useState(() => ({ pageId, sites: allowedSites(pageId) }));
  const allowedHere = allowed.pageId === pageId ? allowed.sites : allowedSites(pageId);
  const [notNow, setNotNow] = useState<string | null>(null);
  const sites = useMemo(() => declared.filter((site) => allowedHere.includes(site)), [declared, allowedHere]);
  const asking = declared.filter((site) => !allowedHere.includes(site));
  const askKey = `${pageId}:${asking.join(" ")}`;

  if (!query.data) return null;
  const report = (message: string) => {
    void reportPageLookError(pageId, message).catch(() => undefined);
  };
  return (
    <>
    {asking.length > 0 && notNow !== askKey && (
      <div className="page-look-sites" role="status">
        <p>{t("pageLook.sitesAsk", { sites: asking.join(", "), count: asking.length })}</p>
        <button type="button" className="page-look-sites-allow" onClick={() => {
          allowSites(pageId, asking);
          setAllowed({ pageId, sites: allowedSites(pageId) });
        }}>{t("pageLook.sitesAllow")}</button>
        <button type="button" onClick={() => setNotNow(askKey)}>{t("pageLook.sitesNotNow")}</button>
      </div>
    )}
    <LookFrame
      data={query.data}
      html={look.html}
      title={title}
      sites={sites}
      onReady={() => mark(pageId, null)}
      onError={(message, beforeReady) => {
        report(message);
        if (beforeReady) {
          mark(pageId, null);
          onFailed();
        }
      }}
      onOpenPage={onOpenPage}
      onCheck={onCheck}
    />
    </>
  );
}
