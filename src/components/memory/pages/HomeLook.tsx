// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { LookData, Page } from "../../../lib/tauri";
import { LookView } from "../page/look/LookView";
import { usePageLook } from "../page/look/usePageLook";
import { useCustomTheme } from "../../../lib/customTheme";

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

/**
 * Home's view is part of the theme in use, so read it again the moment the
 * theme changes rather than at the next poll.
 */
export function useHomeLookFollowsTheme(refresh: () => unknown): void {
  const themeId = useCustomTheme()?.id ?? null;
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    void refresh();
  }, [themeId, refresh]);
}

/**
 * The home screen's HTML view (UX-LOOK-10): an AI tool drew it with
 * `set_page_look` on `@home`, and it is saved into the theme in use, so home
 * shows it while that theme is on. Home itself carries no control for it;
 * Settings › Appearance takes it off. Only Private home takes one; a team's
 * home and the example wiki keep the usual home.
 */
export function useHomeLook({ supported, pages }: { supported: boolean; pages: readonly Page[] }) {
  const version = homeLookVersion(pages);
  const look = usePageLook({ pageId: HOME_LOOK_ID, supported, version });
  useHomeLookFollowsTheme(look.refresh);
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
