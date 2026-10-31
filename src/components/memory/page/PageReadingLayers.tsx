// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type ReactNode } from "react";
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

/**
 * The reading body of a page in its layers (docs/ux/spec.md): a short or full
 * human part, one page either way (UX-VIEW-2), and the part for AI folded
 * away below it (UX-VIEW-3). The length switch shows only when the two lengths
 * differ, and the last choice is kept for the next page.
 */
export function PageReadingLayers({
  content,
  renderBody,
}: {
  readonly content: string;
  readonly renderBody: (markdown: string) => ReactNode;
}) {
  const { t } = useTranslation();
  const [length, setLength] = useState<PageLength>(readLength);
  const layers = splitPageLayers(content);
  const twoLengths = layers.short !== layers.human;
  const shown = twoLengths && length === "short" ? layers.short : layers.human;
  const choose = (next: PageLength) => {
    setLength(next);
    writeLength(next);
  };
  return (
    <>
      {twoLengths ? (
        <div className="page-length-switch" role="group" aria-label={t("pageLayers.length")}>
          {(["short", "full"] as const).map((option) => (
            <button
              key={option}
              type="button"
              aria-pressed={length === option}
              onClick={() => choose(option)}
            >
              {t(`pageLayers.${option}`)}
            </button>
          ))}
        </div>
      ) : null}
      {shown ? renderBody(shown) : null}
      {layers.aiDetails ? (
        <details className="page-ai-details" data-testid="page-ai-details">
          <summary>{t("pageLayers.aiDetails")}</summary>
          {renderBody(layers.aiDetails)}
        </details>
      ) : null}
    </>
  );
}
