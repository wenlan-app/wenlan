// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { createReviewDecisionFixture } from "./fixtures/reviewDecisions";
import { showWikiFolders } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

// Sixty root notes, every third one distilled about an entity (so it carries
// entity_id), plus one entity shadow page that stays out of the tree.
function fixtureWithManyNotes() {
  const base = createReviewDecisionFixture("wiki-folders");
  const template = base.pages[0];
  const notes = Array.from({ length: 60 }, (_, index) => ({
    ...template,
    id: `page-root-${index}`,
    title: `Root page ${String(index + 1).padStart(2, "0")}`,
    content: `# Root page ${index + 1}`,
    status: "active",
    creation_kind: "distilled",
    entity_id: index % 3 === 0 ? `entity-root-${index}` : null,
    storage_path: `root-note-${index + 1}.md`,
    folder_path: null,
    last_modified: new Date(Date.UTC(2026, 0, 1) - index * 60_000).toISOString(),
  }));
  const shadow = { ...template, id: "page-entity-shadow", title: "Entity shadow", creation_kind: "entity", entity_id: "entity-root-0", storage_path: null, folder_path: null };
  return { ...base, pages: [...base.pages, ...notes, shadow] };
}

for (const mode of ["labels", "icons"] as const) {
  test(`the Docs row shows its page count and the list reaches the last page at 1280 ${mode}`, async ({ page }, info) => {
    const errors = collectBrowserErrors(page);
    const fixture = fixtureWithManyNotes();
    const listed = fixture.pages.filter((item) => (item.status === "active" || item.status === "draft") && item.creation_kind !== "entity").length;
    await installTauriMock(page, { locale: "en", rawActions: [], fixture, localStorage: {
      "wenlan-navigation-v1": JSON.stringify({ version: 2, visible: ["pages", "sources", "spaces", "memories", "entities", "graph"], sidebar: { visible: true, mode, width: 240 } }),
    } });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");
    const directory = await showWikiFolders(page);
    // The count sits on the Docs row, at its right, as a bare number; the
    // icon rail has no room for it and the list page needs none.
    const row = page.locator(".notes-rail-nav").getByRole("button", { name: "Docs", exact: true });
    if (mode === "labels") {
      const count = row.locator(".notes-primary-count");
      await expect(count).toHaveText(`${listed}`);
      await expect(row).toHaveAttribute("aria-description", `Docs, ${listed} pages`);
      const rowBox = await row.boundingBox();
      const countBox = await count.boundingBox();
      expect(rowBox).not.toBeNull(); expect(countBox).not.toBeNull();
      expect(countBox!.x + countBox!.width).toBeLessThanOrEqual(rowBox!.x + rowBox!.width);
      expect(countBox!.x).toBeGreaterThan(rowBox!.x + rowBox!.width / 2);
    } else {
      await expect(page.locator(".notes-rail-nav .notes-primary-count:visible")).toHaveCount(0);
    }
    await expect(directory.locator(".wiki-workspace-note-count")).toHaveCount(0);
    await expect(directory.getByRole("button", { name: "Open Root page 01", exact: true })).toBeVisible();
    await expect(directory.getByRole("button", { name: "Open Entity shadow", exact: true })).toHaveCount(0);
    await page.mouse.move(0, 0);
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: info.outputPath(`folders-count-1280-${mode}-top.png`) });

    const last = directory.getByRole("button", { name: "Open Root page 60", exact: true });
    await last.scrollIntoViewIfNeeded();
    await expect(last).toBeInViewport({ ratio: 1 });
    await page.screenshot({ path: info.outputPath(`folders-count-1280-${mode}-bottom.png`) });
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
