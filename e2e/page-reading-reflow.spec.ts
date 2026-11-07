// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { installTauriMock } from "./tauriMock";
import { openWikiNote } from "./helpers/wikiWorkspace";

test("opening and closing the inspector keeps the visible reading paragraph", async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 800 });
  const base = createSpacesNavigationFixture();
  const content = Array.from({ length: 45 }, (_, i) => `Paragraph ${i}: ${"A detailed research sentence with evidence and context. ".repeat(10)}`).join("\n\n");
  await installTauriMock(page, { locale: "en", rawActions: [], fixture: { ...base, pages: base.pages.map(p => p.title === "Fixture architecture" ? { ...p, content } : p) } });
  await page.goto("/");
  await openWikiNote(page, "Fixture architecture");
  const scroll = page.locator(".wiki-workspace-content");
  await scroll.evaluate(el => { el.style.overflowAnchor = "none"; el.scrollTop = 3000; });
  await page.waitForTimeout(100);
  const visible = await page.locator(".cm-line").evaluateAll((nodes) => {
    const bounds = document.querySelector(".wiki-workspace-content")!.getBoundingClientRect();
    return nodes.map(node => ({ text: node.textContent ?? "", rect: node.getBoundingClientRect() }))
      .find(({ text, rect }) => text.startsWith("Paragraph ") && rect.bottom > bounds.top && rect.top < bounds.bottom)?.text;
  });
  expect(visible).toBeTruthy();
  const target = page.locator(".cm-line").filter({ hasText: visible! });
  const before = await target.boundingBox();
  expect(before).not.toBeNull();
  const initialWidth = await page.locator(".page-detail-document").evaluate(el => el.getBoundingClientRect().width);
  const info = page.getByRole("button", { name: "Info", exact: true });
  await info.click();
  await expect(info).toHaveAttribute("aria-pressed", "true");
  await expect.poll(async () => page.locator(".page-detail-document").evaluate(el => el.getBoundingClientRect().width)).toBeLessThan(initialWidth - 10);
  await expect.poll(async () => (await target.boundingBox())!.y).toBeCloseTo(before!.y, 0);
  await info.click();
  await expect(info).toHaveAttribute("aria-pressed", "false");
  await expect.poll(async () => (await target.boundingBox())!.y).toBeCloseTo(before!.y, 0);
});
