// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";

export default function ConnectionListDisclosure({
  expanded,
  hiddenCount,
  onToggle,
}: {
  expanded: boolean;
  hiddenCount: number;
  onToggle: () => void;
}) {
  const { t } = useTranslation();

  return (
    <button
      type="button"
      className="inline-flex w-fit items-center gap-1 px-2 py-1.5 text-[length:var(--mem-text-xs)] text-[var(--mem-text-secondary)] hover:text-[var(--mem-text)] focus-visible:outline-2 focus-visible:outline-[var(--mem-focus-ring)] focus-visible:outline-offset-2 rounded-[var(--mem-radius-sm)]"
      onClick={onToggle}
    >
      <span>
        {expanded
          ? t("connectMatrix.showLess")
          : t("connectMatrix.showMore", { count: hiddenCount })}
      </span>
      <svg
        aria-hidden="true"
        data-direction={expanded ? "up" : "down"}
        className="h-3.5 w-3.5 shrink-0"
        fill="none"
        stroke="currentColor"
        viewBox="0 0 24 24"
      >
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          strokeWidth={2}
          d={expanded ? "M6 15l6-6 6 6" : "M6 9l6 6 6-6"}
        />
      </svg>
    </button>
  );
}
