// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { splitPageLayers } from "./pageLayers";
import "./pageReadingLayers.css";

export type PageLength = "short" | "full";

const LENGTH_KEY = "wenlan-page-length";

function readLength(): PageLength {
  try {
    return localStorage.getItem(LENGTH_KEY) === "short" ? "short" : "full";
  } catch {
    return "full";
  }
}

function writeLength(length: PageLength) {
  try {
    localStorage.setItem(LENGTH_KEY, length);
  } catch {
    // Without storage the choice lasts until the page closes.
  }
}

/** The reader's last choice of length, kept for the next page. */
export function usePageLength(): readonly [PageLength, (next: PageLength) => void] {
  const [length, setLength] = useState<PageLength>(readLength);
  const choose = useCallback((next: PageLength) => {
    setLength(next);
    writeLength(next);
  }, []);
  return [length, choose] as const;
}

/**
 * The reading body of a page in its layers (docs/ux/spec.md): a short or full
 * human part, one page either way (UX-VIEW-2), and the part for AI folded
 * away below it (UX-VIEW-3). The length is chosen in the page's ⋯ menu; a
 * short page ends with a way to the full one. The editor shows the same layers
 * by folding (editor/pageLayersEditing.ts).
 */
export function PageReadingLayers({
  content,
  length,
  onChoose,
  renderBody,
}: {
  readonly content: string;
  readonly length: PageLength;
  readonly onChoose: (next: PageLength) => void;
  readonly renderBody: (markdown: string) => ReactNode;
}) {
  const { t } = useTranslation();
  const layers = splitPageLayers(content);
  const twoLengths = layers.short !== layers.human;
  const short = twoLengths && length === "short";
  const shown = short ? layers.short : layers.human;
  return (
    <>
      {shown ? renderBody(shown) : null}
      {short ? (
        <button type="button" className="page-length-show-full" onClick={() => onChoose("full")}>
          {t("pageLayers.showFull")}
        </button>
      ) : null}
      {layers.aiDetails ? (
        <details className="page-ai-details" data-testid="page-ai-details">
          <summary>{t("pageLayers.aiDetails")}</summary>
          {renderBody(layers.aiDetails)}
        </details>
      ) : null}
    </>
  );
}
