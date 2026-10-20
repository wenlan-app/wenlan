// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";

const languages = [
  { locale: "en", theme: "light", spaces: "Spaces", graph: "Graph", sources: "Sources", memories: "Memories", topics: "Topics", more: "More", pin: (name: string) => `Pin ${name} to sidebar`, unpin: (name: string) => `Unpin ${name} from sidebar` },
] as const;

async function capture(page: Page, name: string) {
  const directory = process.env.WENLAN_UI_EVIDENCE_DIR;
  if (!directory) return;
  await page.mouse.move(page.viewportSize()!.width - 1, 1);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations()
      .filter(animation => Number.isFinite(animation.effect?.getComputedTiming().endTime))
      .map(animation => animation.finished.catch(() => undefined)));
  });
  await page.screenshot({ path: `${directory}/navigation-preferences-${name}.png` });
}

async function expectPanelWithinViewport(page: Page) {
  await expect.poll(async () => {
    const panel = await page.locator(".notes-more-panel").boundingBox();
    const viewport = page.viewportSize()!;
    return panel !== null && panel.x >= 7 && panel.y >= 7
      && panel.x + panel.width <= viewport.width - 7
      && panel.y + panel.height <= viewport.height - 7;
  }).toBe(true);
}

async function panelPosition(page: Page) {
  const panel = await page.locator(".notes-more-panel").boundingBox();
  if (!panel) throw new Error("More panel is not open");
  return { x: panel.x, y: panel.y };
}

for (const copy of languages) {
  test(`${copy.locale} More pins destinations in place and persists across sizes`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width: 1280, height: 800 });
    await installTauriMock(page, {
      locale: copy.locale,
      rawActions: [],
      preserveLocalStorage: true,
      localStorage: { "wenlan-theme": copy.theme },
    });
    await page.goto("/");
    const nav = page.locator(".notes-rail-nav");
    const moreButton = nav.getByRole("button", { name: copy.more, exact: true });
    const railNames = () => nav.locator(".notes-primary-link, .notes-rail-button").evaluateAll(buttons => buttons.map(button => button.getAttribute("aria-label")));
    const morePanel = page.getByRole("group", { name: copy.more, exact: true });
    const row = (name: string) => morePanel.locator(".notes-more-destination").filter({ has: page.getByRole("button", { name, exact: true }) });
    const pin = (name: string) => row(name).getByRole("button", { name: copy.pin(name), exact: true });
    const unpin = (name: string) => row(name).getByRole("button", { name: copy.unpin(name), exact: true });
    const revealPin = async (name: string) => row(name).hover();
    const defaults = ["Wiki", copy.spaces, copy.graph, copy.sources, copy.more];
    await expect.poll(railNames).toEqual(defaults);
    await nav.getByRole("button", { name: "Wiki", exact: true }).click();
    await expect(page.getByRole("heading", { name: copy.locale === "en" ? "Open a page" : copy.locale === "zh-Hant" ? "開啟筆記" : "打开笔记", level: 1 })).toBeVisible();
    await capture(page, `${copy.locale}-${copy.theme}-default`);

    await moreButton.click();
    const moreRows = morePanel.locator(".notes-more-destination");
    await expect(moreRows).toHaveCount(2);
    expect(await morePanel.locator(".notes-more-destination-link").evaluateAll(buttons => buttons.map(button => button.getAttribute("aria-label"))))
      .toEqual([copy.memories, copy.topics]);
    await morePanel.getByRole("button", { name: copy.memories, exact: true }).click();
    await expect(page.getByRole("heading", { name: copy.memories, exact: true })).toBeVisible();
    await moreButton.click();
    const openedPosition = await panelPosition(page);
    await revealPin(copy.memories);
    await expect(pin(copy.memories)).toHaveAttribute("aria-pressed", "false");
    await pin(copy.memories).click();
    await expect.poll(() => panelPosition(page)).toEqual(openedPosition);
    await expect(unpin(copy.memories)).toBeFocused();
    await expect(unpin(copy.memories)).toHaveAttribute("aria-pressed", "true");
    await expect(morePanel).toBeVisible();
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.memories, copy.more]);
    await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);

    await unpin(copy.memories).click();
    await expect.poll(() => panelPosition(page)).toEqual(openedPosition);
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.more]);
    await expect(morePanel.getByRole("button", { name: copy.memories, exact: true })).toHaveAttribute("aria-current", "page");
    await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
    await expectPanelWithinViewport(page);
    await capture(page, `${copy.locale}-${copy.theme}-direct-pin`);
    await revealPin(copy.memories);
    await pin(copy.memories).click();
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.memories, copy.more]);
    await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
    await page.keyboard.press("Escape");
    await expect(morePanel).toHaveCount(0);
    await expect(moreButton).toBeFocused();
    await expect(moreButton).not.toHaveAttribute("aria-current");

    // A real reload must retain the user's choices, not reseed them through the fixture.
    await page.reload();
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.memories, copy.more]);
    expect(await page.evaluate(() => JSON.parse(localStorage.getItem("wenlan-navigation-v1")!))).toEqual({ version: 1, visible: ["pages", "spaces", "graph", "sources", "memories"], sidebar: { visible: true, mode: "labels" } });
    await nav.getByRole("button", { name: copy.graph, exact: true }).click();
    await expect(page.getByTestId("atlas-view")).toBeVisible();
    await expect(nav.locator(`.notes-primary-link[aria-label="${copy.graph}"]`)).toHaveAttribute("aria-current", "page");

    await moreButton.click();
    await revealPin(copy.memories);
    await expect(unpin(copy.memories)).toHaveAttribute("aria-pressed", "true");
    await revealPin(copy.topics);
    await pin(copy.topics).click();
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.memories, copy.topics, copy.more]);
    await page.setViewportSize({ width: 1280, height: 600 });
    await expectPanelWithinViewport(page);
    await capture(page, `${copy.locale}-${copy.theme}-all-pinned-short`);

    await page.keyboard.press("Escape");
    await page.setViewportSize({ width: 375, height: 812 });
    await expect(page.locator(".notes-workspace-sidebar")).toHaveAttribute("aria-hidden", "true");
    await page.locator("[data-sidebar-toggle]").click();
    await moreButton.click();
    await expectPanelWithinViewport(page);
    await capture(page, `${copy.locale}-${copy.theme}-all-pinned-narrow`);
    for (const name of [copy.memories, copy.topics]) {
      await unpin(name).click();
    }
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.more]);
    await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
    await expectPanelWithinViewport(page);
    await capture(page, `${copy.locale}-${copy.theme}-all-unpinned-narrow`);
    await page.keyboard.press("Escape");
    await expect(moreButton).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.locator(".notes-workspace-sidebar")).toHaveAttribute("aria-hidden", "true");
    await page.locator("[data-sidebar-toggle]").click();
    await moreButton.click();
    await revealPin(copy.memories);
    await pin(copy.memories).click();
    await expect.poll(railNames).toEqual(["Wiki", copy.spaces, copy.graph, copy.sources, copy.memories, copy.more]);
    await expect(morePanel).toBeVisible();
    await capture(page, `${copy.locale}-${copy.theme}-narrow-direct-pin`);
    await page.keyboard.press("Escape");
    await expect(moreButton).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.locator(".notes-workspace-sidebar")).toHaveAttribute("aria-hidden", "true");
    await expect(page.locator("[data-sidebar-toggle]")).toBeFocused();
    await page.locator("[data-sidebar-toggle]").click();
    await nav.locator('.notes-primary-link[aria-label="Wiki"]').click();
    await expect(page.getByRole("heading", { name: copy.locale === "en" ? "Open a page" : copy.locale === "zh-Hant" ? "開啟筆記" : "打开笔记", level: 1 })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
    await page.setViewportSize({ width: 1280, height: 800 });
    await expect(moreButton).toBeVisible();
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}

