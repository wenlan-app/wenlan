// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { resources } from "../src/i18n/resources";
import { openTopicContext, openTopicMenu } from "./helpers/topicTools";

async function capture(page: Page, name: string) {
  if (!process.env.WENLAN_UI_EVIDENCE_DIR) return;
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations()
      .filter(animation => Number.isFinite(animation.effect?.getComputedTiming().endTime))
      .map(animation => animation.finished.catch(() => undefined)));
  });
  await page.screenshot({ path: `${process.env.WENLAN_UI_EVIDENCE_DIR}/topics-browse-${name}.png` });
}

const cases = [
  { locale: "en", theme: "light", more: "More", title: "Topics", search: "Filter topics", openAda: "Ada Lovelace", options: "Topic options", archived: "View archived topics", archivedTitle: "Archived topics", confirm: "Confirm topic", archive: "Archive", restore: "Restore", back: "Back", edit: "Edit page", noteConfirm: "Mark page confirmed" },
  { locale: "zh-Hant", theme: "dark", more: "更多", title: "主題", search: "篩選主題", openAda: "Ada Lovelace", options: "主題選項", archived: "查看已歸檔主題", archivedTitle: "已歸檔的主題", confirm: "確認主題", archive: "歸檔", restore: "復原", back: "返回", edit: "編輯筆記", noteConfirm: "標記筆記為已確認" },
] as const;

for (const copy of cases) test(`${copy.locale} topics browse before managing`, async ({ page }) => {
  const tr = resources[copy.locale].translation;
  const editName = (text: string) => `${tr.entityDetail.editNote} — ${text}`;
  const errors = collectBrowserErrors(page);
  const controller = await installTauriMock(page, { fixture: createSpacesNavigationFixture(), locale: copy.locale, rawActions: [], localStorage: { "wenlan-theme": copy.theme } });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.getByRole("button", { name: copy.more, exact: true }).click();
  await page.getByRole("button", { name: copy.title, exact: true }).click();
  const overview = page.locator(".entities-view");
  await expect(overview.getByRole("button", { name: copy.openAda, exact: true })).toBeVisible();
  await expect(overview.getByText("Charles Babbage", { exact: true })).toBeVisible();
  await expect(overview.getByRole("tablist")).toHaveCount(0);
  await expect(overview.getByRole("checkbox")).toHaveCount(0);
  await expect(overview.getByRole("button", { name: copy.archive, exact: true })).toHaveCount(0);
  expect((await overview.getByRole("searchbox", { name: copy.search }).boundingBox())!.y).toBeLessThan(215);
  await capture(page, `${copy.locale}-${copy.theme}-desktop`);
  await page.getByTestId("asset-lens-rows").click();
  const rows = overview.getByTestId("entities-list");
  await expect(rows).toBeVisible();
  await expect(rows.getByRole("button", { name: "Ada Lovelace", exact: true })).toBeVisible();
  await expect(rows.getByRole("button", { name: "Charles Babbage", exact: true })).toBeVisible();
  await capture(page, `${copy.locale}-${copy.theme}-rows`);
  await page.getByTestId("asset-lens-cards").click();
  await page.setViewportSize({ width: 375, height: 812 });
  await capture(page, `${copy.locale}-${copy.theme}-narrow`);
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBe(375);
  await page.setViewportSize({ width: 1280, height: 800 });
  await overview.getByRole("searchbox", { name: copy.search }).fill("Ada");
  await expect(overview.locator(".entity-row-card")).toHaveCount(1);
  await overview.getByRole("button", { name: copy.openAda, exact: true }).click();
  await expect(page.getByRole("heading", { name: "Ada Lovelace", exact: true })).toBeVisible();
  await expect(page.locator(".notes-workspace-sidebar")).toHaveCSS("width", "240px");
  await expect(page.getByRole("button", { name: copy.more, exact: true })).toHaveAttribute("aria-current", "page");
  expect(controller.calls().filter(call => /^(confirm_entity|archive_entities|restore_entities)_cmd$/.test(call.command))).toEqual([]);
  const info = await openTopicContext(page, copy.locale);
  await expect(info.getByRole("button", { name: tr.infoActions.keepTopic, exact: true })).toHaveAttribute("data-active", "false");
  let menu;
  await capture(page, `${copy.locale}-${copy.theme}-detail`);

  await page.getByRole("button", { name: editName("Wrote the first published algorithm"), exact: true }).click();
  await page.getByRole("textbox", { name: tr.entityDetail.editNote, exact: true }).fill("A revised page about Ada");
  await page.getByRole("textbox", { name: tr.entityDetail.editNote, exact: true }).press("Escape");
  await expect(page.getByRole("button", { name: editName("A revised page about Ada"), exact: true })).toBeVisible();
  await info.getByRole("button", { name: tr.infoActions.keepTopic, exact: true }).click();
  await expect.poll(() => controller.calls().filter(call => call.command === "confirm_entity_cmd").length).toBe(1);
  menu = await openTopicMenu(page, copy.locale);
  await menu.getByRole("menuitem", { name: copy.archive, exact: true }).click();
  menu = await openTopicMenu(page, copy.locale);
  await expect(menu.getByRole("menuitem", { name: copy.restore, exact: true })).toBeVisible();
  await expect(menu.getByRole("menuitemcheckbox")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.locator(".entity-obs-content")).toHaveText("A revised page about Ada");
  await expect(page.getByRole("button", { name: editName("A revised page about Ada"), exact: true })).toHaveCount(0);
  await expect(info.getByRole("button", { name: tr.infoActions.releaseTopic, exact: true })).toBeDisabled();
  await page.getByRole("group", { name: /History navigation|歷史導覽|历史导航/ }).getByRole("button", { name: copy.back, exact: true }).click();
  await expect(overview.getByRole("button", { name: copy.openAda, exact: true })).toHaveCount(0);
  await overview.getByRole("button", { name: copy.options, exact: true }).click();
  await page.getByRole("menuitem", { name: copy.archived, exact: true }).click();
  await expect(page.getByRole("heading", { name: copy.archivedTitle, exact: true })).toBeVisible();
  await expect(overview.getByRole("button", { name: copy.openAda, exact: true })).toBeVisible();
  await capture(page, `${copy.locale}-${copy.theme}-archive`);
  await overview.getByRole("button", { name: copy.openAda, exact: true }).click();
  menu = await openTopicMenu(page, copy.locale);
  await menu.getByRole("menuitem", { name: copy.restore, exact: true }).click();
  menu = await openTopicMenu(page, copy.locale);
  await expect(menu.getByRole("menuitem", { name: copy.archive, exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: editName("A revised page about Ada"), exact: true })).toBeEnabled();
  await page.getByRole("group", { name: /History navigation|歷史導覽|历史导航/ }).getByRole("button", { name: copy.back, exact: true }).click();
  await expect(overview.getByRole("button", { name: copy.openAda, exact: true })).toBeVisible();
  expect(controller.calls().filter(call => call.command === "archive_entities_cmd").map(call => call.args)).toEqual([{ req: { ids: ["entity-ada"], dry_run: false } }]);
  expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
});
