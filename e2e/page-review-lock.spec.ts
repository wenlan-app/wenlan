// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";
import type { AppLocale } from "../src/i18n/locales";

const recentChanges = [{
  page_id: "page-architecture",
  title: "Fixture architecture",
  change_kind: "revised" as const,
  changed_at_ms: Date.parse("2026-07-10T12:00:00Z"),
}];

const COPY: Record<AppLocale, { actions: string; toggle: string; notice: string }> = {
  en: { actions: "Note actions", toggle: "Review every change", notice: "AI edits to this note wait for your review." },
  "zh-Hant": { actions: "筆記操作", toggle: "改動要審核", notice: "AI 改這篇筆記時，會先等你審核。" },
  "zh-Hans": { actions: "笔记操作", toggle: "改动要审核", notice: "AI 改这篇笔记时，会先等你审核。" },
};

async function openFixturePage(page: Page, actionsLabel: string): Promise<void> {
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  await expect(page.getByRole("button", { name: actionsLabel, exact: true })).toBeEnabled();
}

for (const locale of ["en", "zh-Hant", "zh-Hans"] as const) {
  test(`toggles the review lock from the page menu (${locale})`, async ({ page }, testInfo) => {
    const copy = COPY[locale];
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [], pageScenario: { recentChanges } });
    await openFixturePage(page, copy.actions);
    const notice = page.getByText(copy.notice, { exact: true });
    const actions = page.getByRole("button", { name: copy.actions, exact: true });
    await expect(notice).toHaveCount(0);

    await actions.click();
    const toggle = page.getByRole("menuitemcheckbox", { name: copy.toggle, exact: true });
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    // A mouse open focuses the first item without drawing a focus ring.
    const menu = page.getByRole("menu", { name: copy.actions, exact: true });
    const focusedItem = menu.locator('[role^="menuitem"]:focus');
    await expect(focusedItem).toHaveCount(1);
    await expect(focusedItem).toHaveCSS("outline-style", "none");
    // The lock icon sits on the same line as the label.
    const iconBox = await toggle.locator("svg").boundingBox();
    const toggleBox = await toggle.boundingBox();
    expect(iconBox && toggleBox).toBeTruthy();
    expect(Math.abs((iconBox!.y + iconBox!.height / 2) - (toggleBox!.y + toggleBox!.height / 2))).toBeLessThan(3);
    expect(toggleBox!.height).toBeLessThan(40);
    // The label stays on one line in every locale.
    const labelLines = await toggle.evaluate((el) => {
      const text = Array.from(el.childNodes).find((node) => node.nodeType === Node.TEXT_NODE && node.textContent?.trim());
      if (!text) return 0;
      const range = document.createRange();
      range.selectNodeContents(text);
      return new Set(Array.from(range.getClientRects(), (rect) => Math.round(rect.top))).size;
    });
    expect(labelLines).toBe(1);
    await page.screenshot({ path: testInfo.outputPath(`menu-${locale}.png`) });
    await toggle.click();
    await expect(notice).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath(`locked-${locale}.png`) });
    expect(controller.calls().filter((call) => call.command === "set_page_review_required").map((call) => call.args))
      .toEqual([{ pageId: "page-architecture", required: true }]);
    const locked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(locked.review_required).toBe(true);

    await actions.click();
    await expect(page.getByRole("menuitemcheckbox", { name: copy.toggle, exact: true })).toHaveAttribute("aria-checked", "true");
    await page.getByRole("menuitemcheckbox", { name: copy.toggle, exact: true }).click();
    await expect(notice).toHaveCount(0);
    // A keyboard open keeps the focus ring.
    await actions.focus();
    await page.keyboard.press("ArrowDown");
    await expect(focusedItem).toHaveCount(1);
    await expect(focusedItem).not.toHaveCSS("outline-style", "none");
    await page.keyboard.press("Escape");
    const unlocked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(unlocked.review_required).toBeUndefined();

    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
