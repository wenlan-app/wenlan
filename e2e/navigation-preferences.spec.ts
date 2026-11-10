// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

// UX-FIND-4: the left side holds every destination, with no More menu. Pins
// saved by earlier versions no longer hide or add anything.
for (const stored of [
  null,
  { version: 2, visible: ["home", "pages", "spaces"], sidebar: { visible: true, mode: "labels" } },
  { version: 2, visible: ["home", "pages", "spaces", "graph", "sources", "memories"], sidebar: { visible: true, mode: "labels" } },
]) {
  test(`every destination sits on the left with saved pins ${stored ? JSON.stringify(stored.visible) : "none"}`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width: 1280, height: 800 });
    await installTauriMock(page, {
      locale: "en",
      rawActions: [],
      localStorage: { "wenlan-theme": "light", ...(stored ? { "wenlan-navigation-v1": JSON.stringify(stored) } : {}) },
    });
    await page.goto("/");
    const nav = page.locator(".notes-rail-nav");
    const railNames = () => nav.locator(".notes-primary-link, .notes-rail-button").evaluateAll(buttons => buttons.map(button => button.getAttribute("aria-label")));
    const all = ["Home", "Wiki", "Spaces", "Graph"];
    // With labels there is one left column: the Wiki closes the list and its
    // pages follow it, instead of a second column beside the sidebar.
    await expect.poll(railNames).toEqual(["Home", "Spaces", "Graph", "Wiki"]);
    await expect(nav.locator(".wiki-sidebar-directory")).toBeVisible();
    await expect(nav.getByRole("button", { name: "More", exact: true })).toHaveCount(0);

    await nav.getByRole("button", { name: "Graph", exact: true }).click();
    await expect(nav.getByRole("button", { name: "Graph", exact: true })).toHaveAttribute("aria-current", "page");
    await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);

    await page.setViewportSize({ width: 375, height: 812 });
    await expect(page.locator(".notes-workspace-sidebar")).toHaveAttribute("aria-hidden", "true");
    await page.locator("[data-sidebar-toggle]").click();
    await expect.poll(railNames).toEqual(all);
    await nav.getByRole("button", { name: "Graph", exact: true }).click();
    await expect(page.getByTestId("atlas-view")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
