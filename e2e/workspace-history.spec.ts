// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

for (const width of [1280, 375]) {
  test(`one history entry spans pages, memories and spaces at ${width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize({ width, height: 900 });
    const errors = collectBrowserErrors(page);
    await installTauriMock(page, { locale: "en", rawActions: [] });
    await page.goto("/");
    const history = page.getByRole("group", { name: "History navigation" });
    const back = history.getByRole("button", { name: "Back", exact: true });
    const forward = history.getByRole("button", { name: "Forward", exact: true });
    await expect(back).toBeDisabled();
    await expect(forward).toBeDisabled();
    await openPrimaryDestination(page, "Spaces");
    await expect(page.getByRole("heading", { name: "Spaces", exact: true })).toBeVisible();
    await back.click();
    await expect(page.getByRole("heading", { name: "Open a page", exact: true })).toBeVisible();
    await forward.press("Enter");
    await expect(page.getByRole("heading", { name: "Spaces", exact: true })).toBeVisible();
    await openPrimaryDestination(page, "Memories");
    await page.getByRole("button", { name: "Open memory", exact: true }).first().click();
    await expect(page.locator(".memory-detail-reading")).toBeVisible();
    await expect(page.locator("main").getByRole("button", { name: "Back", exact: true })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Back to memories", exact: true })).toHaveCount(0);
    await back.click();
    await expect(page.getByRole("region", { name: "Memory list", exact: true })).toBeVisible();
    await forward.click();
    await expect(page.locator(".memory-detail-reading")).toBeVisible();
    await back.click();
    await openPrimaryDestination(page, "Wiki");
    await expect(forward).toBeDisabled();
    const bounds = await history.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    const controls = await page.getByRole("banner").getByRole("button").evaluateAll((nodes) => nodes.filter((node) => node.getBoundingClientRect().width > 0).map((node) => {
      const box = node.getBoundingClientRect(); return { x: box.x, right: box.right };
    }));
    for (const control of controls) expect(control.right).toBeLessThanOrEqual(width);
    await page.screenshot({ path: testInfo.outputPath("shared-navigation.png") });
    expect(errors.pageErrors).toEqual([]);
  });
}
