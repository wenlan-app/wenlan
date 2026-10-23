// SPDX-License-Identifier: AGPL-3.0-only
import { mkdir, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import path from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { openSpaceEntity } from "./helpers/spaceEntity";
import { openTopicContext } from "./helpers/topicTools";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { renderedContrast } from "./helpers/renderedContrast";

const evidenceRoot = process.env.WENLAN_UI_EVIDENCE_DIR || path.join(
  process.env.REPO_DATA_ROOT || path.join(homedir(), ".local", "share", "repo-data"),
  "wenlan", "ui", "spaces-navigation",
);
const evidenceDir = path.join(evidenceRoot, "screenshots");
const fixtureNow = 1_783_728_000_000;

async function settle(page: Page): Promise<void> {
  await page.evaluate(async () => { await document.fonts.ready; });
  await page.evaluate(() => {
    for (const animation of document.getAnimations()) {
      try {
        animation.finish();
      } catch {
        animation.cancel();
      }
    }
  });
  await expect(page.locator("main")).toBeVisible();
  const sidebar = page.locator('aside[aria-label="Primary navigation"]');
  if (await sidebar.getAttribute("aria-hidden") === "false") {
    await expect(sidebar).toHaveCSS("width", "240px");
    await expect(sidebar.locator(".notes-icon-rail")).toHaveCSS("width", "240px");
    await expect(sidebar.locator(".notes-icon-rail")).toHaveCSS("opacity", "1");
  }
}

// These surfaces share the updated typography. Use explicit browser contracts
// and review artifacts instead of requiring new screenshots in Git.
async function assertRedesignedSurface(page: Page, name: string): Promise<boolean> {
  const spaces = name.startsWith("spaces-");
  const spaceDetail = name.startsWith("space-");
  const entityPage = name.startsWith("entity-");
  const wikiLibrary = name.startsWith("pages-");
  if (!spaces && !spaceDetail && !entityPage && !wikiLibrary) return false;
  const viewport = page.viewportSize()!;
  const overflow = await page.evaluate(() => ({
    page: document.documentElement.scrollWidth - window.innerWidth,
    main: document.querySelector("main")!.scrollWidth - document.querySelector("main")!.clientWidth,
  }));
  expect(overflow.page).toBeLessThanOrEqual(1);
  expect(overflow.main).toBeLessThanOrEqual(1);
  if (wikiLibrary) {
    const reading=page.locator(".wiki-workspace-reading");
    await expect(reading.getByRole("heading", { level: 1, name: "Open a page", exact: true })).toBeVisible();
    await expect(reading.locator(".asset-card, .wiki-table, .wiki-folder-lenses")).toHaveCount(0);
    // Pages appear on the home only in the new-pages list, never as a second directory.
    await expect(reading.getByText("Fixture architecture", { exact: true })).toHaveCount(1);
    await expect(reading.getByTestId("wiki-new-pages").getByText("Fixture architecture", { exact: true })).toBeVisible();
    const control=page.locator(".note-tab-create");
    await expect(control).toBeVisible();
    const box=await control.evaluate(el=>{const r=el.getBoundingClientRect();return {left:r.left,right:r.right,height:r.height,width:r.width};});
    expect(box.left).toBeGreaterThanOrEqual(0);expect(box.right).toBeLessThanOrEqual(viewport.width+1);
    expect(box.height).toBeGreaterThanOrEqual(32);expect(box.width).toBeGreaterThanOrEqual(32);
    const contrast=await renderedContrast(page,[
      {selector:".wiki-overview h1",label:"Workspace title",foregroundProperty:"color",minimum:4.5},
      {selector:".note-tab-create",label:"New page icon",foregroundProperty:"color",minimum:4.5},
    ]);
    for(const result of contrast)expect(result.ratio,result.label).toBeGreaterThanOrEqual(result.minimum);
  } else if (entityPage) {
    // The readability update intentionally changes these pixels. Preserve
    // content, legibility, contrast and responsive containment as live contracts
    // while keeping the complete light/dark captures as review artifacts.
    const detail = page.locator(".entity-detail-dossier");
    await expect(detail).toBeVisible();
    await expect(detail.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();
    await expect(detail.locator(".entity-detail-seal, .page-detail-dateline")).toHaveCount(0);
    const context = page.getByRole(viewport.width >= 1100 ? "complementary" : "dialog", { name: "Info", exact: true });
    await expect(context).toBeVisible();
    const metadata = context.locator(".memory-detail-metadata-list");
    await expect(metadata.getByRole("combobox", { name: "Type", exact: true })).toHaveValue("person");
    await expect(metadata.getByText("Wenlan", { exact: true })).toBeVisible();
    await expect(metadata).toContainText("research-agent");
    await expect(detail.locator(".entity-obs-content")).toHaveText("Wrote the first published algorithm");
    await expect(context.locator(".entity-relation-row")).toHaveCount(2);
    await expect(detail.getByRole("button", { name: "Add information", exact: true })).toBeVisible();
    await expect(context.getByRole("button", { name: "Full screen", exact: true })).toBeVisible();
    const typography = await metadata.locator(".memory-detail-metadata-value").first().evaluate((node) => ({
      fontSize: Number.parseFloat(getComputedStyle(node).fontSize),
      width: node.getBoundingClientRect().width,
      scrollWidth: node.scrollWidth,
    }));
    expect(typography.fontSize, "topic metadata must remain readable").toBeGreaterThanOrEqual(13);
    expect(typography.scrollWidth).toBeLessThanOrEqual(typography.width + 1);
    const bounds = await Promise.all([
      detail.getByRole("heading", { level: 1, name: "Ada Lovelace" }),
      detail.locator(".entity-detail-reading"),
      context,
      metadata,
    ].map((surface) => surface.evaluate((node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, width: box.width };
    })));
    for (const box of bounds) {
      expect(box.width).toBeGreaterThan(0);
      expect(box.left).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(viewport.width + 1);
    }
    const contrast = await renderedContrast(page, [
      { selector: ".entity-detail-dossier .page-detail-title", label: "Topic title", foregroundProperty: "color", minimum: 4.5 },
      { selector: ".entity-detail-dossier .page-detail-actions-menu-trigger", label: "Topic actions", foregroundProperty: "color", minimum: 4.5 },
      { selector: ".page-info-drawer .memory-detail-metadata-value", label: "Topic panel metadata", foregroundProperty: "color", minimum: 4.5 },
      { selector: ".entity-detail-dossier .entity-obs-content", label: "Topic observation", foregroundProperty: "color", minimum: 4.5 },
    ]);
    for (const result of contrast) expect(result.ratio, result.label).toBeGreaterThanOrEqual(result.minimum);
  } else if (spaceDetail) {
    const dossier = page.locator(".space-dossier");
    await expect(dossier.getByRole("heading", { level: 1, name: "Wenlan", exact: true })).toBeVisible();
    await expect(dossier.getByRole("textbox", { name: "Edit description" })).toHaveValue("Editorial memory system");
    await expect(dossier.locator(".space-dossier-metrics")).toHaveCount(0);
    const recent = dossier.getByRole("region", { name: "Pages", exact: true });
    await expect(recent.locator(".space-dossier-page-list > button")).toHaveCount(6);
    await expect(recent.getByRole("button").first()).toContainText("Fixture architecture");
    await expect(recent.locator(".space-dossier-page-summary")).toHaveCount(6);
    await expect(recent.locator(".space-dossier-page-meta, time")).toHaveCount(0);
    await expect(dossier.locator(".space-dossier-rail")).toHaveCount(0);
    await expect(dossier.locator(".space-dossier-disclosure,.space-dossier-archive,.space-dossier-entities,.space-dossier-review")).toHaveCount(0);
    await expect(dossier.getByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
    await expect(dossier.getByRole("tab", { name: "Sources" })).toBeVisible();
    await dossier.getByRole("button", { name: "Actions for Wenlan" }).click();
    await expect(dossier.getByRole("menuitem", { name: "Review page changes" })).toBeVisible();
    await page.keyboard.press("Escape");
    await page.locator("main").evaluate((node) => { node.scrollTop = 0; });
    await expect.poll(() => page.locator("main").evaluate((node) => node.scrollTop)).toBe(0);
    const create = dossier.getByRole("button", { name: "New page", exact: true });
    await expect(create).toBeVisible();
    const control = await create.evaluate((node) => ({
      fontSize: Number.parseFloat(getComputedStyle(node).fontSize),
      height: node.getBoundingClientRect().height,
    }));
    expect(control.fontSize, "Space creation uses the shared readable control role").toBeGreaterThanOrEqual(14);
    expect(control.height).toBeGreaterThanOrEqual(32);
    const bounds = await dossier.locator("h1, .space-dossier-actions, .space-dossier-page-list > button, .space-dossier-disclosure > summary").evaluateAll((nodes) => nodes.map((node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, width: box.width, scrollWidth: node.scrollWidth };
    }));
    for (const box of bounds) {
      expect(box.width).toBeGreaterThan(0);
      expect(box.left).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(viewport.width + 1);
      expect(box.scrollWidth).toBeLessThanOrEqual(box.width + 1);
    }
    const recentBox = (await recent.boundingBox())!;
    const tabsBox = (await dossier.locator(".space-project-tabs").boundingBox())!;
    expect(recentBox.y, "pages follow the two-tab project navigation at every width").toBeGreaterThanOrEqual(tabsBox.y + tabsBox.height);
    const contrast = await renderedContrast(page, [
      { selector: ".space-dossier h1", label: "Space title", foregroundProperty: "color", minimum: 4.5 },
      { selector: ".space-dossier-new-page", label: "Space creation control", foregroundProperty: "color", minimum: 4.5 },
      { selector: ".space-dossier-page-title", label: "Space page title", foregroundProperty: "color", minimum: 4.5 },
    ]);
    for (const result of contrast) expect(result.ratio, result.label).toBeGreaterThanOrEqual(result.minimum);
    const sidebar = page.locator('aside[aria-label="Primary navigation"]');
    if (await sidebar.getAttribute("aria-hidden") === "false") {
      const navigation = page.getByRole("navigation", { name: "Primary navigation" });
      await expect(navigation.locator('[aria-current="page"]')).toHaveCount(1);
      await expect(navigation.getByRole("button", { name: "Spaces", exact: true })).toHaveAttribute("aria-current", "page");
      const navFonts = await navigation.getByRole("button").evaluateAll((nodes) => nodes.map((node) => Number.parseFloat(getComputedStyle(node).fontSize)));
      for (const font of navFonts) expect(font, "shared navigation must remain readable").toBeGreaterThanOrEqual(13);
    }
  } else if (spaces) {
    await expect(page.locator(".spaces-suggestions")).not.toHaveAttribute("open");
    await expect(page.locator("summary").filter({ hasText: "Suggested (2)" })).toBeVisible();
    await expect(page.getByTestId("space-row-space-suggested")).toBeHidden();
    const firstRow = page.getByTestId("space-row-space-wenlan");
    await expect(firstRow.getByRole("button", { name: "Wenlan", exact: true })).toBeVisible();
    const box = (await firstRow.boundingBox())!;
    expect(box.y, "confirmed Spaces must appear in the first half of the window").toBeLessThan(viewport.height / 2);
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(viewport.width);
    const controls = page.locator(".spaces-header-actions");
    expect((await controls.boundingBox())!.height, "header actions must fit one row").toBeLessThan(60);
    const contrast = await renderedContrast(page, [
      { selector: ".spaces-overview h1", label: "Spaces title", foregroundProperty: "color", minimum: 4.5 },
      { selector: ".spaces-suggestions > summary", label: "Suggestions disclosure", foregroundProperty: "color", minimum: 4.5 },
    ]);
    for (const result of contrast) expect(result.ratio, result.label).toBeGreaterThanOrEqual(result.minimum);
  }
  return true;
}

async function capture(page: Page, name: string): Promise<void> {
  await page.mouse.move(1, 1);
  await page.evaluate(() => {
    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur();
    }
  });
  await page.waitForTimeout(32);
  await page.locator("main").evaluate((node) => {
    node.scrollLeft = 0;
    node.scrollTop = 0;
  });
  await expect.poll(() => page.locator("main").evaluate((node) => node.scrollTop)).toBe(0);
  await settle(page);
  const redesigned = await assertRedesignedSurface(page, name);
  await page.screenshot({ path: path.join(evidenceDir, `${name}.png`), fullPage: false });
  await test.info().attach(name, { path: path.join(evidenceDir, `${name}.png`), contentType: "image/png" });
  if (!redesigned) {
    await expect(page).toHaveScreenshot(`${name}.png`, {
      animations: "disabled",
      fullPage: false,
      maxDiffPixelRatio: 0.0015,
    });
  }
}

