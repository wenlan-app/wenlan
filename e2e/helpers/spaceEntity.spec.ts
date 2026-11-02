// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "../tauriMock";
import { getSpaceEntityButton } from "./spaceEntity";

const TOPICS_PINNED = JSON.stringify({ version: 2, visible: ["pages", "spaces", "graph", "entities"] });

test("finds the exact entity after navigating to Topics in More", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");

  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  await expect(navigation.getByRole("button", { name: "Topics", exact: true })).toHaveCount(0);
  await expect(navigation.getByRole("button", { name: "More", exact: true })).toBeVisible();

  const entity = await getSpaceEntityButton(page, "Ada Lovelace");
  await expect(page.getByRole("heading", { name: "Topics", exact: true })).toBeVisible();
  await expect(entity).toBeVisible();
  await expect(entity).toHaveAccessibleName("Ada Lovelace");
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("finds the exact entity when Topics is pinned in primary navigation", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    localStorage: { "wenlan-navigation-v1": TOPICS_PINNED },
  });
  await page.goto("/");

  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  await expect(navigation.getByRole("button", { name: "Topics", exact: true })).toBeVisible();
  const entity = await getSpaceEntityButton(page, "Ada Lovelace");
  await expect(page.getByRole("heading", { name: "Topics", exact: true })).toBeVisible();
  await expect(entity).toBeVisible();
  await expect(entity).toHaveAccessibleName("Ada Lovelace");
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
