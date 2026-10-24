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

const COPY: Record<AppLocale, { actions: string; sidebar: string; heading: string; allowed: string; askFirst: string; fromAi: string; fromPage: string }> = {
  en: { actions: "Page actions", sidebar: "Open page sidebar", heading: "AI changes", allowed: "Allowed", askFirst: "Ask me first", fromAi: "AI wrote this page.", fromPage: "You chose this for this page." },
  "zh-Hant": { actions: "筆記操作", sidebar: "開啟筆記側欄", heading: "AI 修改", allowed: "直接改", askFirst: "先問我", fromAi: "這篇筆記是 AI 寫的。", fromPage: "這是你為這篇筆記選的。" },
  "zh-Hans": { actions: "笔记操作", sidebar: "打开笔记侧栏", heading: "AI 修改", allowed: "直接改", askFirst: "先问我", fromAi: "这篇笔记是 AI 写的。", fromPage: "这是你为这篇笔记选的。" },
};

async function openFixturePage(page: Page, actionsLabel: string): Promise<void> {
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  await expect(page.getByRole("button", { name: actionsLabel, exact: true })).toBeEnabled();
}

/** How many lines an element's own text takes. */
async function lineCount(page: Page, text: string): Promise<number> {
  return page.getByText(text, { exact: true }).first().evaluate((el) => {
    const range = document.createRange();
    range.selectNodeContents(el);
    return new Set(Array.from(range.getClientRects(), (rect) => Math.round(rect.top))).size;
  });
}

for (const locale of ["en", "zh-Hant", "zh-Hans"] as const) {
  test(`sets AI changes on a page from its Info panel (${locale})`, async ({ page }, testInfo) => {
    const copy = COPY[locale];
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [], pageScenario: { recentChanges } });
    await openFixturePage(page, copy.actions);

    // The page menu no longer carries a lock.
    await page.getByRole("button", { name: copy.actions, exact: true }).click();
    await expect(page.getByRole("menu", { name: copy.actions, exact: true })).toBeVisible();
    await expect(page.getByRole("menuitemcheckbox")).toHaveCount(0);
    await page.keyboard.press("Escape");

    // The sidebar opens on Info.
    await page.getByRole("button", { name: copy.sidebar, exact: true }).click();
    const group = page.getByRole("radiogroup", { name: copy.heading, exact: true });
    const allowed = group.getByRole("radio", { name: new RegExp(`^${copy.allowed}`) });
    const askFirst = group.getByRole("radio", { name: new RegExp(`^${copy.askFirst}`) });
    await expect(allowed).toBeChecked();
    await expect(page.getByText(copy.fromAi, { exact: true })).toBeVisible();
    // Each option's name stays on one line in every locale.
    expect(await lineCount(page, copy.allowed)).toBe(1);
    expect(await lineCount(page, copy.askFirst)).toBe(1);
    await group.screenshot({ path: testInfo.outputPath(`allowed-${locale}.png`) });

    await askFirst.click();
    await expect(askFirst).toBeChecked();
    await expect(page.getByText(copy.fromPage, { exact: true })).toBeVisible();
    await group.screenshot({ path: testInfo.outputPath(`ask-first-${locale}.png`) });
    expect(controller.calls().filter((call) => call.command === "set_page_review_required").map((call) => call.args))
      .toEqual([{ pageId: "page-architecture", required: true }]);
    const locked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(locked.review_required).toBe(true);

    await allowed.click();
    await expect(allowed).toBeChecked();
    const unlocked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(unlocked.review_required).toBeUndefined();

    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
