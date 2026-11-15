// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../i18n";
import { APP_LOCALE_STORAGE_KEY } from "../i18n/locales";
import {
  initializeWebI18n,
  normalizeSavedWebLocale,
} from "./language";

function configResponse(uiLanguage: unknown): Response {
  return new Response(JSON.stringify({ ui_language: uiLanguage }), {
    headers: { "content-type": "application/json" },
    status: 200,
  });
}

describe("web language bootstrap", () => {
  afterEach(() => vi.unstubAllGlobals());

  beforeEach(() => {
    localStorage.clear();
    document.documentElement.lang = "en";
  });

  it("maps saved Chinese language tags with the app's locale rules", () => {
    expect(normalizeSavedWebLocale("zh-TW")).toBe("zh-Hant");
    expect(normalizeSavedWebLocale("zh-HK")).toBe("zh-Hant");
    expect(normalizeSavedWebLocale("zh-Hant")).toBe("zh-Hant");
    expect(normalizeSavedWebLocale("zh-CN")).toBe("zh-Hans");
    expect(normalizeSavedWebLocale("fr-FR")).toBeNull();
  });

  it("uses fresh zh-TW browser languages instead of the daemon's en language", async () => {
    const fetchImpl = vi.fn(async () => configResponse("en"));
    vi.stubGlobal("fetch", fetchImpl);

    await initializeWebI18n({
      systemLanguages: ["zh-TW"],
    });

    expect(i18n.language).toBe("zh-Hant");
    expect(localStorage.getItem(APP_LOCALE_STORAGE_KEY)).toBeNull();
    expect(document.documentElement.lang).toBe("zh-Hant");
    expect(fetchImpl).not.toHaveBeenCalled();
  });

  it("lets a browser's stored en choice win over zh system languages", async () => {
    localStorage.setItem(APP_LOCALE_STORAGE_KEY, "en");
    await initializeWebI18n({
      systemLanguages: ["zh-HK"],
    });

    expect(i18n.language).toBe("en");
    expect(localStorage.getItem(APP_LOCALE_STORAGE_KEY)).toBe("en");
    expect(document.documentElement.lang).toBe("en");
  });

  it("keeps a browser's stored system choice and follows its languages", async () => {
    localStorage.setItem(APP_LOCALE_STORAGE_KEY, "system");
    await initializeWebI18n({
      systemLanguages: ["zh-HK"],
    });

    expect(i18n.language).toBe("zh-Hant");
    expect(localStorage.getItem(APP_LOCALE_STORAGE_KEY)).toBe("system");
    expect(document.documentElement.lang).toBe("zh-Hant");
  });

  it("uses navigator-style languages when the browser has no saved choice", async () => {
    await initializeWebI18n({
      systemLanguages: ["zh-TW", "en-US"],
    });
    expect(i18n.language).toBe("zh-Hant");
    expect(localStorage.getItem(APP_LOCALE_STORAGE_KEY)).toBeNull();
    expect(document.documentElement.lang).toBe("zh-Hant");
  });
});
