// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Page } from "@playwright/test";

/**
 * Save the ordinary page's writing with Escape before using page actions.
 * A page has one state: Escape saves and leaves the page open for writing.
 */
export async function saveOpenPage(page: Page): Promise<void> {
  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await editor.press("Escape");
  await expect(editor).toBeEditable();
  await expect(page.locator(".page-detail")).toBeVisible();
}
