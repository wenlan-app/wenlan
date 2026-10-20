// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import type { MemoryItem } from "../src/lib/tauri";
import { installTauriMock } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";

const SOURCE_ID = "reference-links-source";
const PAGE_ID = "page-architecture";
const MEMORY_ID = "memory-1";

const sourceMemory: MemoryItem = {
  source_id: SOURCE_ID,
  title: "Reference link fixture",
  content: "Read [Fixture architecture](#concept:page-architecture) and [the related memory](#memory:memory-1).",
  summary: null,
  memory_type: "decision",
  domain: "Wenlan",
  space: "Wenlan",
  source_agent: "codex",
  confidence: 0.9,
  confirmed: true,
  pinned: false,
  supersedes: null,
  last_modified: 1_783_728_000,
  chunk_count: 1,
};

test("canonical page and memory references preview automatically and navigate through Main", async ({ page }) => {
  const fixture = createSpacesNavigationFixture();
  const controller = await installTauriMock(page, {
    locale: "en",
    rawActions: [],
    memories: [sourceMemory, ...fixture.memories.filter((memory) => memory.source_id !== SOURCE_ID)],
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await openPrimaryDestination(page, "Memories");

  const row = page.getByRole("region", { name: "Memory list" })
    .getByRole("article", { name: /Reference link fixture/ });
  await row.getByRole("button", { name: "Open memory", exact: true }).click();
  const pageLink = page.getByRole("link", { name: "Fixture architecture", exact: true });
  await expect(pageLink).toHaveAttribute("href", `#concept:${PAGE_ID}`);
  await expect(pageLink).toHaveClass(/reference-link--page/);
  await expect(pageLink).toHaveAttribute("data-reference-resource-icon", "page");
  await pageLink.hover();
  const pagePreview = page.getByRole("dialog", { name: "Fixture architecture" });
  await expect(pagePreview).toBeVisible();
  await expect(pagePreview).toContainText("Page");
  expect(controller.calls().some((call) => call.command === "get_page" && (call.args as { id?: string })?.id === PAGE_ID)).toBe(true);
  expect(controller.calls().some((call) => call.command === "get_page_explicit_browse" && (call.args as { id?: string })?.id === PAGE_ID)).toBe(false);
  await pagePreview.hover();
  await pageLink.click();
  await expect(page.getByRole("heading", { level: 1, name: "Fixture architecture", exact: true })).toBeVisible();

  await openPrimaryDestination(page, "Memories");
  const secondRow = page.getByRole("region", { name: "Memory list" })
    .getByRole("article", { name: /Reference link fixture/ });
  await secondRow.getByRole("button", { name: "Open memory", exact: true }).click();
  const memoryLink = page.getByRole("link", { name: "the related memory", exact: true });
  await expect(memoryLink).toHaveAttribute("href", `#memory:${MEMORY_ID}`);
  await memoryLink.hover();
  const memoryPreview = page.getByRole("dialog", { name: /Fixture memory 1/ });
  await expect(memoryPreview).toBeVisible();
  await expect(memoryPreview).toContainText("Memory");
  expect(controller.calls().some((call) => call.command === "get_memory_detail" && (call.args as { sourceId?: string })?.sourceId === MEMORY_ID)).toBe(true);
  await memoryPreview.hover();
  await memoryLink.click();
  await expect(page.getByRole("main", { name: "Memory dossier" })).toContainText("Typed fixtures keep rendered browser journeys deterministic.");
});
