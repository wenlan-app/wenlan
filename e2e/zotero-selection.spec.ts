// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { resolve } from "node:path";

test("Zotero article selection is explicit and collection scoped", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.addInitScript(() => {
    Object.assign(window, { __fixtureCalls: [], __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
      (window as any).__fixtureCalls.push({ command, args });
      if (command === "list_zotero_collections") return { server_id: "server1", collections: [{ key: "COLL0001", name: "Machine learning", item_count: 2 }, { key: "COLL0002", name: "Reading", item_count: 1 }] };
      if (command === "list_zotero_items") return { server_id: "server1", collection_key: args.collectionKey, complete: true, items: args.collectionKey === "COLL0001" ? [{ key: "PAPER001", title: "Attention Is All You Need", item_type: "conferencePaper" }, { key: "PAPER002", title: "BERT: Pre-training of Deep Bidirectional Transformers", item_type: "journalArticle" }] : [{ key: "PAPER003", title: "A different paper", item_type: "journalArticle" }] };
      if (command === "add_zotero_source") return { id: "fixture", source_type: "zotero", path: "fixture", status: "Active", last_sync: null, file_count: 0, memory_count: 0 };
      if (command === "sync_registered_source") return { files_found: 1, ingested: 1, skipped: 0, errors: 0 };
      throw new Error(`Unexpected command ${command}`);
    } } });
  });
  await page.goto("/preview/zotero-selection.html");
  await expect(page.getByText("Connected to local Zotero")).toBeVisible();
  await page.getByRole("button", { name: "Choose articles" }).click();
  const first = page.getByRole("checkbox", { name: "Attention Is All You Need" });
  await expect(first).toBeVisible();
  await expect(page.getByRole("button", { name: "Import selected articles" })).toBeDisabled();
  await first.check();
  await expect(page.getByText("1 selected")).toBeVisible();
  if (process.env.ZOTERO_SELECTION_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.ZOTERO_SELECTION_EVIDENCE_DIR, "selection.png") });
  await page.getByRole("combobox").selectOption("COLL0002");
  await expect(page.getByRole("checkbox", { name: "A different paper" })).not.toBeChecked();
  await expect(page.getByText("0 selected")).toBeVisible();
  await expect(page.getByRole("button", { name: "Import selected articles" })).toBeDisabled();
  await page.getByRole("checkbox", { name: "A different paper" }).check();
  await page.getByRole("button", { name: "Import selected articles" }).click();
  await expect(page.getByText("Imported 1; skipped 0; errors 0.")).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__fixtureCalls);
  expect(calls.find((call: any) => call.command === "add_zotero_source")?.args).toMatchObject({ collectionKey: "COLL0002", expectedServerId: "server1", space: "Research", itemKeys: ["PAPER003"] });
  expect(errors).toEqual([]);
});
