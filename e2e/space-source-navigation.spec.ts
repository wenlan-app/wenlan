// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpaceProjectFixture } from "./fixtures/spaceProject";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { resources } from "../src/i18n/resources";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

for (const cfg of [
  { locale: "en", theme: "light", width: 1440, sidebar: true },
  { locale: "zh-Hant", theme: "light", width: 1280, sidebar: false },
  { locale: "zh-Hant", theme: "dark", width: 375, sidebar: false },
] as const) {
  test(`Space preview goes to the same source ${cfg.locale} ${cfg.width}`, async ({ page }, info) => {
    const t = resources[cfg.locale].translation;
    const fixture = createSpaceProjectFixture();
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, { fixture, locale: cfg.locale, rawActions: [], localStorage: {
      "wenlan-theme": cfg.theme, "wenlan-spaces-view-mode": "rows",
      "wenlan-navigation-v1": JSON.stringify({ version: 2, visible: ["pages", "spaces", "sources", "memories", "entities", "graph"], sidebar: { visible: cfg.sidebar, mode: "labels", width: 240 } }),
    } });
    await page.setViewportSize({ width: cfg.width, height: 900 });
    const annotationCalls: Record<string, unknown>[] = [];
    await page.exposeBinding("__sourceJumpAnnotation", (_s, args: Record<string, unknown>) => {
      annotationCalls.push(args);
      return { source: "webpage", source_id: args.sourceId, notes: "Keep this source description.", page_ids: [], pages: [], version: 1, updated_at: 1791680000 };
    });
    await page.goto("/");
    await page.evaluate(() => {
      const bridge = window.__TAURI_INTERNALS__!, original = bridge.invoke;
      bridge.invoke = (command, args) => command === "get_source_annotation"
        ? (window as any).__sourceJumpAnnotation(args) : original(command, args);
    });
    await openPrimaryDestination(page, t.sidebar.spaces, t.sidebar.more);
    await page.getByTestId("space-row-project-reading").getByRole("button", { name: "讀書會", exact: true }).click();
    const capture = async (name: string) => {
      await page.mouse.move(0, 0);
      await page.evaluate(() => document.fonts.ready);
      expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
      await expect(page.locator("vite-error-overlay")).toHaveCount(0);
      await page.screenshot({ path: info.outputPath(name) });
    };
    for (const file of fixture.documents!.filter(doc => ["讀書會摘錄", "慢讀與討論｜讀書會參考文章（示範）"].includes(doc.file.title))) {
      await page.getByRole("tab", { name: t.spaceDetail.sourcesTab, exact: true }).click();
      const sourceRow = page.locator(".space-project-sources").getByRole("button", { name: new RegExp(file.file.title.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")) });
      const origin = () => page.locator(".space-dossier").evaluate(element => {
        const title = element.querySelector(".space-dossier-title-input")!.getBoundingClientRect();
        const controls = element.querySelector(".space-project-controls")!.getBoundingClientRect();
        const row = element.querySelector(".source-library-row")!.getBoundingClientRect();
        return { titleX: title.x, titleY: title.y, controlsX: controls.x, rowX: row.x };
      });
      const before = await origin();
      await sourceRow.click();
      const preview = page.locator(".page-info-drawer").filter({ has: page.locator(".source-document-preview") });
      await expect(preview).toBeVisible();
      const assertOrigin = async () => {
        const after = await origin();
        for (const key of Object.keys(before) as (keyof typeof before)[]) {
          expect(Math.abs(after[key] - before[key]), key).toBeLessThanOrEqual(1);
        }
      };
      await assertOrigin();
      await preview.getByRole("button", { name: t.sourceAccess.close, exact: true }).click();
      await expect(preview).toHaveCount(0);
      await assertOrigin();
      await sourceRow.click();
      await expect(preview).toBeVisible();
      await assertOrigin();
      await expect(preview.locator(".page-info-drawer-workspace-toggle")).toHaveCount(0);
      const go = preview.getByRole("button", { name: t.sourceAccess.goToSource, exact: true });
      await expect(go).toBeVisible();
      await expect(preview.getByRole("button", { name: t.sourceAccess.close, exact: true })).toBeVisible();
      await capture(`${file.file.source}-preview.png`);
      await go.click();
      await expect(page.locator(".source-document-page h1")).toHaveText(file.file.title);
      await expect(page.locator(".source-document-preview")).toHaveCount(0);
      if (file.file.source === "webpage") {
        await expect(page.locator("main .source-annotations")).toContainText("Keep this source description.");
        expect(annotationCalls.every(args => args.space === "讀書會" && args.sourceId === file.file.source_id)).toBe(true);
      } else {
        await expect(page.locator("main .source-document-body")).toContainText("這份文件只屬於讀書會");
        const sourceChunkCalls = controller.calls().filter(call =>
          call.command === "get_chunks" && isRecord(call.args) && call.args.sourceId === file.file.source_id,
        );
        expect(sourceChunkCalls.length).toBeGreaterThan(0);
        expect(sourceChunkCalls.every(call => isRecord(call.args) && call.args.space === "讀書會")).toBe(true);
      }
      // A source has no navigation item: nothing in the rail claims it.
      if (cfg.width >= 1100) await expect(page.getByRole("navigation").getByRole("button", { name: t.sidebar.sources, exact: true })).toHaveCount(0);
      await capture(`${file.file.source}-destination.png`);
      await page.getByRole("button", { name: t.main.back, exact: true }).click();
      await expect(page.locator(".space-dossier")).toBeVisible();
      await expect(page.getByRole("textbox", { name: t.spaces.overview.nameLabel, exact: true })).toHaveValue("讀書會");
    }
    expect(controller.calls().filter(call => call.command === "open_search_result")).toEqual([]);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
