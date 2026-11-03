// SPDX-License-Identifier: AGPL-3.0-only
import { test, expect } from "@playwright/test";
import { installTauriMock, collectBrowserErrors } from "./tauriMock";
import { createSpacesNavigationFixture } from "./fixtures/spacesNavigation";
import { resources } from "../src/i18n/resources";
import type { ThingSummary } from "../src/lib/tauri";

// UX-HOME-5: a thing quiet for a week leaves "what you're working on";
// there is no cap, "See all" opens the rest in place without a count, and
// "Don't show" takes a finished thing off home at once.
const day = 86_400;

function things(now: number): ThingSummary[] {
  const current = [
    ["New tagline", "Picked the X wording; the site still shows the old line.", 0.1],
    ["Storage step 4", "Restore at start is in; a real-copy test is next.", 0.3],
    ["Home redesign", "Cards for current work; the scrollbar shows on hover.", 1],
    ["Source alerts", "Activity names a folder it cannot reach.", 1.5],
    ["Local model list", "Grouped by source, one recommended by memory.", 2],
    ["Product Hunt follow-up", "Thank-you notes drafted, waiting on wording.", 4],
    ["Editor headings", "One line above, half a line below.", 6],
    ["Search groups", "Topics last; open in the original tool is not done.", 5],
    ["Offline mode", "Shipped in 0.20.7; tell the reporter.", 6.5],
  ] as const;
  const quiet = [
    ["Trip planning (old ChatGPT chat)", 40],
    ["Pricing page rewrite", 31],
    ["Summer reading list", 120],
  ] as const;
  return [
    ...current.map(([title, status_line, ago], index) => ({
      id: `thg_c${index}`, title, title_by: "ai" as const, status_line, spaces: ["wenlan"],
      last_activity_at: Math.floor(now - ago * day), quiet: false,
    })),
    ...quiet.map(([title, ago], index) => ({
      id: `thg_q${index}`, title, title_by: "ai" as const, status_line: null, spaces: [],
      last_activity_at: Math.floor(now - ago * day), quiet: true,
    })),
  ];
}

for (const cfg of [
  { locale: "zh-Hans", theme: "light", width: 1280 },
  { locale: "en", theme: "dark", width: 1440 },
  { locale: "zh-Hant", theme: "light", width: 375 },
] as const) {
  test(`UX-HOME-5 home leaves quiet things out, opens the rest in place and hides on request ${cfg.locale} ${cfg.width}`, async ({ page }, info) => {
    const t = resources[cfg.locale].translation;
    const errors = collectBrowserErrors(page);
    await installTauriMock(page, { fixture: createSpacesNavigationFixture(), locale: cfg.locale, rawActions: [], localStorage: {
      "wenlan-theme": cfg.theme,
      "onboarding:gettingStartedDismissed": "1",
    } });
    await page.setViewportSize({ width: cfg.width, height: 900 });
    let list = things(Date.now() / 1000);
    const hidden: string[] = [];
    await page.exposeBinding("__thingsInvoke", (_source, command: string, args: { id?: string } | undefined) => {
      if (command === "list_things") return list;
      if (command === "hide_thing") {
        hidden.push(args?.id ?? "");
        list = list.map((thing) => (thing.id === args?.id ? { ...thing, quiet: true } : thing));
        return list.find((thing) => thing.id === args?.id);
      }
      if (command === "get_thing") {
        const found = list.find((thing) => thing.id === args?.id);
        if (!found) throw new Error("unknown thing");
        return { ...found, sessions: [], pages: [], memories: [] };
      }
      throw new Error(`Unexpected command: ${command}`);
    });
    await page.addInitScript(() => {
      const bridge = (window as any).__TAURI_INTERNALS__, original = bridge.invoke;
      bridge.invoke = (command: string, args: unknown) => ["list_things", "get_thing", "hide_thing"].includes(command)
        ? (window as any).__thingsInvoke(command, args) : original(command, args);
    });
    await page.goto("/");

    const section = page.getByTestId("wiki-working-on");
    await expect(section.locator(".wiki-working-on-card")).toHaveCount(6);
    await expect(section).toContainText("New tagline");
    for (const old of ["Trip planning", "Pricing page rewrite", "Summer reading list"]) {
      await expect(section).not.toContainText(old);
    }
    const all = section.getByRole("button", { name: t.pages.overview.workingOnAll, exact: true });
    await expect(all).toHaveAttribute("aria-expanded", "false");
    await expect(section).not.toContainText(/\d+ (more|件|個)/);
    await page.evaluate(() => document.fonts.ready);
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath("working-on-six.png") });

    await all.click();
    await expect(section.locator(".wiki-working-on-card")).toHaveCount(9);
    await expect(section).toHaveAttribute("data-expanded", "true");
    await expect(section).toContainText("Offline mode");
    await expect(section).not.toContainText("Summer reading list");
    expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
    await page.screenshot({ path: info.outputPath("working-on-all.png"), fullPage: true });

    await section.getByRole("button", { name: t.pages.overview.workingOnFewer, exact: true }).click();
    await expect(section.locator(".wiki-working-on-card")).toHaveCount(6);

    // "Don't show" sits on the card, opens nothing, and takes it off home.
    const first = section.locator("li").first();
    await first.hover();
    const hide = first.locator(".wiki-working-on-hide");
    await expect(hide).toBeVisible();
    await expect(hide).toHaveAttribute("title", t.pages.overview.workingOnHideHint);
    await page.screenshot({ path: info.outputPath("working-on-hide.png") });
    await hide.click();
    await expect(section).not.toContainText("New tagline");
    await expect(section.locator(".wiki-working-on-card")).toHaveCount(6);
    expect(hidden).toEqual(["thg_c0"]);
    expect(errors.pageErrors).toEqual([]);
  });
}
