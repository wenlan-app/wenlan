// SPDX-License-Identifier: AGPL-3.0-only
/**
 * The active custom theme's background image and corner sticker, main window
 * only. Both images come from the daemon's checked answer as `data:` URLs
 * (see `active_theme_image`), and neither ever takes a click.
 */
import { useEffect, useState } from "react";
import { startCustomThemeSync, useCustomTheme } from "../lib/customTheme";
import { getActiveThemeImage } from "../lib/tauri";

const BG_IMAGE = "--wenlan-theme-bg-image";
const BG_OPACITY = "--wenlan-theme-bg-opacity";

/** Base64 and a fixed mime prefix only, so the value is safe inside url("..."). */
function safeDataUrl(value: string | null): string | null {
  return value && /^data:image\/(png|jpeg|webp);base64,[A-Za-z0-9+/=]+$/.test(value) ? value : null;
}

export default function ThemeDecor() {
  const theme = useCustomTheme();
  const [background, setBackground] = useState<string | null>(null);
  const [sticker, setSticker] = useState<string | null>(null);

  useEffect(() => startCustomThemeSync(), []);

  const revision = theme?.revision ?? null;
  const wantsBackground = Boolean(theme?.background);
  const wantsSticker = Boolean(theme?.sticker);
  useEffect(() => {
    let cancelled = false;
    const load = (kind: "background" | "sticker", wanted: boolean, set: (v: string | null) => void) => {
      if (!wanted) {
        set(null);
        return;
      }
      getActiveThemeImage(kind)
        .then((url) => !cancelled && set(safeDataUrl(url)))
        .catch(() => !cancelled && set(null));
    };
    load("background", wantsBackground, setBackground);
    load("sticker", wantsSticker, setSticker);
    return () => {
      cancelled = true;
    };
  }, [revision, wantsBackground, wantsSticker]);

  const placement = theme?.background?.placement;
  const opacity = theme?.background?.opacity;
  useEffect(() => {
    const root = document.documentElement;
    if (background && placement) {
      root.style.setProperty(BG_IMAGE, `url("${background}")`);
      root.style.setProperty(BG_OPACITY, String(opacity ?? 0.35));
      root.setAttribute("data-theme-bg", placement);
    } else {
      root.style.removeProperty(BG_IMAGE);
      root.style.removeProperty(BG_OPACITY);
      root.removeAttribute("data-theme-bg");
    }
  }, [background, placement, opacity]);

  if (!sticker || !theme?.sticker) return null;
  const { corner, size } = theme.sticker;
  const edge = 16;
  return (
    <img
      alt=""
      aria-hidden="true"
      data-testid="theme-sticker"
      draggable={false}
      src={sticker}
      style={{
        position: "fixed",
        zIndex: 6,
        pointerEvents: "none",
        maxWidth: size,
        maxHeight: size,
        bottom: corner === "top-right" ? undefined : edge,
        top: corner === "top-right" ? 52 + edge : undefined,
        right: corner === "bottom-left" ? undefined : edge,
        left: corner === "bottom-left" ? edge : undefined,
      }}
    />
  );
}
