// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { resources } from "../src/i18n/resources";
import type { RegisteredSource, IndexedFileInfo } from "../src/lib/tauri";

for (const cfg of [
  { locale: "en", theme: "light", width: 1440 },
  { locale: "zh-Hant", theme: "dark", width: 1280 },
  { locale: "zh-Hant", theme: "light", width: 375 },
] as const) {
  test(`containers share Sources browsing and document reading ${cfg.locale} ${cfg.width}`, async ({ page }, info) => {
    const t = resources[cfg.locale].translation;
    const errors = collectBrowserErrors(page);
    await installTauriMock(page, { fixture: createSpacesNavigationFixture(), locale: cfg.locale, rawActions: [], localStorage: {
      "wenlan-theme": cfg.theme,
      "wenlan-navigation-v1": JSON.stringify({ version: 1, visible: ["pages", "spaces", "sources", "memories", "entities", "graph"], sidebar: { visible: false, mode: "labels", width: 240 } }),
    } });
    await page.setViewportSize({ width: cfg.width, height: 900 });
    const sources: RegisteredSource[] = [
      { id: "vault", source_type: "obsidian", path: "/fixture/Reading Vault", status: "Active", last_sync: 1791680000, file_count: 2, memory_count: 3 },
      { id: "folder", source_type: "directory", path: "/fixture/Research", status: "Active", last_sync: 1791680000, file_count: 1, memory_count: 1 },
    ];
    const files: IndexedFileInfo[] = [
      { source: "file", source_id: "guide.pdf", title: "Research guide.pdf", chunk_count: 1, last_modified: 1791680000 },
      { source: "webpage", source_id: "https://example.org/reading", title: "Reading reference", chunk_count: 1, last_modified: 1791680000 },
      { source: "memory", source_agent: "obsidian", source_id: "vault::/fixture/Reading Vault/Books/Slow reading.md::0", title: "Slow reading.md", chunk_count: 2, last_modified: 1791680000 },
    ];
    const directories: Record<string, {name: string; isDirectory: boolean}[]> = {
      "/fixture/Reading Vault": [{ name: "Books", isDirectory: true }, { name: "Home.md", isDirectory: false }],
      "/fixture/Reading Vault/Books": [{ name: "Slow reading.md", isDirectory: false }],
      "/fixture/Research": [{ name: "Outline.txt", isDirectory: false }],
    };
    let failDirectory = false;
    const calls: { command: string; args: any }[] = [];
    await page.exposeBinding("__containerInvoke", (_source, command: string, args: any = {}) => {
      calls.push({ command, args });
      if (command === "list_registered_sources") return sources;
      if (command === "list_indexed_files") return files;
      if (command === "read_source_dir") {
        if (failDirectory && args.path === "/fixture/Research") throw new Error("Folder temporarily unavailable");
        return directories[args.path] ?? [];
      }
      if (command === "read_text_file") return "# Slow reading\n\nFull local-file text, including the final paragraph beyond indexed passages.\n\n> Read with a question.\n\n- Keep the context\n- Revisit the source\n\n## Next step\n\nThe final paragraph remains readable.";
      if (command === "sync_registered_source") return { files_found: 2, ingested: 0, skipped: 2, errors: 0 };
      if (command === "plugin:dialog|open") return null;
      throw new Error(`Unexpected container command: ${command}`);
    });
    await page.goto("/");
    await page.evaluate(() => {
      const bridge = window.__TAURI_INTERNALS__!, original = bridge.invoke;
      bridge.invoke = (command, args) => ["list_registered_sources", "list_indexed_files", "read_source_dir", "read_text_file", "sync_registered_source", "plugin:dialog|open"].includes(command)
        ? (window as any).__containerInvoke(command, args) : original(command, args);
    });
    await openPrimaryDestination(page, t.sidebar.sources, t.sidebar.more);
    const capture = async (name: string) => {
      await page.mouse.move(0, 0);
      await page.evaluate(() => document.fonts.ready);
      await expect(page.locator("vite-error-overlay")).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
      await page.screenshot({ path: info.outputPath(name) });
    };
    async function containerBrowsing(title: string) {
      const main = page.locator(".source-container-main");
      const frame = main.locator(".collection-frame");
      const breadcrumb = frame.locator(".source-collection-breadcrumbs");
      const toolbar = frame.locator(".collection-toolbar");
      await expect(breadcrumb.locator('[aria-current="page"]')).toHaveText(title);
      await expect(main.locator(".reading-document-frame,.page-detail-title")).toHaveCount(0);
      const bounds = (await frame.boundingBox())!;
      const lane = (await main.boundingBox())!;
      const path = (await breadcrumb.boundingBox())!;
      const controls = (await toolbar.boundingBox())!;
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(lane.x + lane.width + 1);
      expect(path.y - bounds.y).toBe(24);
      expect(controls.x).toBe(path.x);
      expect(controls.y - path.y - path.height).toBe(20);
      if (cfg.width >= 1100) {
        const inspector = page.locator(".page-info-drawer");
        await expect(inspector).toBeVisible();
        const start = controls.x;
        await inspector.locator(".page-info-drawer-close").click();
        expect(Math.abs((await toolbar.boundingBox())!.x - start)).toBeLessThanOrEqual(1);
        expect((await toolbar.boundingBox())!.width).toBeGreaterThanOrEqual(controls.width);
        await page.getByRole("button", { name: t.knowledgeContext.memoryPanel, exact: true }).click();
        await expect(inspector).toBeVisible();
        expect(Math.abs((await toolbar.boundingBox())!.x - start)).toBeLessThanOrEqual(1);
      }
    }
    const root = page.locator(".source-library");
    await expect(root.locator(".source-library-row")).toHaveCount(4);
    await expect(root.locator(".source-library-icon-obsidian")).toHaveCount(1);
    const vaultLogo = root.locator('img[data-resource-icon="obsidian"]');
    await expect(vaultLogo).toHaveCount(1);
    await expect.poll(() => vaultLogo.evaluate((el: HTMLImageElement) => el.complete && el.naturalWidth > 0)).toBe(true);
    await expect(root).not.toContainText("Slow reading.md");
    await expect(page.getByRole("combobox")).toHaveCount(0);
    await capture("sources-root.png");
    await root.getByRole("searchbox", { name: t.sourceLibrary.search, exact: true }).fill("Slow reading");
    await expect(root.locator(".source-library-row")).toHaveCount(1);
    await expect(root.locator(".source-library-row")).toContainText("Slow reading.md");
    await root.getByRole("searchbox", { name: t.sourceLibrary.search, exact: true }).fill("");
    await root.getByRole("searchbox", { name: t.sourceLibrary.search, exact: true }).fill("Reading Vault");
    await root.getByRole("button", { name: /Reading Vault/ }).click();
    const container = page.locator(".source-container-workspace").first();
    await expect(container.locator(".source-library-row")).toHaveCount(2);
    await expect(page.locator(".source-folder-tree")).toHaveCount(0);
    const inspector = page.locator(".page-info-drawer");
    if (cfg.width >= 1100) {
      await expect(inspector).toBeVisible();
      await expect(inspector).toContainText(t.sourceContainer.vaultReads);
      await expect(inspector).toContainText(t.sourceContainer.folderChecks);
      await inspector.getByRole("button", { name: t.sourceContainer.refresh, exact: true }).click();
      expect(calls.filter(c => c.command === "sync_registered_source").map(c => c.args)).toEqual([{ id: "vault" }]);
    } else await expect(inspector).toHaveCount(0);
    await containerBrowsing("Reading Vault");
    await capture("vault-root.png");
    await container.getByRole("searchbox", { name: t.sourceLibrary.search, exact: true }).fill("missing name");
    await expect(container.locator(".source-container-empty")).toContainText(t.sourceLibrary.noMatches);
    await expect(container.locator(".source-container-empty")).not.toContainText(t.sourceContainer.empty);
    await container.getByRole("button", { name: t.sourceLibrary.clearFilters, exact: true }).click();
    await expect(container.locator(".source-library-row")).toHaveCount(2);
    await container.getByRole("searchbox", { name: t.sourceLibrary.search, exact: true }).fill("Books");
    await container.getByRole("button", { name: /^Books/ }).click();
    await expect(container.locator(".source-collection-breadcrumbs")).toContainText("Books");
    await expect(container.locator(".source-library-row")).toHaveCount(1);
    await page.getByTestId("asset-lens-cards").click();
    await expect(container.locator(".source-library-list")).toHaveAttribute("data-lens", "cards");
    await containerBrowsing("Books");
    await capture("vault-child-cards.png");
    await page.getByTestId("asset-lens-rows").click();
    await container.getByRole("button", { name: /Slow reading.md/ }).click();
    await expect(page.locator(".source-document-page-header h1")).toHaveText("Slow reading.md");
    const body = page.locator(".source-document-body");
    await expect(body).toContainText("The final paragraph remains readable.");
    if (cfg.width >= 1100) {
      const origin = await page.locator(".source-document-page > .reading-document-frame").boundingBox();
      const title = await page.locator(".source-document-page-header h1").boundingBox();
      expect(Math.abs(title!.y - origin!.y - 72)).toBeLessThanOrEqual(1);
      expect(Math.abs(title!.x - origin!.x - 32)).toBeLessThanOrEqual(1);
      const start = (await body.boundingBox())!.x;
      // Info squeezes the lane at these widths, so the column re-centres when
      // it closes; reopening Info must bring it back to the same place.
      await inspector.locator(".page-info-drawer-close").click();
      await expect(inspector).toHaveCount(0);
      await page.getByRole("button", { name: t.knowledgeContext.memoryPanel, exact: true }).click();
      await expect(inspector).toBeVisible();
      expect(Math.abs((await body.boundingBox())!.x - start)).toBeLessThanOrEqual(1);
    }
    if (cfg.width >= 1100) {
      await expect(inspector).toContainText(t.sourceAccess.inLibrary);
      await expect(inspector).toContainText(t.sourceContainer.connection);
    }
    await expect(body.locator("blockquote")).toContainText("Read with a question.");
    await expect(body.locator("h1")).toHaveCount(0);
    expect(await body.evaluate(el => ({ fontSize: getComputedStyle(el).fontSize, lineHeight: getComputedStyle(el).lineHeight }))).toEqual({ fontSize: "15px", lineHeight: "26.25px" });
    await capture("local-file-reader.png");
    await page.getByRole("button", { name: t.sourceContainer.backToFolder, exact: true }).click();
    await expect(container.locator(".source-collection-breadcrumbs")).toContainText("Books");
    await container.locator(".source-collection-breadcrumbs").getByRole("button", { name: t.sourceContainer.root, exact: true }).click();
    await expect(root).toBeVisible();
    await root.getByRole("button", { name: t.sourceLibrary.filters.folders, exact: true }).click();
    await root.locator("button[data-source-key=\"container:folder\"]").click();
    await expect(container.locator(".source-library-row")).toHaveCount(1);
    if (cfg.width >= 1100) await expect(inspector).toContainText(t.sourceContainer.folderChecks);
    await containerBrowsing("Research");
    await capture("folder-root.png");
    await container.locator(".source-collection-breadcrumbs").getByRole("button", { name: t.sourceContainer.root, exact: true }).click();
    failDirectory = true;
    await root.locator("button[data-source-key=\"container:folder\"]").click();
    await expect(container.getByRole("alert")).toContainText(t.sourceContainer.folderReadError);
    await expect(container).not.toContainText(t.sourceContainer.empty);
    failDirectory = false;
    await container.getByRole("button", { name: t.sourceContainer.retryFolder, exact: true }).click();
    await expect(container.locator(".source-library-row")).toHaveCount(1);
    await container.locator(".source-collection-breadcrumbs").getByRole("button", { name: t.sourceContainer.root, exact: true }).click();
    await page.getByRole("button", { name: t.sourceLibrary.add, exact: true }).click();
    const addMenu = page.getByRole("dialog", { name: t.sourceAccess.addTitle, exact: true });
    await expect(addMenu.locator(".source-add-option").first()).toContainText(t.sourceAccess.obsidian);
    await capture("add-source-menu.png");
    await addMenu.getByRole("button", { name: new RegExp(t.sourceAccess.obsidian) }).click();
    expect(calls.filter(c => c.command === "plugin:dialog|open").at(-1)?.args.options).toMatchObject({ directory: true, multiple: false });
    await expect(addMenu).toBeVisible();
    await page.keyboard.press("Escape");
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
