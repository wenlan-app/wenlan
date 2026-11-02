// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Page } from "@playwright/test";
import { openWikiNote } from "./wikiWorkspace";

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
  labels: { readonly wiki: string; readonly more?: string; readonly info: string } = { wiki: "Wiki", info: "Open page sidebar" },
) {
  await openPrimaryDestination(page, labels.wiki, labels.more);
  await openWikiNote(page, pageTitle);
  if (!await page.locator(".page-info-drawer:visible").count()) {
    await page.getByRole("button", { name: labels.info, exact: true }).click();
  }
  await page.locator(".note-info-source-list").getByRole("button", { name: memoryTitle }).click();
}
