// SPDX-License-Identifier: AGPL-3.0-only

/**
 * Where the window's own chrome sits, and how much room the app has to leave
 * for it.
 *
 * macOS draws the traffic lights on top of the webview, because
 * app/tauri.conf.json asks for `titleBarStyle: "Overlay"` and positions them at
 * x=16. Everything the app paints in that corner would land underneath them, so
 * the top bars inset their content past it. Windows and Linux draw a real title
 * bar above the webview instead, so the same inset is just dead space.
 *
 * Read from the user agent rather than @tauri-apps/plugin-os or the daemon's
 * get_system_info: both are async, and these values are needed for the first
 * paint. The webview's user agent is the platform's own, so it reports the host
 * rather than anything the app configured.
 *
 * `hostPlatform` is three-valued, because two values were being spent on three
 * states: a UA that says "Windows" and a UA that is missing or unrecognised are
 * different measurements, and a boolean `isMacOS` answered `false` to both.
 * Every caller here still wants a number in the end -- these values decide
 * pixel offsets at first paint, so nothing may wait on an async platform
 * lookup -- but the collapse happens at a named site *per question*:
 *
 *   - "how much left inset?"   -> `UNKNOWN_PLATFORM_TOP_BAR_INSET`     (macOS's)
 *   - "how tall a drag strip?" -> `UNKNOWN_PLATFORM_DRAG_STRIP_HEIGHT` (macOS's)
 *
 * Each is a DECISION about an unmeasured platform, justified where it is
 * declared, and each is pinned by a test so flipping one fails loudly.
 *
 * There is deliberately no exported `isMacOS`. It existed; both of these
 * questions were answered by calling it; and each of them therefore
 * inherited `unknown -> false` invisibly -- the exact
 * failed-measurement-as-a-negative this module was split up to remove. Anything
 * new that needs the platform asks `hostPlatform()` and handles all three
 * states, or calls a function below that has already decided.
 */
export type HostPlatform = "macos" | "other" | "unknown";

export function hostPlatform(
  ua: string | undefined = globalThis.navigator?.userAgent,
): HostPlatform {
  // No navigator, or an empty UA: nothing was measured. A WebView whose UA the
  // app or the host has customised lands here too.
  if (!ua) return "unknown";
  if (/\b(Macintosh|Mac OS X)\b/.test(ua)) return "macos";
  // A UA that names one of the other two platforms the app ships to is a
  // measured "not macOS". Anything else is a UA we cannot place, and saying
  // "not macOS" about it would be the same failed-measurement-as-negative.
  if (/\b(Windows|Linux|X11|CrOS|Android)\b/.test(ua)) return "other";
  return "unknown";
}

/** One centreline for the main header and its sidebar control. */
export const MAIN_HEADER_HEIGHT = 52;

/** Target geometry for the native macOS traffic lights in the main header. */
export const MACOS_TRAFFIC_LIGHT_X = 16;
export const MACOS_TRAFFIC_LIGHT_CENTER_Y = MAIN_HEADER_HEIGHT / 2;

/** Room for the overlaid traffic lights, matched to their x=16 origin. */
export const MACOS_TRAFFIC_LIGHT_INSET = 104;

/** The inset used everywhere else, matched to the top bars' right padding. */
export const NATIVE_TITLE_BAR_INSET = 20;

/**
 * What an unmeasured platform gets for the top bar's left padding: the macOS
 * inset.
 *
 * This is a DECISION about an unmeasured platform, not a measurement of one.
 * The two ways of being wrong are not symmetrical.
 *
 * Wrong as "not macOS" on a real Mac (a WebView whose UA has been customised
 * past recognition): the header's leftmost control -- the sidebar toggle --
 * gets 20px of inset and paints underneath traffic lights that start at x=16.
 * Those buttons are composited *over* the webview and take the click, so the
 * user does not merely lose the toggle; aiming at it presses Close. Functional,
 * and destructive.
 *
 * Wrong as "macOS" on Windows or Linux: `MACOS_TRAFFIC_LIGHT_INSET -
 * NATIVE_TITLE_BAR_INSET` = 84px of extra padding at the left of a header that
 * spans the window anyway. Nothing is covered, nothing is unreachable, the row
 * is just indented. Cosmetic.
 *
 * Cosmetic beats destructive, so `unknown` takes the macOS side here.
 */
const UNKNOWN_PLATFORM_TOP_BAR_INSET = MACOS_TRAFFIC_LIGHT_INSET;

/** Left padding for a top bar that shares its row with the window controls. */
export function topBarLeftInset(ua?: string): number {
  const platform = hostPlatform(ua);
  if (platform === "unknown") return UNKNOWN_PLATFORM_TOP_BAR_INSET;
  return platform === "macos" ? MACOS_TRAFFIC_LIGHT_INSET : NATIVE_TITLE_BAR_INSET;
}

/** Drag target macOS needs above content that has no top bar of its own. */
export const MACOS_DRAG_STRIP_HEIGHT = 32;

/** None needed where the platform draws a real, draggable title bar itself. */
export const NATIVE_DRAG_STRIP_HEIGHT = 0;

/**
 * What an unmeasured platform gets for the drag strip: the macOS height.
 *
 * This is a DECISION about an unmeasured platform, not a measurement of one,
 * and it is the same asymmetry as the inset above in a sharper form.
 *
 * Wrong as "not macOS" on a real Mac: the setup wizard gets a strip of zero. It
 * has no top bar of its own, and its window is `titleBarStyle: "Overlay"`, so
 * there is no native bar to grab either. The window cannot be moved. There is
 * no other drag handle on that screen, so the failure is not merely functional
 * but unrecoverable from inside the app.
 *
 * Wrong as "macOS" on Windows or Linux: a 32px transparent strip sits above the
 * wizard's content, under a native title bar that already drags, and the
 * content starts 32px lower. Cosmetic -- and the strip is, if anything, a
 * second working drag target.
 *
 * An unmovable window beats 32px of dead space, so `unknown` takes the macOS
 * side here too.
 */
const UNKNOWN_PLATFORM_DRAG_STRIP_HEIGHT = MACOS_DRAG_STRIP_HEIGHT;

/**
 * Height of the drag strip above content that has no top bar of its own, such
 * as the setup wizard. Off macOS the native title bar already provides both the
 * drag target and the clearance.
 */
export function dragStripHeight(ua?: string): number {
  const platform = hostPlatform(ua);
  if (platform === "unknown") return UNKNOWN_PLATFORM_DRAG_STRIP_HEIGHT;
  return platform === "macos" ? MACOS_DRAG_STRIP_HEIGHT : NATIVE_DRAG_STRIP_HEIGHT;
}

/**
 * The modifier a shortcut hint names: ⌘ on macOS, Ctrl on Windows and Linux,
 * both when the platform could not be read, since either may be right.
 */
export function primaryShortcutModifier(ua?: string): string {
  switch (hostPlatform(ua)) {
    case "macos":
      return "⌘";
    case "other":
      return "Ctrl";
    case "unknown":
      return "Ctrl/⌘";
  }
}