async function openSidebar(page: Page): Promise<void> {
  const sidebar = page.locator('aside[aria-label="Primary navigation"]');
  const overlay = await page.evaluate(() => window.matchMedia("(max-width: 899px)").matches);
  await expect(sidebar).toHaveCSS("position", overlay ? "fixed" : "relative");
  if (await sidebar.getAttribute("aria-hidden") === "true") {
    await page.getByTitle("Show sidebar").click();
  }
  await expect(sidebar).toHaveAttribute("aria-hidden", "false");
}

async function openSpaces(page: Page): Promise<void> {
  await openSidebar(page);
  await page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Spaces", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Spaces" })).toBeVisible();
  await expect(page.locator(".spaces-overview-header")).toBeVisible();
}

async function openWiki(page: Page): Promise<void> {
  await openSidebar(page);
  await page.getByRole("navigation", { name: "Primary navigation" }).getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Open a page" })).toBeVisible();
}

async function captureFourSurfaces(page: Page, label: string): Promise<void> {
  await openWiki(page);
  await capture(page, `pages-${label}`);
  await openSpaces(page);
  await capture(page, `spaces-${label}`);
  await page.getByTestId("space-row-space-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  await expect(page.getByRole("heading", { level: 1, name: "Wenlan" })).toBeVisible();
  await capture(page, `space-${label}`);
  await openSpaceEntity(page, "Ada Lovelace");
  await expect(page.getByRole("heading", { level: 1, name: "Ada Lovelace" })).toBeVisible();
  const topicContext = await openTopicContext(page);
  await capture(page, `entity-${label}`);
  // Close the Info drawer before switching to the next primary route.
  await topicContext.getByRole("button", { name: "Close", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(topicContext).toHaveCount(0);
}

test("captures the complete responsive and native-reference matrix", async ({ page }) => {
  test.setTimeout(240_000);
  // Given a fresh deterministic UI with fixed density and clean error capture.
  const browserErrors = collectBrowserErrors(page);
  await mkdir(evidenceDir, { recursive: true });
  await page.clock.setFixedTime(fixtureNow);
  await installTauriMock(page, { locale: "en", localStorage: { "wenlan-spaces-view-mode": "rows" }, rawActions: [] });
  await page.goto("/");
  expect(await page.evaluate(() => window.devicePixelRatio)).toBe(1);

  // When all responsive and native-reference surfaces are rendered.
  for (const viewport of [
    { width: 1280, height: 900, label: "1280x900" },
    { width: 768, height: 900, label: "768x900" },
    { width: 375, height: 812, label: "375x812" },
  ] as const) {
    await page.setViewportSize(viewport);
    await captureFourSurfaces(page, viewport.label);
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
    await captureFourSurfaces(page, `${viewport.label}-dark`);
    await page.evaluate(() => document.documentElement.setAttribute("data-theme", "light"));
  }

  await openSpaces(page);
  const wenlanRow = page.getByTestId("space-row-space-wenlan");
  const pageCount = wenlanRow.getByTestId("space-pages");
  await expect(pageCount).toHaveText("6 pages");
  await expect(pageCount).toBeHidden();
  await page.getByLabel("Filter spaces").focus();
  // Follow the live tab order through the collection controls to the first Space.
  const wenlanControl = wenlanRow.getByRole("button", { name: "Wenlan", exact: true });
  for (let step = 0; step < 8 && !(await wenlanControl.evaluate((node) => node === document.activeElement)); step++) {
    await page.keyboard.press("Tab");
  }
  await expect(wenlanControl).toBeFocused();
  const focusOutline = await wenlanControl.evaluate((node) => {
    const style = getComputedStyle(node);
    return { style: style.outlineStyle, width: Number.parseFloat(style.outlineWidth) };
  });
  expect(focusOutline.style).not.toBe("none");
  expect(focusOutline.width).toBeGreaterThanOrEqual(2);
  await wenlanRow.evaluate((node) => node.scrollIntoView({ block: "center" }));
  await settle(page);
  const targetedCapture = "spaces-375x812-inventory-metadata-focus.png";
  await page.screenshot({ path: path.join(evidenceDir, targetedCapture), fullPage: false });
  await assertRedesignedSurface(page, "spaces-375x812-inventory-metadata-focus");
  await test.info().attach("spaces-mobile-keyboard-focus", {
    path: path.join(evidenceDir, targetedCapture), contentType: "image/png",
  });
  await writeFile(path.join(evidenceRoot, "mobile-inventory-focus.json"), `${JSON.stringify({
    focusOutline,
    noteCount: await pageCount.innerText(),
    screenshot: path.join(evidenceDir, targetedCapture),
    viewport: { height: 812, width: 375 },
  }, null, 2)}\n`);

  await page.setViewportSize({ width: 1586, height: 992 });
  await openSidebar(page);
  await openWiki(page);
  await capture(page, "pages-native-1586x992");
  await page.setViewportSize({ width: 1635, height: 962 });
  await openSpaces(page);
  await capture(page, "spaces-native-1635x962");
  await page.setViewportSize({ width: 1586, height: 992 });
  await page.getByTestId("space-row-space-wenlan").getByRole("button", { name: "Wenlan", exact: true }).click();
  await capture(page, "space-native-1586x992");

  // Then all captures are viewport-only and the browser stayed error-free.
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});
