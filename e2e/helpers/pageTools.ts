// SPDX-License-Identifier: AGPL-3.0-only
import { type Page } from "@playwright/test";

/** Reach a requested view through its context button at every viewport width. */
export async function openPageTool(page: Page, tool: string, actions = "Page actions"): Promise<void> {
  const hant = actions === "筆記操作";
  const hans = actions === "笔记操作";
  const infoLabel = hant ? "資訊" : hans ? "信息" : "Info";
  const mapLabels = ["Mind map", "心智圖", "思维导图"];
  const info = ["Page info", "筆記資訊", "笔记信息", infoLabel].includes(tool);
  const isMap = mapLabels.includes(tool);
  const pane = page.locator(".page-info-drawer:visible");
  if (info) {
    const tab = pane.getByRole("tab", { name: infoLabel, exact: true });
    const active = await tab.count() ? await tab.getAttribute("aria-selected") : null;
    if (active !== "true") await page.getByRole("button", { name: infoLabel, exact: true }).click();
    return;
  }
  if (isMap) {
    const tab = pane.getByRole("tab", { name: tool, exact: true });
    const active = await tab.count() ? await tab.getAttribute("aria-selected") : null;
    if (active !== "true") await page.getByRole("button", { name: tool, exact: true }).click();
    return;
  }
  // PDF and other reader tabs are available only after opening a source.
  if (!await pane.count()) await page.getByRole("button", { name: infoLabel, exact: true }).click();
  await page.getByRole("tab", { name: tool, exact: true }).click();
}

/** Open the memory dossier's Info inspector (where its knowledge context lives). */
export async function openMemoryContext(
  page: Page,
  info = "Info",
): Promise<void> {
  const toggle = page.getByRole("button", { name: info, exact: true });
  if (await toggle.getAttribute("aria-expanded") !== "true") await toggle.click();
}