test("closing More keeps the page in its writing session", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  const nav = page.locator(".notes-rail-nav");
  const more = nav.getByRole("button", { name: "More", exact: true });
  await nav.getByRole("button", { name: "Wiki", exact: true }).click();
  await openWikiNote(page, "Fixture architecture");
  const editor = page.locator(".cm-content[contenteditable='true']");
  await expect(editor).toBeVisible();
  await expect(editor).toContainText("Fixture architecture");
  await expect(editor).toContainText("Deterministic content for the integrated Wenlan journey.");
  for (const narrow of [false, true]) {
    if (narrow) {
      await page.setViewportSize({ width: 375, height: 812 });
      await expect(page.locator(".notes-workspace-sidebar")).toHaveAttribute("aria-hidden", "true");
      await page.locator("[data-sidebar-toggle]").click();
    }
    await more.click();
    await expect(page.getByRole("group", { name: "More", exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("group", { name: "More", exact: true })).toHaveCount(0);
    await expect(more).toBeFocused();
    await expect(editor).toBeVisible();
    await expect(editor).toContainText("Fixture architecture");
    await expect(editor).toContainText("Deterministic content for the integrated Wenlan journey.");
    if (narrow) {
      await page.keyboard.press("Escape");
      await expect(page.locator(".notes-workspace-sidebar")).toHaveAttribute("aria-hidden", "true");
      await expect(editor).toBeVisible();
      await expect(page.locator("[data-sidebar-toggle]")).toBeFocused();
    }
  }
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
