// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

// Desktop detail views open Info without its toggle, so no focused control
// opened the pane. Escape from inside it must still land on the toggle, the
// only control that reopens it, instead of dropping the reader to the body.
async function expectEscapeReturnsToToggle(page: Page) {
  const toggle = page.getByRole("button", { name: "Info", exact: true });
  const pane = page.getByRole("complementary", { name: "Info", exact: true });
  await expect(toggle).toHaveAttribute("aria-expanded", "true");
  await expect(pane).toBeVisible();
  // Opening by default leaves focus in the reader.
  await expect(toggle).not.toBeFocused();
  await pane.getByRole("button", { name: "Close", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(pane).toHaveCount(0);
  await expect(toggle).toBeFocused();
}

async function setup(page: Page) {
  await page.setViewportSize({ width: 1280, height: 900 });
  const errors = collectBrowserErrors(page);
  await installTauriMock(page, { fixture: createSpacesNavigationFixture(), locale: "en", rawActions: [] });
  return errors;
}

test("topic Info opened by default returns Escape focus to its toggle", async ({ page }) => {
  const errors = await setup(page);
  await page.goto("/");
  await openPrimaryDestination(page, "Topics", "More");
  await page.getByRole("button", { name: "Ada Lovelace", exact: true }).click();
  const pane = page.getByRole("complementary", { name: "Info", exact: true });
  // A nested overlay hands focus back into the pane first, as a reader would.
  const graph = pane.getByRole("button", { name: "Full screen", exact: true });
  await graph.click();
  await page.keyboard.press("Escape");
  await expect(graph).toBeFocused();
  await expectEscapeReturnsToToggle(page);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("memory Info opened by default returns Escape focus to its toggle", async ({ page }) => {
  const errors = await setup(page);
  await page.goto("/");
  await openPrimaryDestination(page, "Memories", "More");
  await page.getByRole("article", { name: "Fixture architecture", exact: true }).getByRole("button", { name: /^Open memory/ }).first().click();
  await expectEscapeReturnsToToggle(page);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("source folder and document Info opened by default return Escape focus to their toggles", async ({ page }) => {
  const errors = await setup(page);
  const sources = [{ id: "vault", source_type: "obsidian", path: "/fixture/Reading Vault", status: "Active", last_sync: 1791680000, file_count: 1, memory_count: 1 }];
  const files = [{ source: "memory", source_agent: "obsidian", source_id: "vault::/fixture/Reading Vault/Books/Slow reading.md::0", title: "Slow reading.md", chunk_count: 1, last_modified: 1791680000 }];
  const directories: Record<string, { name: string; isDirectory: boolean }[]> = {
    "/fixture/Reading Vault": [{ name: "Books", isDirectory: true }],
    "/fixture/Reading Vault/Books": [{ name: "Slow reading.md", isDirectory: false }],
  };
  await page.exposeBinding("__containerInvoke", (_source, command: string, args: { path?: string } = {}) => {
    if (command === "list_registered_sources") return sources;
    if (command === "list_indexed_files") return files;
    if (command === "read_source_dir") return directories[args.path ?? ""] ?? [];
    if (command === "read_text_file") return "# Slow reading\n\nRead with a question.";
    throw new Error(`Unexpected container command: ${command}`);
  });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = window.__TAURI_INTERNALS__!, original = bridge.invoke;
    bridge.invoke = (command, args) => ["list_registered_sources", "list_indexed_files", "read_source_dir", "read_text_file"].includes(command)
      ? (window as unknown as { __containerInvoke: typeof original }).__containerInvoke(command, args) : original(command, args);
  });
  await openPrimaryDestination(page, "Sources", "More");
  await page.getByRole("button", { name: /Reading Vault/ }).first().click();
  const container = page.locator(".source-container-workspace").first();
  await expect(container).toBeVisible();
  await expectEscapeReturnsToToggle(page);
  await container.locator(".source-library-row").filter({ hasText: "Books" }).first().click();
  await container.locator(".source-library-row").filter({ hasText: "Slow reading.md" }).first().click();
  await expect(page.locator(".source-document-page")).toBeVisible();
  await expectEscapeReturnsToToggle(page);
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
