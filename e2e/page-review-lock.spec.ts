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

const COPY: Record<AppLocale, { actions: string; sidebar: string; lock: string; off: string; on: string }> = {
  en: { actions: "Page actions", sidebar: "Open page sidebar", lock: "Lock this page", off: "AI keeps the sentences it wrote up to date. Changes to yours wait for you.", on: "Every AI change waits for your OK." },
  "zh-Hant": { actions: "筆記操作", sidebar: "開啟筆記側欄", lock: "鎖定這篇筆記", off: "AI 會更新它自己寫的句子；要改你的句子，會先問你。", on: "AI 的每個修改都等你點頭。" },
  "zh-Hans": { actions: "笔记操作", sidebar: "打开笔记侧栏", lock: "锁定这篇笔记", off: "AI 会更新它自己写的句子；要改你的句子，会先问你。", on: "AI 的每个修改都等你点头。" },
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
  test(`UX-WHO-2 locks a page from its Info panel with one switch (${locale})`, async ({ page }, testInfo) => {
    const copy = COPY[locale];
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [], pageScenario: { recentChanges } });
    await openFixturePage(page, copy.actions);

    // The page menu carries no lock.
    await page.getByRole("button", { name: copy.actions, exact: true }).click();
    await expect(page.getByRole("menu", { name: copy.actions, exact: true })).toBeVisible();
    await expect(page.getByRole("menuitemcheckbox")).toHaveCount(0);
    await page.keyboard.press("Escape");

    // The sidebar opens on Info. One switch, off by default; no radio choice.
    await page.getByRole("button", { name: copy.sidebar, exact: true }).click();
    const lock = page.getByRole("button", { name: copy.lock, exact: true });
    const section = page.locator(".page-lock");
    await expect(lock).toHaveAttribute("aria-pressed", "false");
    await expect(page.getByText(copy.off, { exact: true })).toBeVisible();
    await expect(page.getByRole("radiogroup")).toHaveCount(0);
    // The switch's name stays on one line in every locale.
    expect(await lineCount(page, copy.lock)).toBe(1);
    await section.screenshot({ path: testInfo.outputPath(`unlocked-${locale}.png`) });

    await lock.click();
    await expect(lock).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByText(copy.on, { exact: true })).toBeVisible();
    await section.screenshot({ path: testInfo.outputPath(`locked-${locale}.png`) });
    expect(controller.calls().filter((call) => call.command === "set_page_review_required").map((call) => call.args))
      .toEqual([{ pageId: "page-architecture", required: true }]);
    const locked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(locked.review_required).toBe(true);

    await lock.click();
    await expect(lock).toHaveAttribute("aria-pressed", "false");
    const unlocked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(unlocked.review_required).toBeUndefined();

    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
