// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, useState } from "react";
import { DotsThree } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { removePageLook, restorePageLook, type LookData, type Page } from "../../../lib/tauri";
import { LookPencil } from "../page/look/LookPencil";
import { LookView } from "../page/look/LookView";
import { usePageLook } from "../page/look/usePageLook";

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
 * The home screen's HTML view (UX-LOOK-10): an AI tool drew it with
 * `set_page_look` on `@home`. Home opens on it; the pencil shows the usual
 * home and lights up, and pressed again brings the view back. Only Private
 * home takes one; a team's home and the example wiki keep the usual home.
 */
export function useHomeLook({ supported, pages }: { supported: boolean; pages: readonly Page[] }) {
  const version = homeLookVersion(pages);
  const look = usePageLook({ pageId: HOME_LOOK_ID, supported, version });
  return { ...look, version };
}

export type HomeLookState = ReturnType<typeof useHomeLook>;

/** The pencil, and a ⋯ menu holding the way to take the view off. */
export function HomeLookBar({ homeLook }: { homeLook: HomeLookState }) {
  const { t } = useTranslation();
  const [menuOpen, setMenuOpen] = useState(false);
  const barRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menuOpen) return;
    const close = (event: PointerEvent) => {
      if (!barRef.current?.contains(event.target as Node)) setMenuOpen(false);
    };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") setMenuOpen(false); };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", close);
      document.removeEventListener("keydown", escape);
    };
  }, [menuOpen]);
  if (!homeLook.hasLook) return null;
  const remove = async () => {
    setMenuOpen(false);
    try {
      const removedId = await removePageLook(HOME_LOOK_ID);
      await homeLook.refresh();
      toast(t("pageLook.removed"), {
        action: {
          label: t("pageLook.undo"),
          onClick: () => {
            void restorePageLook(HOME_LOOK_ID, removedId)
              .then(() => homeLook.refresh())
              .catch(() => toast.error(t("pageLook.restoreFailed")));
          },
        },
      });
    } catch {
      toast.error(t("pageLook.removeFailed"));
    }
  };
  return (
    <div className="home-look-bar" data-testid="home-look-bar" ref={barRef}>
      <LookPencil
        editing={!homeLook.shown}
        label={t("pageLook.usualHome")}
        onChange={(next) => homeLook.choose(next)}
      />
      <button
        type="button"
        className="mem-icon-action"
        aria-label={t("pageLook.homeActions")}
        title={t("pageLook.homeActions")}
        aria-haspopup="menu"
        aria-expanded={menuOpen}
        onClick={() => setMenuOpen((open) => !open)}
      >
        <DotsThree aria-hidden="true" size={20} />
      </button>
      {menuOpen ? (
        <div className="mem-popover-surface home-look-menu" role="menu" aria-label={t("pageLook.homeActions")}>
          <button type="button" role="menuitem" onClick={() => { void remove(); }}>
            {t("pageLook.remove")}
          </button>
        </div>
      ) : null}
    </div>
  );
}

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
