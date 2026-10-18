// SPDX-License-Identifier: AGPL-3.0-only
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { homedir } from "node:os";
import { expect, test } from "@playwright/test";
import { openSpaceEntity } from "./helpers/spaceEntity";
import { openTopicContext } from "./helpers/topicTools";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

const evidenceRoot = process.env.WENLAN_UI_EVIDENCE_DIR || path.join(
  process.env.REPO_DATA_ROOT || path.join(homedir(), ".local", "share", "repo-data"),
  "wenlan", "ui", "spaces-navigation",
);
const evidenceDir = path.join(evidenceRoot, "accessibility");

test("captures Simplified Chinese Space and Entity dossiers at physical DPR2", async ({ browser }) => {
  const context = await browser.newContext({
    deviceScaleFactor: 2,
    viewport: { width: 640, height: 450 },
  });
  const page = await context.newPage();
  const errors = collectBrowserErrors(page);
  await mkdir(evidenceDir, { recursive: true });
  await installTauriMock(page, { locale: "zh-Hans", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  await page.getByTitle("显示侧边栏").click();
  await page
    .getByRole("navigation", { name: "主导航" })
    .getByRole("button", { name: "空间", exact: true })
    .click();
  await page.getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("tab", { name: "笔记" })).toHaveAttribute("aria-selected", "true");
  await expect(page.getByRole("tab", { name: "来源" })).toBeVisible();
  await expect(page.locator(".space-dossier-archive,.space-dossier-entities")).toHaveCount(0);
  await expect(page.locator("aside.memory-sidebar")).toHaveAttribute("aria-hidden", "true");
  const content = page.locator(".space-dossier-content");
  await expect(content).toHaveCSS("display", "flex");
  await expect(content).toHaveCSS("flex-direction", "column");
  const pagesBounds = await page.locator(".space-dossier-pages").boundingBox();
  expect(pagesBounds).not.toBeNull();
  const tabsBounds = await page.locator(".space-project-tabs").boundingBox();
  expect(tabsBounds).not.toBeNull();
  expect(pagesBounds!.y).toBeGreaterThanOrEqual(tabsBounds!.y + tabsBounds!.height);
  const spacePath = path.join(evidenceDir, "space-zh-Hans-dpr2.png");
  await page.screenshot({ path: spacePath, fullPage: false });

  await openSpaceEntity(page, "Ada Lovelace", "zh-Hans");
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();
  await expect(page.getByRole("button", { name: "主题操作", exact: true })).toBeInViewport();
  await expect(page.locator(".entity-detail-seal, .entity-detail-dossier .page-detail-dateline")).toHaveCount(0);
  await expect(page.locator(".entity-detail-reading").getByRole("heading", { level: 2, name: "关于", exact: true })).toBeVisible();
  await expect(page.locator(".entity-detail-reading .entity-graph")).toHaveCount(0);
  const topicContext = await openTopicContext(page, "zh-Hans");
  await expect(topicContext.getByRole("heading", { level: 2, name: "关联", exact: true })).toBeVisible();
  await expect(topicContext.locator(".memory-detail-metadata-list")).toBeVisible();
  const entityPath = path.join(evidenceDir, "entity-zh-Hans-dpr2.png");
  await page.screenshot({ path: entityPath, fullPage: false });

  const accessibilitySnapshot = await page.locator("body").ariaSnapshot();
  expect(accessibilitySnapshot).not.toMatch(/\bIndex\b|索引/iu);
  const metrics = await page.evaluate(() => ({
    devicePixelRatio,
    entityGridColumns: getComputedStyle(document.querySelector(".entity-topic-context")!).gridTemplateColumns.split(" ").length,
    innerHeight,
    innerWidth,
  }));
  expect(metrics).toEqual({
    devicePixelRatio: 2,
    entityGridColumns: 1,
    innerHeight: 450,
    innerWidth: 640,
  });
  await writeFile(path.join(evidenceDir, "zh-Hans-dpr2.json"), `${JSON.stringify({
    ...metrics,
    screenshots: { entity: entityPath, space: spacePath },
  }, null, 2)}\n`);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
  await context.close();
});
