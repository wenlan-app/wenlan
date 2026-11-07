// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Page } from "@playwright/test";
import { openWikiNote } from "./wikiWorkspace";
import { resources } from "../../src/i18n/resources";

/** Follow the same rail-or-More path a user takes after customizing navigation. */
export async function openPrimaryDestination(page: Page, name: string, moreLabel = "More") {
  await expect(page.getByRole("main")).toBeVisible();
  const navigation = page.getByRole("navigation", {
    name: /^(Primary navigation|主要導覽|主要导航|主导航)$/,
  });
  const showSidebar = page.getByRole("button", {
    name: /^(Show sidebar|顯示側邊欄|显示侧边栏)$/,
  });
  await expect.poll(async () => await navigation.isVisible() || await showSidebar.isVisible()).toBe(true);
  if (!await navigation.isVisible()) {
    await showSidebar.click();
    await expect(navigation).toBeVisible();
  }
  const destination = navigation.getByRole("button", { name, exact: true });
  if (!await destination.isVisible()) {
    await navigation.getByRole("button", { name: moreLabel, exact: true }).click();
  }
  await destination.click();
}

/**
 * Memories have no page in the navigation (UX-FIND-4): a person opens one
 * from the sources of a page that cites it.
 */
export async function openMemoryFromPage(
  page: Page,
  pageTitle: string,
  memoryTitle: string | RegExp,
  labels: { readonly wiki: string; readonly more?: string; readonly info: string } = { wiki: "Wiki", info: "Info" },
) {
  await openPrimaryDestination(page, labels.wiki, labels.more);
  await openWikiNote(page, pageTitle);
  if (!await page.locator(".page-info-drawer:visible").count()) {
    await page.getByRole("button", { name: labels.info, exact: true }).click();
  }
  await page.locator(".note-info-source-list").getByRole("button", { name: memoryTitle }).click();
}

type SourceLibraryLabels = {
  readonly accountMenu: RegExp;
  readonly settings: string;
  readonly sources: string;
  readonly openList: string;
};

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** The words a person follows to the list of added files, in the app's language. */
export function sourceLibraryLabels(locale: keyof typeof resources = "en"): SourceLibraryLabels {
  const t = resources[locale].translation;
  return {
    accountMenu: new RegExp(`${escapeRegExp(t.identityCard.accountMenu)}$`, "i"),
    settings: t.identityCard.settings,
    sources: t.settings.groups.sources.label,
    openList: t.settings.sources.openLibrary,
  };
}

/**
 * Sources have no place in the navigation: a source is what a sentence cites.
 * The list of added files and links opens from Settings → Sources.
 */
export async function openSourceLibrary(page: Page, labels: SourceLibraryLabels = sourceLibraryLabels()) {
  await expect(page.getByRole("main")).toBeVisible();
  const showSidebar = page.getByRole("button", {
    name: /^(Show sidebar|顯示側邊欄|显示侧边栏)$/,
  });
  const accountMenu = page.getByRole("button", { name: labels.accountMenu });
  await expect.poll(async () => await accountMenu.first().isVisible() || await showSidebar.isVisible()).toBe(true);
  if (!await accountMenu.first().isVisible()) await showSidebar.click();
  // A first-visit hint floats at the foot of the window; close it as a person would.
  const closeHint = page.locator(".tab-intro-close");
  if (await closeHint.isVisible()) await closeHint.click();
  await accountMenu.first().click();
  await page.getByRole("menuitem", { name: labels.settings, exact: true }).click();
  // On a narrow window the Settings groups sit behind the sidebar button.
  const settingsGroup = page.locator(".settings-sidebar").getByRole("button", { name: labels.sources, exact: true });
  await expect.poll(async () => await settingsGroup.isVisible() || await showSidebar.isVisible()).toBe(true);
  if (!await settingsGroup.isVisible()) await showSidebar.click();
  await settingsGroup.click();
  await page.getByRole("button", { name: labels.openList, exact: true }).click();
}
