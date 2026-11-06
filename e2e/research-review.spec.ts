// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { resolve } from "node:path";
import { readFileSync } from "node:fs";

test("source status and dispute decision stay within existing surfaces", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const paper = readFileSync(resolve("crates/wenlan-core/tests/fixtures/pdf/bert_1810.04805.pdf"));
  await page.route("**/review-paper.pdf", route => route.fulfill({ contentType: "application/pdf", body: paper }));
  await page.addInitScript(() => {
    const digest = "a".repeat(64);
    const lifecycle = {state:"active", revision:0, reason:""};
    const evidence = [{ source: "file", source_id: "paper-a", source_digest: digest, source_lifecycle:lifecycle, quote: "BERT: Pre-training of Deep Bidirectional Transformers for", pdf_target: { attachment_key: "PAPER001", pdf_digest: digest, page: 1, quote: "BERT: Pre-training of Deep Bidirectional Transformers for" } }, { source: "file", source_id: "paper-b", source_digest: digest, source_lifecycle:lifecycle, quote: "Second source excerpt for comparison." }];
    let record = { space_id: "space-id", source: "file", source_id: "paper-a", revision: 0, state: "active", reason: "", actor: "", updated_at: 0 };
    let view = { space: "Research", id: "dispute-1", status: "awaiting_review", dispute: { space_id: "space-id", kind: "opposing_claims", conditions: "Different study methods", reason: "Findings disagree", evidence }, payload_digest: digest, decisions: [], source_states: evidence.map(item => ({ source: item.source, source_id: item.source_id, expected_digest: digest, current_digest: digest, expected_pdf_digest: item.pdf_target?.pdf_digest ?? null, current_pdf_digest: item.pdf_target?.pdf_digest ?? null, expected_lifecycle:lifecycle, current_lifecycle:lifecycle, state: "unchanged" })) };
    Object.assign(window, { __fixtureCalls: [], __TAURI_INTERNALS__: { invoke: async (command: string, args: any) => {
      (window as any).__fixtureCalls.push({ command, args });
      if (command === "get_source_lifecycle") return { record, history: [], impacts: [], preview_impacts: [{ page_id: "page-1", page_version: 2, source_revision: 3, source_ids: ["paper-a"], occurrences: [1], reason: "" }] };
      if (command === "get_page" && args.id === "page-1") return { id: "page-1", title: "Evidence synthesis", space: "Research" };
      if (command === "resolve_source_reference") return { space_id: "space-id", source: "file", source_id: args.locator, source_agent: args.locator === "paper-a" ? "zotero" : null, title: args.locator === "paper-a" ? "BERT paper" : "Second fixture source" };
      if (command === "get_zotero_document") return { source_id: "paper-a", title: "BERT paper", historical: true, warnings: [], annotations: [], bibtex: "", attachments: [{key:"PAPER001",title:"Historical paper",available:true,pdf_digest:digest}] };
      if (command === "read_zotero_pdf") return Array.from(new Uint8Array(await (await fetch("/review-paper.pdf")).arrayBuffer()));
      if (command === "set_source_lifecycle") { record = { ...record, ...args.request, revision: 1, actor: "human", updated_at: 1 }; return { record, history: [record], impacts: [{ page_id: "page-1", page_version: 2, source_revision: 3, source_ids: ["paper-a"], occurrences: [1], reason: record.reason }] }; }
      if (command === "get_chunks") return [{ id: "chunk-1", chunk_index: 0, chunk_type: null, language: null, content: "Retained source text." }];
      if (command === "get_research_dispute") return view;
      if (command === "decide_research_dispute") { view = { ...view, status: "resolved" }; return view; }
      if (command === "get_activity") return { state: "up_to_date", assets: [], everyday: { available: true }, synthesis: { available: true } };
      throw new Error(`Unexpected command ${command}`);
    } } });
  });
  await page.goto("/preview/research-review.html?mode=source");
  await expect(page.getByText("Retained source text.")).toBeVisible();
  await expect(page.getByText(/Marked retracted:/)).toHaveCount(0);
  await page.getByRole("button", { name: "Source options" }).click();
  await page.getByRole("button", { name: "Change source status" }).click();
  await expect(page.getByText("Evidence synthesis")).toBeVisible();
  await expect(page.getByText("page-1", { exact: true })).toHaveCount(0);
  await page.getByLabel("New status").selectOption("retracted");
  await page.getByLabel("Reason", { exact: true }).fill("Publisher retracted the paper");
  await page.getByRole("button", { name: "Save status" }).click();
  await expect(page.getByText("Affected pages: 1")).toBeVisible();
  if (process.env.RESEARCH_REVIEW_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.RESEARCH_REVIEW_EVIDENCE_DIR, "source-status.png") });
  await page.getByRole("button", { name: "Close", exact: true }).last().click();
  await expect(page.getByText("Marked retracted: Publisher retracted the paper")).toBeVisible();
  await expect(page.getByText("Retained source text.")).toBeVisible();
  await page.goto("/preview/research-review.html?mode=review");
  await expect(page.getByText("BERT: Pre-training of Deep Bidirectional Transformers for")).toBeVisible();
  await expect(page.getByText("Second source excerpt for comparison.")).toBeVisible();
  await expect(page.getByText(/BERT paper — Unchanged/)).toBeVisible();
  await expect(page.getByText(/paper-a —/)).toHaveCount(0);
  await page.getByRole("button", { name: "Open source" }).first().click();
  await expect(page.getByText("Retained source text.")).toBeAttached();
  await expect(page.getByText("Historical PDF version used by this citation.")).toBeVisible();
  await expect(page.locator(".textLayer").first()).toContainText("BERT");
  await expect(page.getByRole("spinbutton", {name:"Page", exact:true})).toHaveValue("1");
  await expect(page.locator(".wenlan-pdf-quote").first()).toBeVisible();
  if (process.env.RESEARCH_REVIEW_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.RESEARCH_REVIEW_EVIDENCE_DIR, "dispute-source.png") });
  await page.getByRole("button", { name: "Close", exact: true }).last().click();
  await page.getByLabel("Decision", { exact: true }).selectOption("not_comparable");
  await page.getByLabel("Reason for your decision").fill("The study methods differ");
  if (process.env.RESEARCH_REVIEW_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.RESEARCH_REVIEW_EVIDENCE_DIR, "dispute-review.png") });
  await page.getByRole("button", { name: "Record decision" }).click();
  await expect(page.getByText("Decision recorded")).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__fixtureCalls);
  expect(calls.find((call: any) => call.command === "get_zotero_document")?.args).toMatchObject({ sourceId: "paper-a", space: "Research", attachmentKey: "PAPER001", expectedDigest: "a".repeat(64), allowRetained: true });
  expect(calls.find((call: any) => call.command === "read_zotero_pdf")?.args).toMatchObject({ sourceId: "paper-a", attachmentKey: "PAPER001", expectedDigest: "a".repeat(64), allowRetained: true });
  expect(calls.find((call: any) => call.command === "decide_research_dispute")?.args).toMatchObject({ space: "Research", id: "dispute-1", decision: "not_comparable", reason: "The study methods differ" });
  expect(errors).toEqual([]);
});
