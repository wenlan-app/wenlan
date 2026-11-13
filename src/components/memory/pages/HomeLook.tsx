// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import type { LookData, Page } from "../../../lib/tauri";
import { LookView } from "../page/look/LookView";
import { usePageLook, type LookMode } from "../page/look/usePageLook";

/** The id the home screen's look is kept under (UX-LOOK-10). */
export const HOME_LOOK_ID = "home";

/** Changes whenever a page is added, removed or edited, so the look reads again. */
export function homeLookVersion(pages: readonly Page[]): string {
  let latest = "";
  for (const page of pages) {
    const at = page.last_modified || page.created_at || "";
    if (at > latest) latest = at;
  }
  return `${pages.length}:${latest}`;
}

/** Which home Settings › Appearance chose, for which view; home opens on it. */
export const HOME_MODE_KEY = "wenlan-home-look-mode";

/**
 * The usual home only while the view it was chosen over is still the one an
 * AI tool set; a new view opens on home again.
 */
export function storedHomeMode(lookId: number): LookMode {
  try {
    return window.localStorage.getItem(HOME_MODE_KEY) === `text:${lookId}` ? "text" : "look";
  } catch {
    return "look";
  }
}

export function storeHomeMode(mode: LookMode, lookId: number): void {
  try {
    if (mode === "text") window.localStorage.setItem(HOME_MODE_KEY, `text:${lookId}`);
    else window.localStorage.removeItem(HOME_MODE_KEY);
  } catch { /* lasts until the app closes */ }
}

/**
 * The home screen's HTML view (UX-LOOK-10): an AI tool drew it with
 * `set_page_look` on `@home`. Home opens on it unless Settings › Appearance
 * chose the usual home; home itself carries no control for it. Only Private
 * home takes one; a team's home and the example wiki keep the usual home.
 */
export function useHomeLook({ supported, pages }: { supported: boolean; pages: readonly Page[] }) {
  const version = homeLookVersion(pages);
  const look = usePageLook({ pageId: HOME_LOOK_ID, supported, version, startMode: storedHomeMode });
  return { ...look, version };
}

export type HomeLookState = ReturnType<typeof useHomeLook>;

/** The view itself, filled with the pages and Spaces this side of home shows. */
export function HomeLookView({ homeLook, inScope, onOpenPage }: {
  homeLook: HomeLookState;
  /** Whether a Space belongs to this side (Private leaves a team's Spaces out). */
  inScope: (space: string) => boolean;
  onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const adjust = useCallback((data: LookData): LookData => {
    if (!data.home) return data;
    const keep = (space: string) => space === "" || inScope(space);
    return {
      ...data,
      home: {
        ...data.home,
        spaces: data.home.spaces.filter((space) => keep(space.name)),
        pages: data.home.pages.filter((page) => keep(page.space)),
      },
    };
  }, [inScope]);
  if (homeLook.failed && homeLook.mode === "look") {
    return <p className="page-look-failed" role="status">{t("pageLook.homeFailed")}</p>;
  }
  if (!homeLook.shown || !homeLook.look) return null;
  return (
    <LookView
      pageId={HOME_LOOK_ID}
      title={t("pageLook.homeName")}
      look={homeLook.look}
      version={homeLook.version}
      adjust={adjust}
      onFailed={homeLook.markFailed}
      onOpenPage={onOpenPage}
    />
  );
}
