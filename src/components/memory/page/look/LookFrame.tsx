// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { LookData } from "../../../../lib/tauri";
import {
  buildLookDocument,
  knownPageIds,
  LOOK_SANDBOX,
  parseLookMessage,
  readLookTheme,
  type LookTheme,
} from "./lookDocument";
import "./look.css";

/** How long a look has to say it is ready before the page shows its text. */
export const LOOK_READY_TIMEOUT_MS = 3000;
const MIN_HEIGHT = 120;

function useAppTheme(): LookTheme {
  const [theme, setTheme] = useState<LookTheme>(() => readLookTheme());
  useEffect(() => {
    // A new object only when a color changed: a new theme reloads the look.
    const update = () => setTheme((previous) => {
      const next = readLookTheme();
      return JSON.stringify(previous) === JSON.stringify(next) ? previous : next;
    });
    update();
    const observer = new MutationObserver(update);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class", "style"] });
    return () => observer.disconnect();
  }, []);
  return theme;
}

/**
 * A page's look, in a frame that cannot reach the app (UX-LOOK-4): two nested
 * sandboxed frames with no same-origin flag and a policy that lets nothing
 * out. The app only hears four kinds of message from its own frame, and
 * `open` only for a page the look was given.
 *
 * `onError(message, beforeReady)`: an error before the look said it was
 * ready, or no ready within three seconds, is `beforeReady`; the caller then
 * shows the text instead.
 */
export function LookFrame({ data, html, title, onReady, onError, onOpenPage }: {
  data: LookData;
  html: string;
  title: string;
  onReady?: () => void;
  onError?: (message: string, beforeReady: boolean) => void;
  onOpenPage?: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLIFrameElement>(null);
  const theme = useAppTheme();
  const [height, setHeight] = useState(480);
  const readyRef = useRef(false);
  const callbacks = useRef({ onReady, onError, onOpenPage });
  callbacks.current = { onReady, onError, onOpenPage };
  const document_ = useMemo(() => buildLookDocument({ data, html, theme }), [data, html, theme]);
  const known = useMemo(() => knownPageIds(data), [data]);

  useEffect(() => {
    readyRef.current = false;
    // The clock only runs while the app is on screen: a hidden window may
    // hold the frame back, and that is not the look's fault.
    let timer = 0;
    const expire = () => {
      if (readyRef.current) return;
      if (document.visibilityState === "hidden") {
        timer = window.setTimeout(expire, LOOK_READY_TIMEOUT_MS);
        return;
      }
      readyRef.current = true;
      callbacks.current.onError?.("The look did not finish loading in three seconds.", true);
    };
    timer = window.setTimeout(expire, LOOK_READY_TIMEOUT_MS);
    const onMessage = (event: MessageEvent) => {
      // Only the frame this component mounted, never another window.
      if (!ref.current || event.source !== ref.current.contentWindow) return;
      const message = parseLookMessage(event.data, known);
      if (!message) return;
      switch (message.kind) {
        case "ready":
          if (readyRef.current) return;
          readyRef.current = true;
          window.clearTimeout(timer);
          callbacks.current.onReady?.();
          return;
        case "error": {
          const before = !readyRef.current;
          if (before) {
            readyRef.current = true;
            window.clearTimeout(timer);
          }
          callbacks.current.onError?.(message.message, before);
          return;
        }
        case "height":
          setHeight(Math.max(MIN_HEIGHT, message.value));
          return;
        case "open":
          callbacks.current.onOpenPage?.(message.pageId);
          return;
      }
    };
    window.addEventListener("message", onMessage);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("message", onMessage);
    };
  }, [document_, known]);

  return (
    <iframe
      ref={ref}
      className="page-look-frame"
      data-testid="page-look-frame"
      title={t("pageLook.frameTitle", { title })}
      sandbox={LOOK_SANDBOX}
      referrerPolicy="no-referrer"
      srcDoc={document_}
      style={{ height }}
    />
  );
}
