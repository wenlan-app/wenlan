// SPDX-License-Identifier: AGPL-3.0-only
// A ```mermaid block in a page, drawn as a diagram. Mermaid is loaded on first
// use so pages without a diagram never pay for it, and drawn in strict mode so
// a page cannot run script or load anything through its diagram. Colors come
// from the app's theme tokens and are redrawn when the theme flips. A block
// Mermaid cannot parse stays visible as its source, as it was before.
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/** A wide diagram shrinks to the column down to this share of its own size,
 * then scrolls; below it the labels get too small to read. */
const MIN_SCALE = 0.8;

type MermaidApi = typeof import("mermaid").default;

let mermaidLoad: Promise<MermaidApi> | null = null;
function loadMermaid(): Promise<MermaidApi> {
  mermaidLoad ??= import("mermaid").then((m) => m.default);
  return mermaidLoad;
}

function readTheme() {
  const style = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) =>
    style.getPropertyValue(name).trim() || fallback;
  return {
    dark: document.documentElement.getAttribute("data-theme") !== "light",
    bg: read("--mem-bg", "#191C24"),
    surface: read("--mem-surface", "#202430"),
    text: read("--mem-text", "#F1EFE8"),
    muted: read("--mem-text-secondary", "#C9C8D1"),
    border: read("--mem-border", "#333A4B"),
    accent: read("--mem-accent-page", "#8FB3EA"),
    font: read("--mem-font-body", "sans-serif"),
  };
}

/** Re-renders when <html data-theme> changes. */
function useThemeKey(): string {
  const [key, setKey] = useState(
    () => document.documentElement.getAttribute("data-theme") ?? "",
  );
  useEffect(() => {
    const observer = new MutationObserver(() =>
      setKey(document.documentElement.getAttribute("data-theme") ?? ""),
    );
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, []);
  return key;
}

// Mermaid keeps one global config, so draws are serialized: each one sets the
// theme it needs right before it renders.
let queue: Promise<unknown> = Promise.resolve();

async function draw(id: string, source: string): Promise<string> {
  const mermaid = await loadMermaid();
  const theme = readTheme();
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: "strict",
    theme: "base",
    darkMode: theme.dark,
    fontFamily: theme.font,
    // Tighter than Mermaid's defaults, so a diagram stays close to the size
    // of the text around it.
    flowchart: { nodeSpacing: 30, rankSpacing: 36, diagramPadding: 8 },
    themeVariables: {
      darkMode: theme.dark,
      background: theme.bg,
      fontFamily: theme.font,
      fontSize: "14px",
      primaryColor: theme.surface,
      primaryTextColor: theme.text,
      primaryBorderColor: theme.accent,
      secondaryColor: theme.surface,
      tertiaryColor: theme.bg,
      lineColor: theme.muted,
      textColor: theme.text,
      mainBkg: theme.surface,
      nodeBorder: theme.accent,
      clusterBkg: theme.bg,
      clusterBorder: theme.border,
      edgeLabelBackground: theme.bg,
      titleColor: theme.text,
      actorBkg: theme.surface,
      actorBorder: theme.accent,
      actorTextColor: theme.text,
      signalColor: theme.muted,
      signalTextColor: theme.text,
      noteBkgColor: theme.surface,
      noteBorderColor: theme.border,
      noteTextColor: theme.text,
    },
  });
  const { svg } = await mermaid.render(id, source);
  return withMinWidth(svg);
}

/** The SVG with a min-width of MIN_SCALE times its own width, so a wide
 * diagram stops shrinking there and its frame scrolls instead. */
function withMinWidth(svg: string): string {
  const width = Number(/viewBox="[\d.-]+ [\d.-]+ ([\d.]+)/.exec(svg)?.[1]);
  if (!width) return svg;
  const rule = `min-width: ${Math.round(width * MIN_SCALE)}px;`;
  return svg.replace(/<svg([^>]*?)style="/, `<svg$1style="${rule} `);
}

export default function MermaidDiagram({ source }: { source: string }) {
  const themeKey = useThemeKey();
  const id = `mermaid-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const [state, setState] = useState<
    { kind: "loading" } | { kind: "drawn"; svg: string } | { kind: "failed" }
  >({ kind: "loading" });

  const frame = useRef<HTMLElement>(null);

  const { t } = useTranslation();
  const [overflowing, setOverflowing] = useState(false);

  useLayoutEffect(() => {
    const el = frame.current;
    if (!el || state.kind !== "drawn") return;
    // A diagram wider than its frame scrolls; a fade at the edge says so.
    const measure = () => setOverflowing(el.scrollWidth > el.clientWidth + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [state]);

  useEffect(() => {
    let live = true;
    const run = queue.then(() => draw(id, source));
    queue = run.catch(() => undefined);
    run.then(
      (svg) => live && setState({ kind: "drawn", svg }),
      () => {
        // A failed render can leave Mermaid's scratch element behind.
        document.getElementById(`d${id}`)?.remove();
        if (live) setState({ kind: "failed" });
      },
    );
    return () => {
      live = false;
    };
  }, [id, source, themeKey]);

  if (state.kind === "failed") {
    return (
      <>
        <p className="page-diagram-note">{t("pageDetail.diagramFailed")}</p>
        <pre className="page-diagram-source">
          <code className="language-mermaid">{source}</code>
        </pre>
      </>
    );
  }
  return (
    <figure
      ref={frame}
      className={overflowing ? "page-diagram is-overflowing" : "page-diagram"}
      data-testid="page-diagram"
      // A diagram wider than its frame can be scrolled from the keyboard.
      tabIndex={overflowing ? 0 : undefined}
      aria-busy={state.kind === "loading"}
      // Mermaid's own sanitizer (securityLevel strict) cleans the SVG.
      dangerouslySetInnerHTML={state.kind === "drawn" ? { __html: state.svg } : undefined}
    />
  );
}
