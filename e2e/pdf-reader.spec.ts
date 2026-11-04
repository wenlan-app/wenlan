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
