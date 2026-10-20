// SPDX-License-Identifier: AGPL-3.0-only
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

const evidenceDir = path.join(process.cwd(), ".omo/evidence/spaces-cards");

const viewports = [
  { width: 1487, height: 1058 },
  { width: 1280, height: 900 },
  { width: 768, height: 900 },
  { width: 375, height: 812 },
] as const;

async function openSpaces(page: Page): Promise<void> {
  const sidebar = page.locator('aside[aria-label="Primary navigation"]');
  if (await sidebar.getAttribute("aria-hidden") === "true") {
    await page.getByTitle("Show sidebar").click();
  }
  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Spaces", exact: true })
    .click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toHaveClass(/sr-only/);
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
}

test("Spaces cards lens stays inside the viewport", async ({ browser }) => {
  await mkdir(evidenceDir, { recursive: true });

  for (const viewport of viewports) {
    // A fresh context per width: the Tauri mock binds once per page.
    const context = await browser.newContext({ viewport: { ...viewport } });
    const page = await context.newPage();
    const browserErrors = collectBrowserErrors(page);
    // No lens seed: Spaces opens in the default cards lens.
    await installTauriMock(page, { locale: "en", rawActions: [] });
    await page.goto("/");
    await openSpaces(page);

    const cards = page.getByTestId("spaces-cards");
    await expect(cards).toBeVisible();
    await expect(cards.locator(".asset-card")).toHaveCount(4);

    const first = cards.locator(".asset-card").first();
    await expect(first).toContainText("6 pages");
    await expect(first).not.toContainText("205 memories");
    await expect(first).not.toContainText("7 topics");

    const overflow = await page.evaluate(() => ({
      clientWidth: document.documentElement.clientWidth,
      scrollWidth: document.documentElement.scrollWidth,
    }));
    expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth);

    const cardCount = await cards.locator(".asset-card").count();
    for (let index = 0; index < cardCount; index += 1) {
      const card = cards.locator(".asset-card").nth(index);
      await card.scrollIntoViewIfNeeded();
      const box = await card.boundingBox();
      expect(box).not.toBeNull();
      expect(box!.x).toBeGreaterThanOrEqual(-1);
      expect(box!.y).toBeGreaterThanOrEqual(-1);
      expect(box!.x + box!.width).toBeLessThanOrEqual(viewport.width + 1);
      expect(box!.y + box!.height).toBeLessThanOrEqual(viewport.height + 1);
    }

    await expect(page.locator(".spaces-drag-handle")).toHaveCount(0);

    await page.screenshot({
      path: path.join(evidenceDir, `spaces-cards-light-${viewport.width}x${viewport.height}.png`),
      fullPage: false,
    });

    await page.getByRole("button", { name: "Open Wenlan", exact: true }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();

    expect(browserErrors.pageErrors).toEqual([]);
    expect(browserErrors.consoleErrors).toEqual([]);
    await context.close();
  }
});
