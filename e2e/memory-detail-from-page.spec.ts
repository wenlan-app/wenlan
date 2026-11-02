// SPDX-License-Identifier: AGPL-3.0-only
import { writeFile } from "node:fs/promises";
import { openMemoryFromPage } from "./helpers/primaryNavigation";
import { expect, test, type Page } from "@playwright/test";
import { installTauriMock } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import type { MemoryItem } from "../src/lib/tauri";

const evidenceDir = process.env.MEMORY_LIST_EVIDENCE_DIR;
test.describe.configure({ mode: "serial" });
const memoryFixtures: readonly MemoryItem[] = [
  {
    source_id: "mem-local-first",
    title: "Local-first decision",
    content: "Wenlan keeps the review workflow local-first so source context stays inspectable.",
    source_text: "The original conversation remains available when the reader needs more context.",
    summary: "Local-first review workflow",
    memory_type: "decision",
    domain: "Wenlan",
    source_agent: "claude-code",
    confidence: 0.92,
    confirmed: true,
    pinned: true,
    supersedes: null,
    last_modified: 1_700_000_000,
    chunk_count: 2,
    access_count: 7,
    is_recap: false,
    stability: "confirmed",
  },
  {
    source_id: "mem-dark-mode",
    title: "Prefers dark mode",
    content: "Lucian prefers dark mode in editors and terminals.",
    summary: null,
    memory_type: "preference",
    domain: "Work",
    source_agent: "chatgpt",
    confidence: 0.84,
    confirmed: false,
    pinned: false,
    supersedes: null,
    last_modified: 1_699_999_800,
    chunk_count: 1,
    access_count: 3,
    is_recap: false,
    stability: "new",
  },
  {
    source_id: "mem-ci-fact",
    title: "CI placeholder sidecars are compile-time only",
    content: "The desktop app uses placeholder sidecars for compile checks; daemon runtime proof is separate.",
    summary: null,
    memory_type: "fact",
    domain: "Release",
    source_agent: "claude-code",
    confidence: 0.78,
    confirmed: true,
    pinned: false,
    supersedes: null,
    last_modified: 1_699_999_500,
    chunk_count: 1,
    access_count: 2,
    is_recap: false,
    stability: "confirmed",
  },
];

async function capture(page: Page, name: string): Promise<void> {
  if (!evidenceDir) return;
  await page.screenshot({ path: `${evidenceDir}/${name}.png`, fullPage: true });
}

async function writeConsoleEvidence(pageErrors: readonly string[], consoleErrors: readonly string[]): Promise<void> {
  if (!evidenceDir) return;
  await writeFile(
    `${evidenceDir}/console.json`,
    `${JSON.stringify({ pageErrors, consoleErrors }, null, 2)}\n`,
  );
}

// Memories have no list page of their own (UX-FIND-4): a person opens one
// from the sources of a page that cites it, reads it, and goes back.
test("opens a memory from a page that cites it and returns with Escape", async ({ page }) => {
  const pageErrors: string[] = [];
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors.push(message.text());
    }
  });

  const base = createSpacesNavigationFixture();
  const fixture = {
    ...base,
    pages: base.pages.map((p) => p.id === "page-history" ? { ...p, source_memory_ids: [memoryFixtures[0].source_id] } : p),
  };
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], fixture, memories: memoryFixtures });
  await page.goto("/");

  await openMemoryFromPage(page, "History semantics", /Local-first decision/i);
  const detail = page.getByRole("main", { name: "Memory dossier" });
  await expect(detail).toBeVisible();
  await expect(detail.getByText(memoryFixtures[0].content)).toBeVisible();
  await expect(detail.getByText(memoryFixtures[0].source_text!)).not.toBeVisible();
  const info = page.getByRole("button", { name: "Info", exact: true });
  await expect(info).toHaveCount(1);
  if (await info.getAttribute("aria-expanded") !== "true") await info.click();
  const context = page.locator(".page-info-drawer:visible");
  const sourceDetails = context.locator(".memory-detail-source");
  await expect(sourceDetails).toHaveAttribute("open", "");
  await expect(context.getByText(memoryFixtures[0].source_text!)).toBeVisible();
  await sourceDetails.locator("summary").click();
  await expect(context.getByText(memoryFixtures[0].source_text!)).toBeHidden();
  await sourceDetails.locator("summary").click();
  await expect(context.getByText(memoryFixtures[0].source_text!)).toBeVisible();
  await expect(context.getByRole("button", { name: /decision/i })).toBeVisible();
  await context.getByRole("button", { name: "Close", exact: true }).click();
  await expect(info).toBeFocused();
  const bodyParagraph = detail.locator(".memory-document-editor .cm-content");
  // Short memories use reading type, not a length-dependent display headline.
  await expect(bodyParagraph).toHaveCSS("font-size", "15px");
  await capture(page, "memory-detail-clickthrough");

  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { level: 1, name: "History semantics", exact: true })).toBeVisible();
  await capture(page, "memory-detail-returned");

  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
  await writeConsoleEvidence(pageErrors, consoleErrors);
});
