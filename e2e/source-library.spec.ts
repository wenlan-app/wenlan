// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect, type Page } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import type { IndexedFileInfo } from "../src/lib/tauri";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

async function capture(page: Page, filename: string) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations()
      .filter(animation => Number.isFinite(animation.effect?.getComputedTiming().endTime))
      .map(animation => animation.finished.catch(() => undefined)));
  });
  await page.screenshot({ path: test.info().outputPath(filename) });
}

const examples: IndexedFileInfo[] = [
  { source: "file", source_id: "guide.pdf", title: "閱讀與研究指南.pdf", chunk_count: 1, last_modified: 1 },
  { source: "memory", source_agent: "folder", source_id: "notes::/fixture/notes/reading.md", title: "閱讀筆記.md", chunk_count: 1, last_modified: 1 },
  { source: "webpage", source_id: "https://example.org/article", url: "https://example.org/article", title: "知識如何累積", chunk_count: 1, last_modified: 1 },
  { source: "memory", source_agent: "codex", source_id: "agent-preference", title: "PRIVATE AGENT PREFERENCE", chunk_count: 1, last_modified: 1 },
];
async function setup(page: Page, empty = false, dark = false) {
  const errors = collectBrowserErrors(page);
  const fixture = createSpacesNavigationFixture();
  const history = fixture.pages.find(item => item.id === "page-history")!;
  history.content = "# History semantics\n\n" + Array.from({ length: 70 }, (_, i) => `Paragraph ${i}: A longer page for returning to the same reading position.\n\n`).join("");
  await installTauriMock(page, { fixture, locale: "zh-Hant", rawActions: [], localStorage: { "wenlan-theme": dark ? "dark" : "light" } });
  let files = empty ? [] : [...examples];
  const documents = new Map(examples.map(f => [f.source_id, `這是 ${f.title} 的已保存內容。`]));
  documents.set("guide.pdf", `${documents.get("guide.pdf")}\n\n![Reference diagram](https://tracker.invalid/pixel)`);
  const writes: unknown[] = [];
  const opens: unknown[] = [];
  const annotations = new Map<string, { notes: string; page_ids: string[]; version: number }>();
  const nativeDialogCalls: unknown[] = [];
  let unavailable = false;
  let concurrentCreate = false;
  await page.exposeBinding("__sourceFixtureInvoke", async (_source, command: string, args: Record<string, unknown> = {}) => {
    if (command === "list_registered_sources") return [];
    if (command === "list_indexed_files") {
      if (unavailable) throw new Error("Fixture unavailable");
      return files;
    }
    if (command === "get_chunks") return [{ id: "fixture-chunk", content: documents.get(String(args.sourceId)) ?? "", chunk_index: 0, chunk_type: null, language: null }];
    if (command === "get_source_annotation") {
      const sourceId = String(args.sourceId);
      const annotation = annotations.get(sourceId) ?? { notes: "", page_ids: [], version: 0 };
      return { source: "webpage", source_id: sourceId, ...annotation, updated_at: 1, pages: [] };
    }
    if (command === "update_source_annotation") {
      const req = args.req as { source_id: string; notes?: string; page_ids?: string[]; expected_version: number };
      const previous = annotations.get(req.source_id) ?? { notes: "", page_ids: [], version: 0 };
      const next = { notes: req.notes ?? previous.notes, page_ids: req.page_ids ?? previous.page_ids, version: req.expected_version + 1 };
      annotations.set(req.source_id, next);
      return { source: "webpage", source_id: req.source_id, ...next, updated_at: 2, pages: [] };
    }
    if (command === "list_pages") return [];
    if (command === "plugin:dialog|open") { nativeDialogCalls.push(args); return null; }
    if (command === "ingest_webpage") {
      const req = args.req as { url: string; title: string; content: string; create_only?: boolean };
      if (req.create_only && concurrentCreate) {
        concurrentCreate = false;
        files.push({ source: "webpage", source_id: req.url, url: req.url, title: "另一個工具剛保存的摘錄", chunk_count: 1, last_modified: 3 });
        documents.set(req.url, "另一個工具剛保存的內容。");
      }
      if (req.create_only && files.some(file => file.source === "webpage" && file.source_id === req.url)) {
        throw new Error("WEBPAGE_ALREADY_EXISTS");
      }
      writes.push(req);
      files = [...files.filter(file => !(file.source === "webpage" && file.source_id === req.url)), { source: "webpage", source_id: req.url, url: req.url, title: req.title, chunk_count: 1, last_modified: 2 }];
      documents.set(req.url, req.content);
      return { chunks_created: 1, document_id: req.url };
    }
    if (command === "open_search_result") { opens.push(args); return null; }
    throw new Error(`Unexpected source fixture command: ${command}`);
  });
  await page.goto("/");
  await page.evaluate(() => {
    const bridge = window.__TAURI_INTERNALS__!;
    const original = bridge.invoke;
    bridge.invoke = (command, args) => {
      if (["list_registered_sources", "list_indexed_files", "get_chunks", "get_source_annotation", "update_source_annotation", "list_pages", "ingest_webpage", "open_search_result", "plugin:dialog|open"].includes(command)) {
        return (window as unknown as { __sourceFixtureInvoke: (name: string, args?: unknown) => Promise<unknown> }).__sourceFixtureInvoke(command, args) as ReturnType<typeof original>;
      }
      return original(command, args);
    };
  });
  await openPrimaryDestination(page, "來源", "更多");
  await expect(page.getByRole("heading", { name: "來源", exact: true })).toBeVisible();
  return { errors, writes, opens, nativeDialogCalls, fail: () => { unavailable = true; }, raceNextCreate: () => { concurrentCreate = true; }, savedText: (url: string) => documents.get(url) };
}

