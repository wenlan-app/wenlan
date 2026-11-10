// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "../tauriMock";
import { getSpaceEntityButton } from "./spaceEntity";

const TOPICS_PINNED = JSON.stringify({ version: 2, visible: ["pages", "spaces", "graph", "entities"] });

test("finds the exact entity through the graph's All topics, with no Topics on the left", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");

  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  await expect(navigation.getByRole("button", { name: "Graph", exact: true })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Topics", exact: true })).toHaveCount(0);
  await expect(navigation.getByRole("button", { name: "More", exact: true })).toHaveCount(0);

  const entity = await getSpaceEntityButton(page, "Ada Lovelace");
  await expect(page.getByRole("heading", { name: "Topics", exact: true })).toBeVisible();
  await expect(entity).toBeVisible();
  await expect(entity).toHaveAccessibleName("Ada Lovelace");
  // The topic list counts as part of the graph.
  await expect(navigation.getByRole("button", { name: "Graph", exact: true })).toHaveAttribute("aria-current", "page");
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("drops a Topics pin saved before it left the left side", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    localStorage: { "wenlan-navigation-v1": TOPICS_PINNED },
  });
  await page.goto("/");

  const navigation = page.getByRole("navigation", { name: "Primary navigation" });
  await expect(navigation.getByRole("button", { name: "Graph", exact: true })).toBeVisible();
  await expect(navigation.getByRole("button", { name: "Topics", exact: true })).toHaveCount(0);
  const entity = await getSpaceEntityButton(page, "Ada Lovelace");
  await expect(entity).toHaveAccessibleName("Ada Lovelace");
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
