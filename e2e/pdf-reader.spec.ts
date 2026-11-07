// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test } from "@playwright/test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

test("bundled PDF.js reads, searches and navigates an original paper without external services", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const remoteRequests: string[] = [];
  page.on("request", request => { if (!new URL(request.url()).hostname.match(/^(127\.0\.0\.1|localhost)$/)) remoteRequests.push(request.url()); });
  const paper = readFileSync(resolve("crates/wenlan-core/tests/fixtures/pdf/bert_1810.04805.pdf"));
  await page.route("**/pdf-fixture.pdf", route => route.fulfill({ contentType: "application/pdf", body: paper }));
  await page.goto("/preview/pdf-reader.html");
  await expect(page.getByRole("button", { name: "Next page", exact: true })).toBeEnabled();
  await expect(page.locator(".textLayer").first()).toContainText("BERT");
  await page.getByRole("button", { name: "Next page", exact: true }).click();
  await expect(page.getByRole("spinbutton", { name: "Page", exact: true })).toHaveValue("2");
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  await page.getByRole("button", { name: "Fit width", exact: true }).click();
  await page.getByRole("textbox", { name: "Find in PDF", exact: true }).fill("BERT");
  await page.getByRole("button", { name: "Find in PDF", exact: true }).click();
  await expect(page.getByRole("status")).toContainText("matches");
  await expect(page.locator(".textLayer .highlight").first()).toBeVisible();
  await page.getByRole("spinbutton", { name: "Page", exact: true }).fill("3");
  await expect(page.getByRole("spinbutton", { name: "Page", exact: true })).toHaveValue("3");
  await page.reload();
  await expect(page.getByRole("button", { name: "Next page", exact: true })).toBeEnabled();
  await expect(page.getByRole("spinbutton", { name: "Page", exact: true })).toHaveValue("3");
  expect(errors).toEqual([]);
  expect(remoteRequests).toEqual([]);
});

test("Zotero rectangle tracks the actual PDF viewport through zoom, rotation and reset", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const paper = readFileSync(resolve("crates/wenlan-core/tests/fixtures/pdf/bert_1810.04805.pdf"));
  await page.route("**/pdf-fixture.pdf", route => route.fulfill({ contentType: "application/pdf", body: paper }));
  await page.goto("/preview/pdf-reader.html?annotation=1&page=1");
  const mark = page.locator('.page[data-page-number="1"] .wenlan-pdf-annotation');
  await expect(mark).toBeVisible();
  const checkProjection = async () => {
    await expect.poll(async () => page.evaluate(() => {
      const viewer = (window as any).__pdfFixtureViewer;
      const view = viewer.getPageView(0);
      const el = view.div.querySelector(".wenlan-pdf-annotation") as HTMLElement | null;
      if (!el) return false;
      const a = view.viewport.convertToViewportPoint(70, 690);
      const b = view.viewport.convertToViewportPoint(300, 705);
      return Math.abs(parseFloat(el.style.left) - Math.min(a[0], b[0])) < .2 &&
        Math.abs(parseFloat(el.style.top) - Math.min(a[1], b[1])) < .2 &&
        Math.abs(parseFloat(el.style.width) - Math.abs(a[0] - b[0])) < .2 &&
        Math.abs(parseFloat(el.style.height) - Math.abs(a[1] - b[1])) < .2;
    })).toBe(true);
    await expect(mark).toHaveCount(1);
  };
  await checkProjection();
  if (process.env.PDF_UI_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.PDF_UI_EVIDENCE_DIR, "annotation.png") });
  await page.getByRole("button", { name: "Zoom in", exact: true }).click();
  await checkProjection();
  await page.evaluate(() => { (window as any).__pdfFixtureViewer.pagesRotation = 90; });
  await checkProjection();
  await mark.scrollIntoViewIfNeeded();
  if (process.env.PDF_UI_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.PDF_UI_EVIDENCE_DIR, "annotation-rotated.png") });
  await page.evaluate(() => { const viewer = (window as any).__pdfFixtureViewer; viewer.getPageView(0).reset(); viewer.update(); });
  await expect(mark).toBeVisible();
  await checkProjection();
  expect(errors).toEqual([]);
});

