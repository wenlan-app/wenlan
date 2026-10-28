// SPDX-License-Identifier: AGPL-3.0-only
import { writeFile } from "node:fs/promises";
import { openPrimaryDestination } from "./helpers/primaryNavigation";
import { expect, test, type Page } from "@playwright/test";
import { installTauriMock } from "./tauriMock";
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

test("opens a memory from the parent list and returns with Escape", async ({ page }) => {
  const pageErrors: string[] = [];
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors.push(message.text());
    }
  });

  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], memories: memoryFixtures });
  await page.goto("/");

  await openPrimaryDestination(page, "Memories");

  const list = page.getByRole("region", { name: "Memory list" });
  await expect(list).toBeVisible();
  const row = list.getByRole("article", { name: /Local-first decision/i });
  await expect(row).toBeVisible();
  await expect(row.getByText(memoryFixtures[0].content)).toBeVisible();
  await expect(row.getByText(memoryFixtures[0].title!, { exact: true })).toHaveCount(0);
  await expect(row.getByText(memoryFixtures[0].source_text!)).toHaveCount(0);
  await expect(row.locator("dl")).toHaveCount(0);
  await expect(row.getByRole("button", { name: "Delete memory", exact: true })).toHaveCount(0);
  await row.hover();
  await row.getByRole("button", { name: "Memory actions", exact: true }).click();
  await expect(page.getByRole("menuitem", { name: "Unpin memory", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(row.getByRole("button", { name: "Memory actions", exact: true })).toBeFocused();
  await capture(page, "memory-list-desktop");

  await page.setViewportSize({ width: 768, height: 900 });
  await capture(page, "memory-list-tablet");

  await page.setViewportSize({ width: 375, height: 812 });
  await expect(page.getByTitle("Show sidebar")).toBeVisible();
  await page.getByTitle("Show sidebar").click();
  await expect(page.getByTitle("Hide sidebar")).toBeVisible();
  await expect(page.locator("aside")).toHaveAttribute("aria-hidden", "false");
  await page.getByTitle("Hide sidebar").click();
  await expect(page.getByTitle("Show sidebar")).toBeVisible();
  await expect(page.locator("aside")).toHaveAttribute("aria-hidden", "true");
  await capture(page, "memory-list-mobile");

  await row.getByRole("button", { name: "Open memory" }).click();
  await expect(page.getByRole("main", { name: "Memory dossier" })).toBeVisible();
  const detail = page.getByRole("main", { name: "Memory dossier" });
  await expect(detail.getByText(memoryFixtures[0].content)).toBeVisible();
  await expect(detail.getByText(memoryFixtures[0].source_text!)).not.toBeVisible();
  await expect(page.getByRole("button", { name: "Info", exact: true })).toHaveCount(1);
  await page.getByRole("button", { name: "Info", exact: true }).click();
  const context = page.getByRole("dialog", { name: "Info" });
  const sourceDetails = context.locator(".memory-detail-source");
  await expect(sourceDetails).toHaveAttribute("open", "");
  await expect(context.getByText(memoryFixtures[0].source_text!)).toBeVisible();
  await sourceDetails.locator("summary").click();
  await expect(context.getByText(memoryFixtures[0].source_text!)).toBeHidden();
  await sourceDetails.locator("summary").click();
  await expect(context.getByText(memoryFixtures[0].source_text!)).toBeVisible();
  await expect(context.getByRole("button", { name: /decision/i })).toBeVisible();
  await context.getByRole("button", { name: "Close", exact: true }).click();
  await expect(page.getByRole("button", { name: "Info", exact: true })).toBeFocused();
  const bodyParagraph = detail.locator(".memory-document-editor .cm-content");
  // Short memories use reading type, not a length-dependent display headline.
  await expect(bodyParagraph).toHaveCSS("font-size", "15px");
  await capture(page, "memory-detail-clickthrough");

  await page.setViewportSize({ width: 1280, height: 900 });
  await page.keyboard.press("Escape");
  await expect(page.getByRole("region", { name: "Memory list" })).toBeVisible();
  await capture(page, "memory-list-returned");

  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
  await writeConsoleEvidence(pageErrors, consoleErrors);
});

test("renders the empty parent memory list", async ({ page }) => {
  const pageErrors: string[] = [];
  const consoleErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") {
      consoleErrors.push(message.text());
    }
  });

  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [], memories: [] });
  await page.goto("/");
  await openPrimaryDestination(page, "Memories");

  await expect(page.getByRole("region", { name: "Memory list" })).toBeVisible();
  await expect(page.getByText(/^Memories are small things/)).toBeVisible();
  await capture(page, "memory-list-empty");

  expect(pageErrors).toEqual([]);
  expect(consoleErrors).toEqual([]);
  await writeConsoleEvidence(pageErrors, consoleErrors);
});


test("filters Memories in the shared toolbar without duplicate navigation and retains the query on return", async ({page}, info) => {
  await page.setViewportSize({width:1280,height:840});
  const runtime=await installTauriMock(page,{locale:"en",rawActions:[],memories:memoryFixtures});
  await page.goto("/");await openPrimaryDestination(page,"Memories");
  const list=page.getByRole("region",{name:"Memory list",exact:true});
  const filter=list.getByRole("searchbox",{name:"Filter memories",exact:true});
  await expect(filter).toBeVisible();
  await expect(page.getByRole("button",{name:"Recaps",exact:true})).toHaveCount(0);
  await expect(page.getByRole("button",{name:"Recent memories",exact:true})).toHaveCount(0);
  await filter.fill("  DARK MODE  ");await expect(list.getByRole("article")).toHaveCount(1);
  await expect(list.getByRole("article",{name:"Prefers dark mode"})).toBeVisible();
  await page.getByTestId("asset-lens-cards").click();await expect(list.locator(".memory-collection-card")).toHaveCount(1);
  await filter.fill("compile checks");await expect(list.getByRole("article")).toHaveCount(1);
  await expect(list.getByRole("article",{name:"CI placeholder sidecars are compile-time only"})).toBeVisible();
  await page.getByTestId("asset-lens-rows").click();
  expect(runtime.calls().filter(c=>["search","search_memories"].includes(c.command))).toHaveLength(0);
  await list.getByRole("button",{name:"Open memory",exact:true}).click();
  await expect(page.getByRole("main",{name:"Memory dossier"})).toBeVisible();
  await expect(page.getByRole("button",{name:"Recent memories",exact:true})).toBeVisible();
  await page.keyboard.press("Escape");await expect(filter).toHaveValue("compile checks");await expect(list.getByRole("article")).toHaveCount(1);
  await filter.fill("no-matching-memory");await expect(list.getByRole("article")).toHaveCount(0);await expect(list.getByText("No memories match this view",{exact:true})).toBeVisible();
  await list.getByRole("button",{name:"Clear filter",exact:true}).click();await expect(filter).toHaveValue("");await expect(list.getByRole("article")).toHaveCount(3);
  await page.screenshot({path:info.outputPath("memories-filter.png")});
  expect(runtime.calls().filter(c=>["search","search_memories"].includes(c.command) && ["  DARK MODE  ","compile checks","no-matching-memory"].includes(String((c.args as Record<string, unknown>)?.query)))).toHaveLength(0);
});
