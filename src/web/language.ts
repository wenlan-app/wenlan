// SPDX-License-Identifier: AGPL-3.0-only
import { i18n, initializeI18n } from "../i18n";
import {
  type AppLocale,
  isStoredLocale,
  resolveAppLocale,
} from "../i18n/locales";

type LocaleStorage = Pick<Storage, "getItem" | "setItem">;

export function normalizeSavedWebLocale(value: unknown): AppLocale | null {
  if (isStoredLocale(value)) return value === "system" ? null : value;
  if (typeof value !== "string") return null;
  const language = value.trim().replaceAll("_", "-");
  if (!/^(en|zh)(-|$)/iu.test(language)) return null;
  return resolveAppLocale(language);
}

/** Initialize the web UI from this browser's preference before React mounts. */
export async function initializeWebI18n(options: {
  storage?: LocaleStorage;
  systemLanguages?: readonly string[];
} = {}): Promise<void> {
  const storage = options.storage ?? window.localStorage;
  await initializeI18n(i18n, {
    storage,
    systemLanguages: options.systemLanguages,
  });

  const syncDocumentLanguage = (language: string) => {
    document.documentElement.lang = normalizeSavedWebLocale(language) ?? "en";
  };
  syncDocumentLanguage(i18n.resolvedLanguage ?? i18n.language);
  i18n.on("languageChanged", syncDocumentLanguage);
}
