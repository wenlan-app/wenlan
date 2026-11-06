// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";
import { openMemoryFromPage } from "./helpers/primaryNavigation";
import { resources } from "../src/i18n/resources";

// A design token changed once must reach every detail page. This catches pages
// silently returning to their own typography or sidebar spacing after a refactor.
test("Memory, Topic and Wiki consume the same reading and Info template", async ({ page }) => {
  const base = createSpacesNavigationFixture();
  const t = resources.en.translation;
  const prose = "Keep observations and next steps together. Read the source before adding more detail.";
  const topic = { ...base.entities[0], name: "Reading layout", entity_type: "concept" };
  const fixture = {
    ...base,
    memories: base.memories.map((memory, index) => index === 0 ? { ...memory, content: prose } : memory),
    pages: base.pages.map((note, index) => index === 0 ? { ...note, content: `# ${note.title}\n\n${prose}` } : note),
    entities: [topic, ...base.entities.slice(1)],
    entityDetails: [{
      entity: topic,
      observations: [{ ...base.entityDetails[0].observations[0], entity_id: topic.id, content: prose }],
      relations: [],
    }, ...base.entityDetails.slice(1)],
  };
  await page.setViewportSize({ width: 1440, height: 900 });
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, {
    fixture, locale: "en", rawActions: [],
    localStorage: {
      "wenlan-theme": "light",
      "wenlan-navigation-v1": JSON.stringify({
        version: 2, visible: ["pages", "spaces", "graph", "sources", "memories", "entities"],
        sidebar: { visible: true, mode: "labels", width: 240 },
      }),
    },
  });
  await page.goto("/");
  const nav = page.getByRole("navigation", { name: t.sidebar.navigation });
  for (const kind of ["memory", "topic", "wiki"] as const) {
    if (kind === "memory") {
      // Memories have no place in the navigation: one opens from a page's sources.
      await openMemoryFromPage(page, "Fixture architecture", /Fixture architecture$/,
        { wiki: t.sidebar.pages, more: t.sidebar.more, info: t.pageInspector.info });
    } else if (kind === "topic") {
      await nav.getByRole("button", { name: t.sidebar.entities, exact: true }).click();
      await page.locator(".entities-view").getByRole("button", { name: topic.name, exact: true }).click();
    } else {
      await nav.getByRole("button", { name: t.sidebar.pages, exact: true }).click();
      await openWikiNote(page, "Fixture architecture");
    }
    const frame = page.locator("[data-detail-reading]");
    await expect(frame).toBeVisible();
    await expect(frame).toHaveAttribute("data-detail-reading", kind === "wiki" ? "long-form" : "compact");
    const panel = page.locator(".page-info-drawer:visible");
    if (!await panel.count()) {
      await page.getByRole("button", {
        name: kind === "wiki" ? t.pageInspector.info : t.knowledgeContext.memoryPanel, exact: true,
      }).click();
    }
    const info = panel.locator("[data-detail-info]");
    await expect(info).toBeVisible();
    const body = frame.locator(kind === "topic" ? ".entity-obs-content" : ".cm-line").filter({ hasText: prose }).first();
    const label = panel.locator(kind === "wiki" ? ".note-info-summary .note-info-section-heading" : ".page-info-drawer-header h2");
    const titleToken = kind === "wiki" ? "--detail-info-heading-size" : "--detail-info-title-size";
    await expect(body).toHaveCSS("font-size", "15px");
    await expect(info).toHaveCSS("gap", "24px");
    await expect(label).toHaveCSS("font-size", kind === "wiki" ? "15px" : "14px");
    await page.evaluate((token) => {
      const style = document.documentElement.style;
      style.setProperty("--reading-font-size", "17px");
      style.setProperty("--detail-info-section-gap", "28px");
      style.setProperty(token, "16px");
    }, titleToken);
    await expect(body).toHaveCSS("font-size", "17px");
    await expect(info).toHaveCSS("gap", "28px");
    await expect(label).toHaveCSS("font-size", "16px");
    await page.evaluate((titleToken) => {
      const style = document.documentElement.style;
      for (const token of ["--reading-font-size", "--detail-info-section-gap", titleToken]) {
        style.removeProperty(token);
      }
    }, titleToken);
    await expect(body).toHaveCSS("font-size", "15px");
    if (kind === "wiki") await page.getByRole("button", { name: t.pageInspector.info, exact: true }).click();
    else await panel.getByRole("button", { name: t.common.close, exact: true }).click();
  }
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
  expect(runtime.calls().filter(call => ["update_memory_cmd", "update_page", "update_entity_observation_cmd"].includes(call.command))).toHaveLength(0);
});
