// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { openWikiNote } from "./helpers/wikiWorkspace";

test("draft inspector toggles without leaving the draft or losing autosaved content", async ({ page }, info) => {
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await openPrimaryDestination(page, "Wiki");
  await page.locator(".note-tab-create").click();

  const title = page.getByRole("textbox", { name: "Title", exact: true });
  const content = page.getByRole("textbox", { name: "Content", exact: true });
  await expect(title).toBeFocused();
  await title.fill("Regression draft");
  await content.fill("Draft body survives opening and closing Info.");
  await expect.poll(() => runtime.calls().find((call) => call.command === "create_page_draft")?.args)
    .toMatchObject({ title: "Regression draft", content: "Draft body survives opening and closing Info." });

  await page.getByRole("button", { name: "Open page sidebar", exact: true }).click();
  const infoDialog = page.getByRole("complementary", { name: "Info" });
  await expect(infoDialog.getByText("Regression draft", { exact: true })).toBeVisible();
  await expect(infoDialog.getByRole("status")).toHaveText("Saved");
  await page.screenshot({ path: info.outputPath("draft-info-sidebar.png") });
  await page.keyboard.press("Escape");
  await expect(infoDialog).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Open page sidebar", exact: true })).toBeFocused();
  await expect(title).toHaveValue("Regression draft");
  await expect(content).toHaveValue("Draft body survives opening and closing Info.");
  expect(runtime.calls().filter((call) => call.command === "publish_page_draft")).toHaveLength(0);

  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

test("page title directly opens the existing guarded rename form", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const runtime = await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await openPrimaryDestination(page, "Wiki");
  await openWikiNote(page, "Fixture architecture");

  const editor = page.getByRole("textbox", { name: "Page editor", exact: true });
  await expect(editor).toBeVisible();
  await editor.click();
  await editor.press("ControlOrMeta+End");
  await page.keyboard.insertText("\nBody survives direct title rename.");

  await page.getByRole("button", { name: "Rename page", exact: true }).click();
  const title = page.getByRole("textbox", { name: "Page title", exact: true });
  await expect(title).toBeFocused();
  await title.fill("Directly renamed fixture");
  await page.getByRole("button", { name: "Save title", exact: true }).click();

  await expect(page.getByRole("heading", { level: 1, name: "Directly renamed fixture" })).toBeVisible();
  await expect.poll(() => runtime.calls().some((call) => call.command === "rename_page"))
    .toBe(true);
  const saved = await page.evaluate(async () =>
    window.__TAURI_INTERNALS__!.invoke("get_page", { id: "page-architecture" }),
  ) as { title: string; content: string };
  expect(saved.title).toBe("Directly renamed fixture");
  expect(saved.content).toContain("Body survives direct title rename.");
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});