test("imported documents remain browsable without any registered folder", async ({ page }) => {
  const remoteImages: string[] = [];
  await page.route("https://tracker.invalid/**", route => { remoteImages.push(route.request().url()); return route.abort(); });
  const state = await setup(page);
  const library = page.getByRole("list", { name: "來源庫" });
  await expect(library.getByRole("button")).toHaveCount(3);
  await expect(page.getByText("PRIVATE AGENT PREFERENCE")).toHaveCount(0);
  await capture(page, "sources-after-library-light.png");
  await library.getByRole("button", { name: /閱讀與研究指南/ }).click();
  const preview = page.locator(".source-document-page");
  await expect(preview.getByRole("heading", { name: "閱讀與研究指南.pdf", exact: true })).toBeVisible();
  await expect(preview.locator(".source-document-body")).toContainText("這是 閱讀與研究指南.pdf 的已保存內容。");
  await expect(preview.locator("img")).toHaveCount(0);
  expect(remoteImages).toEqual([]);
  await page.getByRole("button", { name: "來源", exact: true }).click();
  await expect(preview).toHaveCount(0);
  await expect(library.getByRole("button", { name: /閱讀與研究指南/ })).toBeFocused();
  await page.getByRole("button", { name: "連結", exact: true }).click();
  await expect(library.getByRole("button")).toHaveCount(1);
  await library.getByRole("button", { name: /知識如何累積/ }).click();
  const webpage = page.locator(".source-document-page");
  await expect(webpage.getByRole("heading", { name: "知識如何累積", exact: true })).toBeVisible();
  await expect(webpage.locator(".source-document-reader-url")).toContainText("https://example.org/article");
  await expect(webpage.getByRole("alert")).toHaveCount(0);
  await page.getByRole("link", { name: "開啟原始網頁", exact: true }).click();
  expect(state.opens).toEqual([{ url: "https://example.org/article" }]);
  await page.getByRole("button", { name: "來源", exact: true }).click();
  await page.getByRole("searchbox", { name: "篩選來源", exact: true }).fill("找不到的項目");
  await expect(page.getByRole("heading", { name: "沒有符合的來源" })).toBeVisible();
  await page.getByRole("button", { name: "清除搜尋與篩選" }).click();
  await expect(library.getByRole("button")).toHaveCount(3);
  expect(state.errors.pageErrors).toEqual([]); expect(state.errors.consoleErrors).toEqual([]);
});

