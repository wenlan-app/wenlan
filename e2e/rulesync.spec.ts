// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Page } from "@playwright/test";
import type { RulesSetupState } from "../preview/rulesSetupFixture";
import { openSection } from "./helpers/wikiWorkspace";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";

test.use({ screenshot: "off", trace: "off" });

async function openSetup(page: Page, rulesSetup: RulesSetupState) {
  await installTauriMock(page, { locale: "en", rawActions: [], rulesSetup });
  await page.goto("/");
  const directory = await openSection(page, "rules");
  return directory.getByRole("region", { name: "Rules and skills setup" });
}

test("UX-SKILL-25 Bring in shows settings in the same confirmation, then Status and Restore", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const setup = await openSetup(page, "adopt");
  await expect(setup.getByRole("button", { name: "Status" })).toHaveAttribute("aria-pressed", "true");
  await expect(setup.getByText("Not managed").first()).toBeVisible();
  await setup.getByRole("button", { name: "Bring in" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog.getByText("Wenlan imports ~/notes/team-rules.md once and leaves the file in place.")).toBeVisible();
  await expect(dialog.getByText("MCP servers, hooks and permissions")).toBeVisible();
  await expect(dialog.getByText("+ [mcp_servers.calendar]")).toBeVisible();
  await expect(dialog).not.toContainText("config.toml");
  await dialog.getByRole("button", { name: "Bring in" }).click();
  // Brought in: Restore appears. OpenCode is still not managed, so its row
  // says so and Bring in stays offered for it.
  await expect(setup.getByRole("button", { name: "Restore" })).toBeVisible();
  await expect(setup.locator(".rules-setup-plan-list").getByRole("listitem").filter({ hasText: "OpenCode" }))
    .toContainText("Not managed · Use Bring in to manage this tool");
  await expect(setup.locator(".rules-setup-plan-list").getByRole("listitem").filter({ hasText: "Claude Code" })).toHaveCount(0);
  await expect(setup.getByRole("button", { name: "Bring in" })).toBeVisible();
  await expect(setup.getByRole("button", { name: "Sync now" })).toBeVisible();

  await setup.getByRole("button", { name: "Restore" }).click();
  await expect(dialog.getByText("Edits you made since then stay where they are for review.")).toBeVisible();
  await dialog.getByRole("button", { name: "Restore" }).click();
  await expect(setup.getByRole("button", { name: "Bring in" })).toBeVisible();
  expect(errors.pageErrors).toEqual([]);
});

test("UX-SKILL-25 a review chip opens every kind in the shared queue", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  const setup = await openSetup(page, "merged");
  await expect(setup.getByRole("button", { name: /To review\s*5/ })).toBeVisible();
  await expect(setup.getByText("Needs review · Open To review", { exact: true })).toBeVisible();
  await setup.getByRole("button", { name: "Cursor needs review" }).click();
  await expect(setup.getByRole("article")).toHaveCount(5);
  await expect(setup.getByRole("heading", { name: "Skill proposal for summarize" })).toBeVisible();
  await expect(setup.getByRole("heading", { name: "New version of release-notes" })).toBeVisible();
  await expect(setup.getByRole("heading", { name: "MCP servers, hooks and permissions" })).toBeVisible();
  await expect(setup.locator('[data-change="removed"]').first()).toBeVisible();
  await expect(setup.locator('[data-change="added"]').first()).toBeVisible();
  await expect(setup).not.toContainText(/AGENTS\.md|SKILL\.md|config\.toml/);

  await setup.getByRole("button", { name: "No", exact: true }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog.getByRole("button", { name: "Restore" })).toBeVisible();
  await dialog.getByRole("button", { name: "Restore" }).click();
  await expect(setup.getByRole("article")).toHaveCount(4);
  expect(errors.pageErrors).toEqual([]);
});

test("UX-SKILL-19 setup stays hidden with the feature off", async ({ page }) => {
  const setup = await openSetup(page, "off");
  await expect(setup).toHaveCount(0);
});