test("citation quote uses unique real text geometry and otherwise stays page-only", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", error => errors.push(error.message));
  const paper = readFileSync(resolve("crates/wenlan-core/tests/fixtures/pdf/bert_1810.04805.pdf"));
  await page.route("**/pdf-fixture.pdf", route => route.fulfill({ contentType: "application/pdf", body: paper }));
  await page.goto("/preview/pdf-reader.html?page=1");
  const textLayer = page.locator('.page[data-page-number="1"] .textLayer');
  await expect(textLayer).toContainText("BERT");
  const quote = await textLayer.evaluate(layer => {
    const text = (layer.textContent ?? "").replace(/\s+/g, " ");
    return Array.from(layer.querySelectorAll("span"))
      .map(span => (span.textContent ?? "").replace(/\s+/g, " ").trim())
      .find(value => value.length > 30 && text.indexOf(value) === text.lastIndexOf(value));
  });
  expect(quote).toBeTruthy();
  const multiline = await textLayer.evaluate(layer => {
    const all = Array.from(layer.childNodes).map(node => node.nodeName === "BR" ? " " : node.textContent ?? "").join("").replace(/\s+/g, " ");
    return Array.from(layer.querySelectorAll("br")).map(br => {
      const before = br.previousElementSibling?.textContent?.trim() ?? "";
      const after = br.nextElementSibling?.textContent?.trim() ?? "";
      return `${before} ${after}`.trim().replace(/\s+/g, " ");
    }).find(value => value.length > 20 && value.includes(" ") && all.indexOf(value) >= 0 && all.indexOf(value) === all.lastIndexOf(value));
  });
  expect(multiline).toBeTruthy();
  await page.goto(`/preview/pdf-reader.html?page=1&quote=${encodeURIComponent(multiline!)}`);
  await expect(page.locator('.page[data-page-number="1"] .wenlan-pdf-quote')).toHaveCount(2);
  if (process.env.PDF_UI_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.PDF_UI_EVIDENCE_DIR, "citation-multiline.png") });
  await page.goto(`/preview/pdf-reader.html?page=1&quote=${encodeURIComponent(quote!)}`);
  const mark = page.locator('.page[data-page-number="1"] .wenlan-pdf-quote');
  await expect(mark.first()).toBeVisible();
  const intersects = await page.evaluate(quote => {
    const layer = document.querySelector('.page[data-page-number="1"] .textLayer')!;
    const target = Array.from(layer.querySelectorAll("span")).find(span => (span.textContent ?? "").replace(/\s+/g, " ").trim() === quote);
    const mark = document.querySelector('.page[data-page-number="1"] .wenlan-pdf-quote');
    if (!target || !mark) return false;
    const a = target.getBoundingClientRect(), b = mark.getBoundingClientRect();
    return a.width > 0 && b.width > 0 && Math.min(a.right, b.right) > Math.max(a.left, b.left) && Math.min(a.bottom, b.bottom) > Math.max(a.top, b.top);
  }, quote!);
  expect(intersects).toBe(true);
  if (process.env.PDF_UI_EVIDENCE_DIR) await page.screenshot({ path: resolve(process.env.PDF_UI_EVIDENCE_DIR, "citation-quote.png") });
  const bertCount = await textLayer.evaluate(layer => ((layer.textContent ?? "").match(/BERT/g) ?? []).length);
  expect(bertCount).toBeGreaterThan(1);
  for (const absent of ["BERT", "this quote does not exist in the paper at all"]) {
    await page.goto(`/preview/pdf-reader.html?page=1&quote=${encodeURIComponent(absent)}`);
    await expect(page.locator('.page[data-page-number="1"] .textLayer')).toContainText("BERT");
    await expect(page.locator(".wenlan-pdf-quote")).toHaveCount(0);
    await expect(page.getByRole("spinbutton", { name: "Page", exact: true })).toHaveValue("1");
  }
  await page.goto("/preview/pdf-reader.html?page=9999&quote=anything");
  await expect(page.getByText("The cited PDF location is unavailable or has changed.")).toBeVisible();
  await expect(page.getByRole("spinbutton", { name: "Page", exact: true })).not.toHaveValue("9999");
  await expect(page.locator(".wenlan-pdf-quote")).toHaveCount(0);
  expect(errors).toEqual([]);
});
