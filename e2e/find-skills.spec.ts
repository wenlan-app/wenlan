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
    await page.getByLabel(zh ? /只在一個空間/ : /One Space only/).check();
    await page.getByLabel(zh ? "選擇空間" : "Choose a Space").selectOption({ index: 1 });
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

for (const locale of ["zh-Hant", "en"] as const) {
  test(`UX-SKILL-13 a skill that cannot be shown in full says so and cannot be installed (${locale})`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [] });
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.goto("/");
    const directory = await openSection(page, "skills");
    const zh = locale === "zh-Hant";
    await directory.locator(".section-list-new-button").click();
    await page.getByRole("menuitem", { name: zh ? "找技能" : "Find skills" }).click();
    await page.getByRole("searchbox").fill("playbook");
    await page.getByRole("searchbox").press("Enter");
    await page.getByRole("button", { name: /long-playbook/ }).click();

    await expect(page.getByText("Keep each update under one minute.")).toBeVisible();
    await expect(page.getByText(zh ? "到這裡被截斷，後面的內容沒有顯示。" : "Cut here. The rest of this file is not shown.")).toHaveCount(1);
    await expect(page.getByRole("note")).toHaveText(zh
      ? "notes/playbook.md 太長，沒辦法完整顯示，所以不能安裝。"
      : "notes/playbook.md is too long to show in full, so it can't be installed.");
    const install = page.getByRole("button", { name: zh ? "安裝" : "Install", exact: true });
    await expect(install).toBeDisabled();
    // Neither sentence wraps or spills out of its column.
    for (const text of [page.getByRole("note"), page.getByText(zh ? "不能安裝" : "Can't be installed", { exact: true })]) {
      const box = await text.evaluate((node) => {
        const range = document.createRange();
        range.selectNodeContents(node);
        return { lines: range.getClientRects().length, spills: node.scrollWidth > node.clientWidth };
      });
      expect(box).toEqual({ lines: 1, spills: false });
    }
    await shot(page, `7-cannot-install-${locale}`);
    expect(controller.calls().some((call) => call.command === "skills_install")).toBe(false);
    expect(errors.pageErrors).toEqual([]);
  });

  test(`UX-SKILL-13 a skill with characters that hide its text cannot be installed (${locale})`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { locale, rawActions: [] });
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.goto("/");
    const directory = await openSection(page, "skills");
    const zh = locale === "zh-Hant";
    await directory.locator(".section-list-new-button").click();
    await page.getByRole("menuitem", { name: zh ? "找技能" : "Find skills" }).click();
    await page.getByRole("searchbox").fill("helper");
    await page.getByRole("searchbox").press("Enter");
    await page.getByRole("button", { name: /quiet-helper/ }).click();

    await expect(page.locator("#find-skills-blocked")).toHaveText(zh
      ? "SKILL.md 裡有看不見或會改變文字順序的字元，所以不能安裝。"
      : "SKILL.md has characters that don't show or that reorder its text, so it can't be installed.");
    // Every hidden character is on the screen as a mark the person can read.
    await expect(page.locator(".find-skills-file mark")).toHaveCount(15);
    await expect(page.locator(".find-skills-file mark").first()).toHaveText("<U+E0073>");
    await expect(page.locator(".find-skills-marked")).toHaveText(zh
      ? "這個檔案有 15 個在畫面上看不見的字元，這裡用 <U+…> 標出來。"
      : "15 characters in this file don't show on screen. Each is written out here as <U+…>.");
    await expect(page.getByRole("button", { name: zh ? "安裝" : "Install", exact: true })).toBeDisabled();
    await shot(page, `9-hidden-characters-${locale}`);
    expect(controller.calls().some((call) => call.command === "skills_install")).toBe(false);
    expect(errors.pageErrors).toEqual([]);
  });

  test(`UX-SKILL-12 characters that would not show are written out in the preview (${locale})`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await installTauriMock(page, { locale, rawActions: [] });
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.goto("/");
    const directory = await openSection(page, "skills");
    const zh = locale === "zh-Hant";
    await directory.locator(".section-list-new-button").click();
    await page.getByRole("menuitem", { name: zh ? "找技能" : "Find skills" }).click();
    await page.getByRole("searchbox").fill("tidy");
    await page.getByRole("searchbox").press("Enter");
    await page.getByRole("button", { name: /tidy-notes/ }).click();

    await expect(page.locator(".find-skills-file mark")).toHaveText(["<U+200B>", "<U+200D>"]);
    await expect(page.locator(".find-skills-marked")).toHaveText(zh
      ? "這個檔案有 2 個在畫面上看不見的字元，這裡用 <U+…> 標出來。"
      : "2 characters in this file don't show on screen. Each is written out here as <U+…>.");
    await expect(page.locator("#find-skills-blocked")).toHaveCount(0);
    await expect(page.getByRole("button", { name: zh ? "安裝" : "Install", exact: true })).toBeEnabled();
    await shot(page, `10-visible-marks-${locale}`);
    expect(errors.pageErrors).toEqual([]);
  });

  test(`UX-SKILL-13 a refused install says why in the app's own words (${locale})`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await installTauriMock(page, {
      locale,
      rawActions: [],
      failures: [{ command: "skills_install", message: "skills_need_this_computer: this Wenlan listens beyond this computer (WENLAN_BIND_ADDR), so skills cannot be installed, taken in or removed over it. Use the Wenlan app" }],
    });
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.goto("/");
    const directory = await openSection(page, "skills");
    const zh = locale === "zh-Hant";
    await directory.locator(".section-list-new-button").click();
    await page.getByRole("menuitem", { name: zh ? "找技能" : "Find skills" }).click();
    await page.getByRole("searchbox").fill("meeting");
    await page.getByRole("searchbox").press("Enter");
    await page.getByRole("button", { name: /meeting-notes/ }).click();
    await page.getByRole("button", { name: zh ? "安裝" : "Install", exact: true }).click();
    const refused = page.getByRole("alert").filter({ hasText: zh ? "裝不起來" : "Couldn't install" });
    await expect(refused.locator("p")).toHaveText(zh
      ? ["裝不起來。", "文瀾現在能從其他電腦連進來，所以這裡不能安裝、收進或移除技能。"]
      : ["Couldn't install.", "Wenlan can be reached from other computers right now, so skills can't be changed here."]);
    await expect(refused).not.toContainText("WENLAN_BIND_ADDR");
    // The refusal is a block of its own inside the narrow install column:
    // it wraps there, and neither it nor the column spills sideways.
    const fits = await refused.evaluate((node) => {
      const side = node.closest(".find-skills-side") as HTMLElement;
      const own = node.getBoundingClientRect();
      const column = side.getBoundingClientRect();
      return { inside: own.left >= column.left && own.right <= column.right, spills: node.scrollWidth > node.clientWidth || side.scrollWidth > side.clientWidth };
    });
    expect(fits).toEqual({ inside: true, spills: false });
    await shot(page, `8-refused-${locale}`);
    expect(errors.pageErrors).toEqual([]);
  });
}
