// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Locator, type Page } from "@playwright/test";
import { openPrimaryDestination } from "./primaryNavigation";
export type SpaceEntityLocale = "en" | "zh-Hans" | "zh-Hant";
const labels = { en: { topics: "Topics", more: "More" }, "zh-Hans": { topics: "主题", more: "更多" }, "zh-Hant": { topics: "主題", more: "更多" } };
/** Legacy helper name retained by callers; Topics opens from the graph. */
export async function getSpaceEntityButton(page: Page, entityName: string, locale: SpaceEntityLocale = "en"): Promise<Locator> {
  await openPrimaryDestination(page, labels[locale].topics, labels[locale].more);
  const entity = page.locator(".entities-view").getByRole("button", { name: entityName, exact: true });
  await expect(entity).toBeVisible();
  return entity;
}
export async function openSpaceEntity(page: Page, entityName: string, locale: SpaceEntityLocale = "en"): Promise<void> {
  await (await getSpaceEntityButton(page, entityName, locale)).click();
}
