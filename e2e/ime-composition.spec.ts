// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

// Headless Chromium has no IME, so the composition lifecycle is dispatched as
// the DOM events a Pinyin or Bopomofo IME produces around the typed preedit.
async function composition(field: Locator, type: "compositionstart" | "compositionend", data = "") {
  await field.evaluate((element, init) => {
    element.dispatchEvent(new CompositionEvent(init.type, { bubbles: true, data: init.data }));
  }, { type, data });
}

test("a draft title mid-composition keeps Escape and autosave for the committed text", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await openPrimaryDestination(page, "Wiki");
  await page.locator(".note-tab-create").click();

  const title = page.getByRole("textbox", { name: "Title", exact: true });
  await expect(title).toBeFocused();
  const composingReports = () => runtime.calls()
    .filter((call) => call.command === "set_editor_composing")
    .map((call) => (call.args as { active: boolean }).active);

  await composition(title, "compositionstart");
  await title.fill("ㄓㄨㄥ");
  // The native hotkey guard hears about it, so Ctrl+K cannot take focus.
  await expect.poll(() => composingReports().at(-1)).toBe(true);
  // Escape belongs to the IME: the draft stays open, and the preedit is not saved.
  await page.keyboard.press("Escape");
  await page.waitForTimeout(1000);
  await expect(title).toBeVisible();
  await expect(title).toHaveValue("ㄓㄨㄥ");
  expect(runtime.calls().filter((call) => call.command === "create_page_draft")).toHaveLength(0);

  await title.fill("中文標題");
  await composition(title, "compositionend", "中文標題");
  await expect.poll(() => composingReports().at(-1)).toBe(false);
  await expect.poll(() => runtime.calls().find((call) => call.command === "create_page_draft")?.args)
    .toMatchObject({ title: "中文標題" });
  expect(runtime.calls().filter((call) => call.command === "create_page_draft")).toHaveLength(1);

  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
