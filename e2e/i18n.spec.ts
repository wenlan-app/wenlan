// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock } from "./tauriMock";
import type { AppLocale } from "../src/i18n/locales";

const rawActions = [
  "page_skip_user_edited",
  "observation_add",
  "entity_create",
  "relation_create",
] as const;

const localeCases: Record<Exclude<AppLocale, "en">, {
  home: string;
  wiki: string;
  sources: string;
  more: string;
  navigation: string;
  activity: string;
  seeAllActivity: string;
  action: string;
  spaces: string;
  actionLabels: string[];
}> = {
  "zh-Hans": {
    home: "首页",
    wiki: "Wiki", sources: "来源", more: "更多",
    navigation: "主导航",
    activity: "活动",
    seeAllActivity: "查看全部活动",
    action: "动作",
    spaces: "空间",
    actionLabels: [
      "跳过笔记更新",
      "新增观察",
      "新增实体",
      "新增关系",
    ],
  },
  "zh-Hant": {
    home: "首頁",
    wiki: "Wiki", sources: "來源", more: "更多",
    navigation: "主要導覽",
    activity: "活動",
    seeAllActivity: "查看全部活動",
    action: "動作",
    spaces: "空間",
    actionLabels: [
      "略過筆記更新",
      "新增觀察",
      "新增實體",
      "新增關聯",
    ],
  },
};

test.describe("Chinese interface localization", () => {
  for (const [locale, labels] of Object.entries(localeCases) as [Exclude<AppLocale, "en">, typeof localeCases[Exclude<AppLocale, "en">]][]) {
    test(`${locale} localizes the shell and activity filter`, async ({ page }) => {
      const pageErrors: string[] = [];
      const consoleErrors: string[] = [];
      page.on("pageerror", (error) => pageErrors.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error") {
          consoleErrors.push(message.text());
        }
      });

      await installTauriMock(page, { locale, rawActions: [...rawActions] });
      await page.goto("/");

      const header = page.getByRole("banner");
      const primaryNavigation = page.getByRole("navigation", { name: labels.navigation });
      await expect(primaryNavigation.getByRole("button", { name: labels.home, exact: true })).toHaveCount(0);
      const moreButton = primaryNavigation.getByRole("button", { name: labels.more, exact: true });
      await expect(moreButton).toBeVisible();
      await moreButton.click();
      await expect(primaryNavigation.getByRole("button", { name: labels.sources, exact: true })).toBeVisible();
      await moreButton.click();
      await expect(primaryNavigation.getByRole("button", { name: labels.wiki, exact: true })).toBeVisible();
      await expect(header.getByRole("button", { name: labels.home })).toHaveCount(0);
      await expect(header.getByRole("button", { name: labels.activity })).toHaveCount(0);
      await expect(page.getByRole("complementary", { name: labels.navigation }).getByRole("button", { name: labels.activity, exact: true })).toBeVisible();
      await expect(primaryNavigation.getByRole("button", { name: labels.spaces, exact: true })).toBeVisible();
      await expect(page.getByText("Wenlan 文瀾", { exact: true })).toHaveCount(0);
      await expect(page.getByText("SPACES", { exact: true })).toHaveCount(0);
      await expect(page.getByText("Home", { exact: true })).toHaveCount(0);
      await expect(page.getByText("Activity", { exact: true })).toHaveCount(0);

      // The global sidebar Activity entry opens its summary; the summary's own
      // action goes to the Activity page.
      const activityButton = page.getByTestId("activity-status");
      await expect(activityButton).toHaveAttribute("aria-haspopup", "dialog");
      await activityButton.click();
      await page.getByRole("button", { name: labels.seeAllActivity }).click();

      const actionFilter = page.getByLabel(labels.action);
      await expect(actionFilter).toBeVisible();
      await expect(actionFilter.locator("option")).toHaveText([
        labels.action,
        ...labels.actionLabels,
      ]);

      const renderedText = await page.locator("body").innerText();
      for (const action of rawActions) {
        expect(renderedText).not.toContain(action);
      }
      expect(pageErrors).toEqual([]);
      expect(consoleErrors).toEqual([]);
    });
  }
});
