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
  test(`the Folders list shows its page count and reaches the last page at 1280 ${mode}`, async ({ page }, info) => {
    const errors = collectBrowserErrors(page);
    const fixture = fixtureWithManyNotes();
    const listed = fixture.pages.filter((item) => (item.status === "active" || item.status === "draft") && item.creation_kind !== "entity").length;
    await installTauriMock(page, { locale: "en", rawActions: [], fixture, localStorage: {
      "wenlan-navigation-v1": JSON.stringify({ version: 1, visible: ["pages", "sources", "spaces", "memories", "entities", "graph"], sidebar: { visible: true, mode, width: 240 } }),
    } });
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto("/");
    const directory = await showWikiFolders(page);
    // The count sits on the title row, right after "Folders", as a bare number.
    const count = directory.locator(".wiki-workspace-mode-row .wiki-workspace-note-count");
    await expect(count).toHaveAttribute("title", `${listed} pages`);
    await expect(count.locator("[aria-hidden=true]")).toHaveText(`${listed}`);
    const title = await directory.locator(".wiki-workspace-mode select").boundingBox();
    const countBox = await count.boundingBox();
    expect(title).not.toBeNull(); expect(countBox).not.toBeNull();
    expect(countBox!.x).toBeGreaterThanOrEqual(title!.x + title!.width - 1);
    expect(countBox!.x - (title!.x + title!.width)).toBeLessThanOrEqual(8);
    expect(Math.abs((countBox!.y + countBox!.height / 2) - (title!.y + title!.height / 2))).toBeLessThanOrEqual(2);
    await expect(directory.getByRole("button", { name: "Open Root page 01", exact: true })).toBeVisible();
    await expect(directory.getByRole("button", { name: "Open Entity shadow", exact: true })).toHaveCount(0);
    await page.mouse.move(0, 0);
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: info.outputPath(`folders-count-1280-${mode}-top.png`) });

    const scroll = directory.locator(".notes-list-scroll");
    await scroll.evaluate((element) => { element.scrollTop = element.scrollHeight; });
    const last = directory.getByRole("button", { name: "Open Root page 60", exact: true });
    await expect(last).toBeInViewport();
    const scrollBox = await scroll.boundingBox();
    const lastBox = await last.boundingBox();
    expect(scrollBox).not.toBeNull(); expect(lastBox).not.toBeNull();
    expect(lastBox!.y + lastBox!.height).toBeLessThanOrEqual(scrollBox!.y + scrollBox!.height + 1);
    await page.screenshot({ path: info.outputPath(`folders-count-1280-${mode}-bottom.png`) });
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
