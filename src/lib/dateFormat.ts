export type LocaleDateDisplay = {
  readonly label: string;
  readonly dateTime?: string;
};

/**
 * Formats an already-normalized date for visible inventory copy and semantic
 * `<time>` markup. Callers remain responsible for the source timestamp's unit.
 */
export function formatLocaleDate(
  date: Date,
  locales?: Intl.LocalesArgument,
): LocaleDateDisplay {
  if (!Number.isFinite(date.getTime())) return { label: "—" };
  return {
    label: date.toLocaleDateString(locales, {
      day: "numeric",
      month: "short",
      year: "numeric",
    }),
    dateTime: date.toISOString(),
  };
}

/** Relative "time ago" label for a Unix-seconds timestamp. With a locale the
 *  label is that locale's own short relative time; without one it keeps the
 *  compact English "3h ago". */
export function formatTimeAgo(ts: number, locale?: string): string {
  const now = Date.now() / 1000;
  const diff = now - ts;
  if (locale) {
    const relative = new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "short" });
    if (diff < 60) return relative.format(0, "second");
    if (diff < 3600) return relative.format(-Math.floor(diff / 60), "minute");
    if (diff < 86400) return relative.format(-Math.floor(diff / 3600), "hour");
    if (diff < 604800) return relative.format(-Math.floor(diff / 86400), "day");
    return new Date(ts * 1000).toLocaleDateString(locale);
  }
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
  return new Date(ts * 1000).toLocaleDateString();
}

/** Last path segment, used as a display name for a folder/source path. */
export function folderName(p: string): string {
  return p.split("/").filter(Boolean).pop() || p;
}