test("empty sources retains its library shell and offers clear localized add choices", async ({ page }) => {
  const state = await setup(page, true);
  await expect(page.getByRole("heading", { name: "把你的來源放在一起" })).toBeVisible();
  await expect(page.getByRole("searchbox", { name: "篩選來源" })).toBeVisible();
  await capture(page, "sources-after-empty-light.png");
  await page.getByRole("button", { name: "新增", exact: true }).first().click();
  const menu = page.getByRole("dialog", { name: "加入來源", exact: true });
  await expect(menu.getByRole("button", { name: /Connect Obsidian vault|連接 Obsidian/ })).toBeVisible();
  await expect(menu.getByRole("button", { name: /Add files|加入檔案/ })).toBeVisible();
  await expect(menu.getByRole("button", { name: /加入連結與文字/ })).toBeVisible();
  await capture(page, "sources-after-add-light.png");
  await menu.getByRole("button", { name: /Connect Obsidian vault|連接 Obsidian/ }).click();
  await expect.poll(() => state.nativeDialogCalls.length).toBe(1);
  await expect(menu).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toHaveCount(0);
  expect(state.errors.pageErrors).toEqual([]); expect(state.errors.consoleErrors).toEqual([]);
});

test("a web excerpt can be saved then reopened at 375px in dark mode", async ({ page }) => {
  const state = await setup(page, false, true);
  await page.setViewportSize({ width: 375, height: 812 });
  await capture(page, "sources-after-library-dark-narrow.png");
  await page.getByRole("button", { name: "新增", exact: true }).click();
  await page.getByRole("button", { name: /加入連結與文字/ }).click();
  const form = page.getByRole("dialog", { name: "加入連結與文字", exact: true });
  await form.getByLabel("連結", { exact: true }).fill("https://example.org/book?chapter=2");
  await form.getByLabel("標題（選填）", { exact: true }).fill("書中的一段話");
  await form.getByLabel("要保留的文字", { exact: true }).fill("# 摘錄\n\n知識需要被反覆閱讀，而不只是收藏。");
  await capture(page, "sources-after-excerpt-dark-narrow.png");
  await form.getByRole("button", { name: "保存連結", exact: true }).click();
  await expect(form).toHaveCount(0);
  await page.getByRole("list", { name: "來源庫" }).getByRole("button", { name: /書中的一段話/ }).click();
  const saved = page.locator(".source-document-page");
  await expect(saved.getByRole("heading", { name: "書中的一段話", exact: true })).toBeVisible();
  await expect(saved.locator(".source-document-reader-url")).toContainText("https://example.org/book?chapter=2");
  await expect(saved.getByRole("textbox", { name: "來源描述", exact: true })).toBeVisible();
  await expect(saved.getByRole("alert")).toHaveCount(0);
  expect(state.savedText("https://example.org/book?chapter=2")).toBe("# 摘錄\n\n知識需要被反覆閱讀，而不只是收藏。");
  expect(state.writes).toEqual([{ url: "https://example.org/book?chapter=2", title: "書中的一段話", content: "# 摘錄\n\n知識需要被反覆閱讀，而不只是收藏。", create_only: true }]);
  await capture(page, "sources-after-preview-dark-narrow.png");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  expect(state.errors.pageErrors).toEqual([]); expect(state.errors.consoleErrors).toEqual([]);
});

