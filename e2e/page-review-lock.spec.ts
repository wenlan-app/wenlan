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
  en: { actions: "Page actions", toggle: "Require review for changes", notice: "Machine changes to this page wait for your review." },
  "zh-Hant": { actions: "頁面操作", toggle: "改動要審核", notice: "這頁的機器改動會先等你審核。" },
  "zh-Hans": { actions: "页面操作", toggle: "改动要审核", notice: "这页的机器改动会先等你审核。" },
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
    const unlocked = await page.evaluate(async () => window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" })) as { review_required?: boolean };
    expect(unlocked.review_required).toBeUndefined();

    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
