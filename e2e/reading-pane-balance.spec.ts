// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { resources } from "../src/i18n/resources";

for (const cfg of [
  { width: 1280, theme: "light", mode: "icons" },
  { width: 1280, theme: "light", mode: "labels" },
  { width: 1440, theme: "dark", mode: "icons" },
  { width: 1440, theme: "dark", mode: "labels" },
] as const) {
  test(`Memory, Topic, Wiki and Sources reading panes ${cfg.width} ${cfg.theme} ${cfg.mode}`, async ({ page }, info) => {
    const t = resources.en.translation;
    const errors = collectBrowserErrors(page);
    const base = createSpacesNavigationFixture();
    await installTauriMock(page, { locale: "en", fixture: { ...base, documents: [{ file: {
      source: "file", source_id: "/demo/reading.md", title: "Reading pages", chunk_count: 1, last_modified: 1,
    }, content: "# Reading pages\n\nA familiar reading lane keeps text comfortable when supplementary information is open.\n\n## Context\n\nKeep the original source and the next step clear." }] }, rawActions: [], localStorage: {
      "wenlan-theme": cfg.theme,
      "wenlan-navigation-v1": JSON.stringify({ version: 1, visible: ["pages", "sources", "spaces", "memories", "entities", "graph"], sidebar: { visible: true, mode: cfg.mode, width: 240 } }),
    } });
    await page.setViewportSize({ width: cfg.width, height: 900 });
    await page.goto("/");
    async function capture(name: string) {
      await page.mouse.move(0, 0);
      await page.evaluate(() => document.fonts.ready);
      expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
      await page.screenshot({ path: info.outputPath(`${name}.png`) });
    }
    async function readable(frameSelector: string, laneSelector: string) {
      const frame = await page.locator(frameSelector).boundingBox();
      const lane = await page.locator(laneSelector).first().boundingBox();
      expect(frame).not.toBeNull(); expect(lane).not.toBeNull();
      expect(frame!.x).toBeGreaterThanOrEqual(lane!.x);
      expect(frame!.x + frame!.width).toBeLessThanOrEqual(lane!.x + lane!.width + 1);
      expect(frame!.width).toBeLessThanOrEqual(794);
      expect(frame!.width).toBeGreaterThanOrEqual(460);
    }
    async function balancedStart(selector: string) {
      // The column centres in its lane, settling one frame after a side
      // column opens or closes and once the rail finishes folding.
      await expect(page.locator(".memory-shell[data-sidebar-auto-transition]")).toHaveCount(0);
      await expect.poll(() => page.locator(selector).evaluate(el => {
        const frame = el.getBoundingClientRect();
        const lane = el.parentElement!;
        const box = lane.getBoundingClientRect();
        const style = getComputedStyle(lane);
        const left = box.left + lane.clientLeft + parseFloat(style.paddingLeft);
        const right = left + lane.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
        return Math.abs((frame.left - left) - (right - frame.right));
      })).toBeLessThanOrEqual(1);
    }
    async function titleAndTools(frameSelector: string, titleSelector: string, toolsSelector?: string) {
      const frame = await page.locator(frameSelector).boundingBox();
      const title = page.locator(titleSelector).first();
      const box = (await title.boundingBox())!;
      // The accepted origin is 72px below chrome, with a 32px reading inset.
      expect(Math.abs(box.x - frame!.x - 32)).toBeLessThanOrEqual(1);
      expect(Math.abs(box.y - frame!.y - 72)).toBeLessThanOrEqual(1);
      const type = await title.evaluate(el => ({ size: getComputedStyle(el).fontSize, family: getComputedStyle(el).fontFamily, leading: getComputedStyle(el).lineHeight }));
      if (toolsSelector) {
        const tools = (await page.locator(toolsSelector).boundingBox())!;
        expect(Math.abs(tools.y - box.y)).toBeLessThanOrEqual(1);
        expect(Math.abs(tools.x + tools.width - frame!.x - frame!.width + 32)).toBeLessThanOrEqual(1);
      }
      return type;
    }
    await openPrimaryDestination(page, t.sidebar.sources, t.sidebar.more);
    await page.locator(".source-library-row").filter({ hasText: "Reading pages" }).click();
    const pane = page.locator(".page-info-drawer");
    await expect(pane).toBeVisible();
    expect((await pane.boundingBox())!.width).toBe(320);
    await readable(".source-document-page > .reading-document-frame", ".source-document-page");
    const headingType = await titleAndTools(".source-document-page > .reading-document-frame", ".source-document-page .page-detail-title");
    const sourceType = await page.locator(".source-document-body p").first().evaluate(el => ({ size: getComputedStyle(el).fontSize, leading: getComputedStyle(el).lineHeight }));
    expect(sourceType).toEqual({ size: "15px", leading: "26.25px" });
    await balancedStart(".source-document-page > .reading-document-frame");
    const sourceOpenWidth = (await page.locator(".source-document-body").boundingBox())!.width;
    await capture("source-info-open");
    await pane.locator(".page-info-drawer-close").click();
    await expect(pane).toHaveCount(0);
    await readable(".source-document-page > .reading-document-frame", ".source-document-page");
    expect((await page.locator(".source-document-body").boundingBox())!.width).toBeGreaterThanOrEqual(sourceOpenWidth);
    await balancedStart(".source-document-page > .reading-document-frame");
    await capture("source-info-closed");

    await openPrimaryDestination(page, t.sidebar.pages, t.sidebar.more);
    await openWikiNote(page, "Fixture architecture");
    await expect(page.locator(".page-detail .cm-content")).toBeVisible();
    await readable(".page-detail-document", ".note-group-content");
    const wikiStart = (await page.locator(".page-detail-document").boundingBox())!.x;
    await balancedStart(".page-detail-document");
    expect(await titleAndTools(".page-detail-document", ".page-document-editor--title .cm-line:first-child", ".page-detail-actions-menu-trigger")).toEqual(headingType);
    await capture("wiki-info-closed");
    // Info and the note list open and close independently.
    const directory = page.locator(".wiki-workspace-directory");
    const listShown = await directory.isVisible();
    const shell = page.locator(".memory-shell");
    await expect(shell).toHaveAttribute("data-sidebar-layout", cfg.mode);
    await page.getByRole("button", { name: t.pageInspector.open, exact: true }).click();
    await expect(pane).toBeVisible();
    // Labels fold to icons while the rail, list, Info and a 600px note do not fit.
    const folds = cfg.mode === "labels" && cfg.width < 240 + 248 + 320 + 600;
    await expect(shell).toHaveAttribute("data-sidebar-layout", folds ? "icons" : cfg.mode);
    await expect(page.locator(".memory-shell[data-sidebar-auto-transition]")).toHaveCount(0);
    expect((await pane.boundingBox())!.width).toBe(320);
    await readable(".page-detail-document", ".note-group-content");
    expect(await page.locator(".page-detail .cm-line:not(.cm-writing-heading)").first().evaluate(el => ({ size: getComputedStyle(el).fontSize, leading: getComputedStyle(el).lineHeight }))).toEqual(sourceType);
    await balancedStart(".page-detail-document");
    await expect(directory).toBeVisible({ visible: listShown });
    await capture("wiki-info-open");
    if (folds) {
      // The rail toggle brings the labels back while Info stays open.
      await page.locator("[data-sidebar-toggle]").click();
      await expect(shell).toHaveAttribute("data-sidebar-layout", "labels");
      await expect(pane).toBeVisible();
      await expect(page.locator(".memory-shell[data-sidebar-auto-transition]")).toHaveCount(0);
      await readable(".page-detail-document", ".note-group-content");
    }
    await pane.locator(".page-info-drawer-close").click();
    await expect(pane).toHaveCount(0);
    await expect(shell).toHaveAttribute("data-sidebar-layout", cfg.mode);
    await expect(page.locator(".memory-shell[data-sidebar-auto-transition]")).toHaveCount(0);
    await expect(directory).toBeVisible({ visible: listShown });
    await expect(page.locator(".page-detail .cm-content")).toBeVisible();
    await readable(".page-detail-document", ".note-group-content");
    expect(Math.abs((await page.locator(".page-detail-document").boundingBox())!.x - wikiStart)).toBeLessThanOrEqual(1);
    await page.getByRole("button", { name: t.pageInspector.open, exact: true }).click();
    await page.locator(".note-info-source-list").getByRole("button", { name: /Fixture architecture$/ }).click();
    await expect(page.locator(".memory-document-editor .cm-content")).toBeVisible();
    for (const kind of ["memory", "topic"] as const) {
      if (kind === "topic") {
        await page.getByRole("navigation", { name: "Primary navigation", exact: true }).getByRole("button", { name: t.sidebar.entities, exact: true }).click();
        await page.getByRole("button", { name: "Ada Lovelace", exact: true }).click();
        await expect(page.locator(".entity-detail-dossier")).toBeVisible();
      }
      const reader = page.locator(kind === "memory" ? ".memory-document-frame" : ".entity-detail-dossier .page-detail-document");
      await expect(pane).toBeVisible();
      if (kind === "topic") {
        expect(await titleAndTools(".entity-detail-dossier .page-detail-document", ".entity-detail-dossier .page-detail-title", ".entity-detail-dossier .page-detail-actions-menu-trigger")).toEqual(headingType);
        expect(await page.locator(".entity-obs-content").first().evaluate(el => ({ size: getComputedStyle(el).fontSize, leading: getComputedStyle(el).lineHeight }))).toEqual(sourceType);
      } else {
        await titleAndTools(".memory-document-frame", ".memory-document-editor .cm-line:first-child", ".memory-detail-actions .page-detail-actions-menu-trigger");
        expect(await page.locator(".memory-document-editor .cm-line:first-child").evaluate(el => ({ size: getComputedStyle(el).fontSize, leading: getComputedStyle(el).lineHeight }))).toEqual(sourceType);
      }
      await balancedStart(kind === "memory" ? ".memory-document-frame" : ".entity-detail-dossier .page-detail-document");
      const start = (await reader.boundingBox())!.x;
      await capture(`${kind}-info-open`);
      await pane.locator(".page-info-drawer-close").click();
      await balancedStart(kind === "memory" ? ".memory-document-frame" : ".entity-detail-dossier .page-detail-document");
      await capture(`${kind}-info-closed`);
      await page.getByRole("button", { name: t.knowledgeContext.memoryPanel, exact: true }).click();
      await expect(pane).toBeVisible();
      expect(Math.abs((await reader.boundingBox())!.x - start)).toBeLessThanOrEqual(1);
    }
    expect(errors.pageErrors).toEqual([]); expect(errors.consoleErrors).toEqual([]);
  });
}
