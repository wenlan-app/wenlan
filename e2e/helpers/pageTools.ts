// SPDX-License-Identifier: AGPL-3.0-only
import { type Page } from "@playwright/test";

/** Reach page views via the dedicated inspector, at every viewport width. */
export async function openPageTool(page: Page, tool: string, actions = "Page actions"): Promise<void> {
  const hant = actions === "筆記操作";
  const hans = actions === "笔记操作";
  const open = hant ? "開啟筆記側欄" : hans ? "打开笔记侧栏" : "Open page sidebar";
  const toggle = page.getByRole("button", {name: open, exact: true});
  if (await toggle.count()) await toggle.click();
  const info = ["Page info", "筆記資訊", "笔记信息", "資訊", "信息"].includes(tool);
  await page.getByRole("tab", {name: info ? (hant ? "資訊" : hans ? "信息" : "Info") : tool, exact: true}).click();
}

/** Open the memory dossier's Info inspector (where its knowledge context lives). */
export async function openMemoryContext(
  page: Page,
  info = "Info",
): Promise<void> {
  const toggle = page.getByRole("button", { name: info, exact: true });
  if (await toggle.getAttribute("aria-expanded") !== "true") await toggle.click();
}