test("another excerpt from the same URL requires explicit replacement", async ({ page }) => {
  const state = await setup(page);
  await page.getByRole("button", { name: "新增", exact: true }).click();
  await page.getByRole("button", { name: /加入連結與文字/ }).click();
  const dialog = page.getByRole("dialog", { name: "加入連結與文字", exact: true });
  await dialog.getByLabel("連結", { exact: true }).fill("https://example.org/article");
  await dialog.getByLabel("標題（選填）", { exact: true }).fill("新的摘錄");
  await dialog.getByLabel("要保留的文字", { exact: true }).fill("保留新的這一段。");
  await dialog.getByRole("button", { name: "保存連結", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("取代後會移除先前的文字");
  expect(state.writes).toEqual([]);
  await dialog.getByRole("button", { name: "取代已保存的文字", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(state.writes).toHaveLength(1);
  await page.getByRole("list", { name: "來源庫" }).getByRole("button", { name: /新的摘錄/ }).click();
  await expect(page.locator(".source-document-page").getByRole("heading", { name: "新的摘錄", exact: true })).toBeVisible();
  expect(state.savedText("https://example.org/article")).toBe("保留新的這一段。");
});

test("a concurrent first save preserves the winning excerpt until explicit replacement", async ({ page }) => {
  const state = await setup(page);
  await page.getByRole("button", { name: "新增", exact: true }).click();
  await page.getByRole("button", { name: /加入連結與文字/ }).click();
  const dialog = page.getByRole("dialog", { name: "加入連結與文字", exact: true });
  const url = "https://example.org/concurrent";
  await dialog.getByLabel("連結", { exact: true }).fill(url);
  await dialog.getByLabel("標題（選填）", { exact: true }).fill("我的待保存摘錄");
  await dialog.getByRole("textbox", { name: "要保留的文字", exact: true }).fill("這段草稿必須保留。");
  state.raceNextCreate();
  await dialog.getByRole("button", { name: "保存連結", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("取代後會移除先前的文字");
  await expect(dialog.getByRole("textbox", { name: "要保留的文字", exact: true })).toHaveValue("這段草稿必須保留。");
  expect(state.writes).toEqual([]);
  expect(state.savedText(url)).toBe("另一個工具剛保存的內容。");
  await capture(page, "sources-concurrent-replace-confirmation.png");
  await dialog.getByRole("button", { name: "取代已保存的文字", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(state.writes).toEqual([{ url, title: "我的待保存摘錄", content: "這段草稿必須保留。", create_only: false }]);
  expect(state.savedText(url)).toBe("這段草稿必須保留。");
  expect(state.errors.pageErrors).toEqual([]);
  expect(state.errors.consoleErrors).toEqual([]);
});

test("navigation destinations keep one primary workspace and restore page context", async ({ page }) => {
  const state = await setup(page);
  const sidebar = page.locator(".notes-workspace-sidebar");
  await expect(sidebar).toHaveCSS("width", "240px");
  await expect(page.getByRole("navigation", { name: "主要導覽" }).locator('[aria-current="page"]')).toHaveCount(1);
  await expect(page.getByRole("button", { name: "來源", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.locator("#workspace-context-browser")).toHaveCount(0);
  await page.getByRole("button", { name: "連結", exact: true }).click();
  await page.getByRole("searchbox", { name: "篩選來源", exact: true }).fill("知識");
  await capture(page, "navigation-sources-light.png");

  await page.getByRole("button", { name: "Wiki", exact: true }).click();
  await openWikiNote(page, "History semantics");
  const editor = page.locator(".cm-content[contenteditable=true]");
  await expect(editor).toBeVisible();
  await editor.evaluate(element => {
    const view = (element as any).cmTile.root.view;
    view.dispatch({ selection: { anchor: 300, head: 315 } });
  });
  const main = page.locator(".wiki-workspace-content");
  await expect(main).toHaveCSS("overflow-y", "auto");
  await main.evaluate(element => { element.scrollTop = 600; });
  await expect.poll(() => main.evaluate(element => element.scrollTop)).toBe(600);
  await openPrimaryDestination(page, "來源", "更多");
  await expect(page.getByRole("searchbox", { name: "篩選來源", exact: true })).toHaveValue("知識");
  await expect(page.getByRole("button", { name: "連結", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(sidebar).toHaveCSS("width", "240px");

  await page.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(editor).toBeVisible();
  await expect.poll(() => editor.evaluate(element => {
    const selection = (element as any).cmTile.root.view.state.selection.main;
    return { anchor: selection.anchor, head: selection.head };
  })).toEqual({ anchor: 300, head: 315 });
  await expect.poll(() => main.evaluate(element => element.scrollTop)).toBe(600);
  await capture(page, "navigation-note-restored-light.png");

  await page.getByRole("button", { name: "空間", exact: true }).click();
  await expect(page.locator("#workspace-context-browser")).toHaveCount(0);
  await expect(sidebar).toHaveCSS("width", "240px");
  await capture(page, "navigation-spaces-light.png");
  await openPrimaryDestination(page, "來源", "更多");
  await expect(page.getByRole("searchbox", { name: "篩選來源", exact: true })).toHaveValue("知識");
  // The global toggle hides navigation, while page context remains independent.
  await page.locator("[data-sidebar-toggle]").click();
  await expect(sidebar).toHaveCSS("width", "0px");
  await page.locator("[data-sidebar-toggle]").click();
  await page.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(editor).toBeVisible();
  expect(state.errors.pageErrors).toEqual([]); expect(state.errors.consoleErrors).toEqual([]);
});


test("Sources stays pinned while occasional people and topics stay in More on a narrow dark workspace", async ({ page }) => {
  const state = await setup(page, false, true);
  await page.setViewportSize({ width: 375, height: 812 });
  await page.locator("[data-sidebar-toggle]").click();
  const nav = page.getByRole("navigation", { name: "主要導覽", exact: true });
  await expect(nav.getByRole("button", { name: "來源", exact: true })).toBeVisible();
  await expect(nav.getByRole("button", { name: "圖譜", exact: true })).toBeVisible();
  await expect(nav.getByRole("button", { name: "空間", exact: true })).toBeVisible();
  await expect(nav.getByRole("button", { name: "主題", exact: true })).toHaveCount(0);
  await nav.getByRole("button", { name: "更多", exact: true }).click();
  await expect(nav.getByRole("button", { name: "主題", exact: true })).toBeVisible();
  await capture(page, "navigation-more-dark-narrow.png");
  await nav.getByRole("button", { name: "主題", exact: true }).click();
  await expect(page.getByRole("heading", { name: "主題", exact: true })).toHaveClass(/sr-only/);
  const topicsToolbar = page.locator(".entities-toolbar");
  await expect(topicsToolbar).toBeVisible();
  await expect(topicsToolbar.getByRole("searchbox", { name: "篩選主題", exact: true })).toBeVisible();
  await expect(page.getByText("Ada Lovelace", { exact: true })).toBeVisible();
  await capture(page, "people-topics-dark-narrow.png");
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1280, height: 800 });
  await expect(page.locator(".notes-workspace-sidebar")).toHaveCSS("width", "240px");
  await expect(nav.getByRole("button", { name: "Wiki", exact: true })).toBeVisible();
  await capture(page, "people-topics-dark.png");
  expect(state.errors.pageErrors).toEqual([]); expect(state.errors.consoleErrors).toEqual([]);
});


test("the Pages entry reopens a newly saved draft after browsing sources", async ({ page }) => {
  const state = await setup(page);
  await page.getByRole("button", { name: "Wiki", exact: true }).click();
  await page.locator(".note-tab-create").click();
  await page.locator(".page-draft-title").fill("A page to return to");
  await page.locator(".page-draft-content").fill("Keep this draft when I look at sources.");
  // Leave before the debounce: navigation must flush and remember the assigned id.
  await openPrimaryDestination(page, "來源", "更多");
  await expect(page.getByRole("heading", { name: "來源", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Wiki", exact: true }).click();
  await expect(page.getByRole("heading", { name: "A page to return to", exact: true })).toBeVisible();
  await expect(page.locator(".cm-content[contenteditable=true]")).toContainText("Keep this draft when I look at sources.");
  expect(state.errors.pageErrors).toEqual([]); expect(state.errors.consoleErrors).toEqual([]);
});
