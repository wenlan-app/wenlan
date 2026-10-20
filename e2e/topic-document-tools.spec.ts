// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openTopicContext } from "./helpers/topicTools";

const copies = [
  { locale: "en", more: "More", topics: "Topics", actions: "Topic actions", close: "Close", expand: "Full screen", edit: "Edit information", confirm: "Confirm topic" },
] as const;

async function ensureTopicContext(page: import("@playwright/test").Page) {
  const wide = (page.viewportSize()?.width ?? 0) >= 1100;
  const pane = page.getByRole(wide ? "complementary" : "dialog", { name: "Info", exact: true });
  const toggle = page.getByRole("button", { name: "Info", exact: true });
  // The toggle renders with the loaded topic, so reading its state waits for
  // that load. A one-shot pane count can run first on a slow load and mistake
  // the default-open pane for a closed one.
  if ((await toggle.getAttribute("aria-expanded")) === "true") {
    await expect(pane).toBeVisible();
    // Desktop topics open their Info inspector by default. Reopen it through
    // its own control so Escape has the same real trigger to restore focus to
    // as the narrow-window flow.
    if (wide) {
      await toggle.click();
      await expect(pane).toHaveCount(0);
      await toggle.click();
      await expect(pane).toBeVisible();
    }
    return pane;
  }
  return openTopicContext(page);
}

for (const copy of copies) for (const width of [1280, 375]) {
  test(`topic document tools preserve reading and nested context: ${copy.locale} ${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const errors = collectBrowserErrors(page);
    const controller = await installTauriMock(page, {
      fixture: createSpacesNavigationFixture(), locale: copy.locale, rawActions: [],
    });
    await page.goto("/");
    await openPrimaryDestination(page, copy.topics, copy.more);
    await page.getByRole("button", { name: "Ada Lovelace", exact: true }).click();
    const title = page.getByRole("heading", { name: "Ada Lovelace", exact: true, level: 1 });
    const actions = page.getByRole("button", { name: copy.actions, exact: true });
    await expect(title).toBeVisible();
    await expect(actions).toBeInViewport();
    const titleBox = (await title.boundingBox())!;
    const actionBox = (await actions.boundingBox())!;
    expect(Math.abs(titleBox.y + titleBox.height / 2 - actionBox.y - actionBox.height / 2)).toBeLessThanOrEqual(4);
    await expect(page.locator(".entity-dossier-seal, .entity-dossier-dateline")).toHaveCount(0);
    if (width >= 1100) await expect(page.locator(".entity-topic-context")).toBeVisible();
    else await expect(page.locator(".entity-topic-context")).toHaveCount(0);
    await expect(page.getByRole("button", { name: copy.confirm, exact: true })).toHaveCount(0);
    const note = page.getByRole("button", { name: /Edit information — Wrote the first published algorithm/ });
    await expect(note).toBeVisible();
    await actions.press("ArrowDown");
    await expect(page.getByRole("menuitem", { name: "Archive", exact: true })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(actions).toBeFocused();

    const pane = await ensureTopicContext(page);
    const graph = pane.getByRole("button", { name: copy.expand, exact: true });
    await graph.click();
    const overlay = page.getByRole("dialog", { name: copy.expand, exact: true });
    await expect(overlay).toBeVisible();
    await expect(overlay.getByRole("button", { name: copy.close, exact: true })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(overlay).toHaveCount(0);
    await expect(pane).toBeVisible();
    await expect(graph).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(pane).toHaveCount(0);
    // Info is opened by its own toolbar toggle, so Escape returns focus there.
    await expect(page.getByRole("button", { name: "Info", exact: true })).toBeFocused();
    await expect(title).toBeVisible();
    expect(controller.calls().filter(call => /^(confirm_entity|archive_entities|restore_entities)_cmd$/.test(call.command))).toEqual([]);

    // Preserve the existing direct edit/blur-save contract and same-page draft.
    await note.click();
    const editor = page.getByRole("textbox", { name: copy.edit, exact: true });
    await editor.fill("Topic page saved before opening context.");
    await ensureTopicContext(page);
    await pane.getByRole("button", { name: copy.close, exact: true }).click();
    await expect(page.getByRole("button", { name: /Edit information — Topic page saved before opening context\./ })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}

for (const width of [1280, 375]) test(`nested Atlas filters remain visible and keep keyboard ownership at ${width}`, async ({ page }) => {
  await page.setViewportSize({ width, height: 900 });
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { fixture: createSpacesNavigationFixture(), locale: "en", rawActions: [] });
  await page.goto("/");
  await openPrimaryDestination(page, "Topics", "More");
  await page.getByRole("button", { name: "Ada Lovelace", exact: true }).click();
  const context = await ensureTopicContext(page);
  await context.getByRole("button", { name: "Full screen", exact: true }).click();
  const graph = page.getByRole("dialog", { name: "Full screen", exact: true });
  await graph.getByRole("group", { name: "Graph view", exact: true }).getByRole("button", { name: "Atlas", exact: true }).click();
  const displayTrigger = graph.getByRole("button", { name: "Display", exact: true });
  await displayTrigger.click();
  const display = graph.getByRole("group", { name: "Show in graph", exact: true });
  const filter = display.getByRole("button", { name: "Filter topic types", exact: true });
  await filter.click();
  const types = graph.getByRole("dialog", { name: "Topic types", exact: true });
  await expect(types).toBeVisible();
  await expect(types).toBeFocused();
  const person = types.getByRole("button", { name: "Person", exact: true });
  // Real hit testing verifies the portal is on top, not merely in the DOM.
  expect(await person.evaluate((node) => {
    const box = node.getBoundingClientRect();
    return node.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
  })).toBe(true);
  await page.keyboard.press("Tab");
  await expect(types.getByRole("button", { name: "Close type filters", exact: true })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(person).toBeFocused();
  await page.keyboard.press("Space");
  await expect(person).toHaveAttribute("aria-pressed", "false");
  await page.keyboard.press("Escape");
  await expect(types).toHaveCount(0);
  await expect(filter).toBeFocused();
  await expect(display).toBeVisible();
  await page.keyboard.press("Escape");
  // The enclosing full-screen dialog owns Escape once the nested filter has
  // closed, even while Display is still expanded.
  await expect(graph).toHaveCount(0);
  await expect(context.getByRole("button", { name: "Full screen", exact: true })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(context).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Info", exact: true })).toBeFocused();
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
