import { describe, expect, it } from "vitest";
import { formatLocaleDate, formatTimeAgo } from "./dateFormat";

describe("formatLocaleDate", () => {
  it("returns one unambiguous locale-aware calendar style and an ISO machine value", () => {
    const date = new Date("2026-07-10T12:00:00Z");
    const options: Intl.DateTimeFormatOptions = { day: "numeric", month: "short", year: "numeric" };

    expect(formatLocaleDate(date, "en-US")).toEqual({
      label: date.toLocaleDateString("en-US", options),
      dateTime: date.toISOString(),
    });
    expect(formatLocaleDate(date, "zh-TW")).toEqual({
      label: date.toLocaleDateString("zh-TW", options),
      dateTime: date.toISOString(),
    });
  });

  it("uses one neutral fallback for invalid dates", () => {
    expect(formatLocaleDate(new Date(Number.NaN))).toEqual({ label: "—" });
  });
});

describe("formatTimeAgo", () => {
  const fifteenHoursAgo = () => Date.now() / 1000 - 15 * 3600 - 60;

  it("keeps the compact English label without a locale", () => {
    expect(formatTimeAgo(fifteenHoursAgo())).toBe("15h ago");
  });

  it.each(["zh-Hans", "zh-Hant"])("uses the %s relative time instead of English", (locale) => {
    const label = formatTimeAgo(fifteenHoursAgo(), locale);
    expect(label).toBe(new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "short" }).format(-15, "hour"));
    expect(label).not.toMatch(/ago/);
  });
});
