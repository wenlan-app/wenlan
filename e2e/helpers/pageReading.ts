// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Page } from "@playwright/test";

/** Finish the ordinary page's writing view before using reading-only actions. */
export async function returnToPageReading(page: Page): Promise<void> {
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await editor.press("Escape");
  await expect(editor).toHaveCount(0);
  await expect(page.locator(".page-detail")).toBeVisible();
}
