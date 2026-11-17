// SPDX-License-Identifier: AGPL-3.0-only
// Skills › New: the entry is in the Skills list's New menu (not a sidebar row of its own),
// search lists only passing skills, the full SKILL.md shows before install,
// and install goes to all Spaces or one Space. Set FIND_SKILLS_SHOTS to a
// folder to keep screenshots.
import { expect, test, type Page } from "@playwright/test";
import { openSection } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

const shots = process.env.FIND_SKILLS_SHOTS;

async function shot(page: Page, name: string) {
  if (shots) await page.screenshot({ path: `${shots}/${name}.png`, fullPage: true });
}

for (const locale of ["zh-Hant", "en"] as const) {
  test(`find, read and install a skill from the Skills list (${locale})`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [] });
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.goto("/");
    const directory = await openSection(page, "skills");
    const sidebarItemsBefore = await page.locator(".notes-primary-link").count();

    await directory.locator(".section-list-new-button").click();
    const zh = locale === "zh-Hant";
    await shot(page, `0-library-menu-${locale}`);
    await page.getByRole("menuitem", { name: zh ? "找技能" : "Find skills" }).click();
    await expect(page.getByRole("heading", { level: 1, name: zh ? "找技能" : "Find skills" })).toBeVisible();
    // No new sidebar entry appeared for skills.
    expect(await page.locator(".notes-primary-link").count()).toBe(sidebarItemsBefore);

    await page.getByRole("searchbox").fill(zh ? "會議紀錄" : "meeting notes");
    await page.getByRole("searchbox").press("Enter");
    await expect(page.getByRole("button", { name: /meeting-notes/ })).toBeVisible();
    await expect(page.getByRole("button", { name: /notes-sync/ })).toHaveCount(0);
    await shot(page, `1-search-${locale}`);

    await page.getByRole("button", { name: /meeting-notes/ }).click();
    await expect(page.getByText("Keep the speaker's words when quoting.")).toBeVisible();
    await page.getByLabel(zh ? /只在一個 Space/ : /One Space only/).check();
    await page.getByLabel(zh ? "選擇 Space" : "Choose a Space").selectOption({ index: 1 });
    await shot(page, `2-detail-${locale}`);
    await page.getByRole("button", { name: zh ? "安裝" : "Install", exact: true }).click();
    await expect(page.getByRole("status").filter({ hasText: zh ? "已裝給 2 個 AI 工具" : "Installed for 2 AI tools" })).toBeVisible();
    const install = controller.calls().find((call) => call.command === "skills_install");
    expect((install?.args as { scope: { kind: string } }).scope.kind).toBe("space");
    expect((install?.args as { commit: string }).commit).toMatch(/^[0-9a-f]{40}$/);
    await shot(page, `3-installed-${locale}`);

    await page.getByRole("button", { name: zh ? "‹ 回到搜尋" : "‹ Back to search" }).click();
    await expect(page.getByRole("heading", { level: 2, name: zh ? "你的技能" : "Your skills" })).toBeVisible();
    await shot(page, `4-your-skills-${locale}`);
    expect(errors.pageErrors).toEqual([]);
  });
}

for (const locale of ["zh-Hant", "en"] as const) {
  test(`take skills already on this computer into Wenlan (${locale})`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [] });
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.goto("/");
    const directory = await openSection(page, "skills");
    const zh = locale === "zh-Hant";
    await directory.locator(".section-list-new-button").click();
    await page.getByRole("menuitem", { name: zh ? "找技能" : "Find skills" }).click();

    const local = page.getByRole("region", { name: zh ? "這台電腦上已有的技能" : "Already on this computer" });
    await expect(local.getByText("code-review")).toBeVisible();
    await shot(page, `5-local-${locale}`);
    await local.getByRole("button", { name: zh ? "全部收進文瀾" : "Take all into Wenlan" }).click();
    await expect(page.getByRole("status").filter({ hasText: zh ? "已收進 3 個技能" : "Took in 3 skills" })).toBeVisible();
    const call = controller.calls().find((c) => c.command === "skills_import_local");
    expect((call?.args as { names: string[] }).names).toEqual(["code-review", "release-notes", "sql-style"]);
    const yours = page.getByRole("region", { name: zh ? "你的技能" : "Your skills" });
    await expect(yours.getByText(zh ? "來自這台電腦" : "From this computer")).toHaveCount(3);
    await shot(page, `6-local-taken-in-${locale}`);
    expect(errors.pageErrors).toEqual([]);
  });
}
