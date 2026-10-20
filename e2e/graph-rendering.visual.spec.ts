// SPDX-License-Identifier: AGPL-3.0-only
import { expect, test, type Locator } from "@playwright/test";
import { collectBrowserErrors, installTauriMock } from "./tauriMock";
import { openPrimaryDestination } from "./helpers/primaryNavigation";

type CanvasEvidence = {
  coloredPixels: number;
  orangeCoverage: number;
  sampledPixels: number;
  uniqueColors: number;
};

test("renders Graph as a structured canvas instead of a flat orange field", async ({ page }, testInfo) => {
  const browserErrors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, {
    locale: "en",
    localStorage: { "wenlan-theme": "light" },
    rawActions: [],
  });
  await page.goto("/");

  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Graph", exact: true })
    .click();

  const graph = page.getByTestId("atlas-view");
  await expect(graph).toBeVisible();
  // Notes lead the line now, and the counts are over what is actually drawn:
  // seven notes plus the three entities that have a connection.
  await expect(page.getByText(/^7 pages · 3 topics(?: · \d+ regions?)?$/)).toBeVisible();

  // Regions stay quiet by default. Names and contours have separate transparent
  // canvases, above and below Sigma respectively, and become visible together.
  const ours = graph.locator('canvas:not([class*="sigma-"])');
  await expect(ours).toHaveCount(2);
  const canvas = graph.getByTestId("atlas-region-names");
  const areas = graph.getByTestId("atlas-community-areas");
  const display = page.getByRole("button", { name: "Display", exact: true });
  await display.click();
  const regions = page.getByRole("button", { name: "Regions", exact: true });
  await expect(regions).toHaveAttribute("aria-pressed", "false");
  await expect(canvas).toBeHidden();
  await expect(areas).toBeHidden();
  await expect(page.getByRole("group", { name: "Show in graph" })
    .getByRole("button", { name: "Memories", exact: true })).toHaveAttribute("aria-pressed", "false");
  await regions.click();
  await expect(regions).toHaveAttribute("aria-pressed", "true");
  await expect(canvas).toBeVisible();
  await expect(areas).toBeVisible();
  await expect(canvas).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
  await expect(areas).toHaveCSS("background-color", "rgba(0, 0, 0, 0)");

  const readOverlay = (canvas: Locator): Promise<CanvasEvidence> =>
    canvas.evaluate((node): CanvasEvidence => {
      if (!(node instanceof HTMLCanvasElement)) {
        return { coloredPixels: 0, orangeCoverage: 1, sampledPixels: 0, uniqueColors: 0 };
      }
      const context = node.getContext("2d", { willReadFrequently: true });
      if (!context) {
        return { coloredPixels: 0, orangeCoverage: 1, sampledPixels: 0, uniqueColors: 0 };
      }
      const pixels = context.getImageData(0, 0, node.width, node.height).data;
      const colors = new Set<string>();
      let coloredPixels = 0;
      let orangePixels = 0;
      let sampledPixels = 0;
      for (let y = 0; y < node.height; y += 2) {
        for (let x = 0; x < node.width; x += 2) {
          sampledPixels += 1;
          const offset = (y * node.width + x) * 4;
          const red = pixels[offset] ?? 0;
          const green = pixels[offset + 1] ?? 0;
          const blue = pixels[offset + 2] ?? 0;
          const alpha = pixels[offset + 3] ?? 0;
          if (alpha === 0) continue;
          coloredPixels += 1;
          // Raw alpha in the key: anti-aliased text shows many alpha steps
          // where a flat fill shows one.
          colors.add(`${red >> 4}:${green >> 4}:${blue >> 4}:${alpha}`);
          if (red > 170 && green > 55 && green < 175 && blue < 100) {
            orangePixels += 1;
          }
        }
      }
      return {
        coloredPixels,
        orangeCoverage: sampledPixels === 0 ? 1 : orangePixels / sampledPixels,
        sampledPixels,
        uniqueColors: colors.size,
      };
    });

  // The fixture has one community. Verify it actually paints a contour without
  // recreating the old orange flood. Names may yield to a visible hub label.
  let evidence: CanvasEvidence = { coloredPixels: 0, orangeCoverage: 1, sampledPixels: 0, uniqueColors: 0 };
  await expect.poll(async () => {
    evidence = await readOverlay(areas);
    return evidence.coloredPixels;
  }).toBeGreaterThan(25);
  expect(evidence.sampledPixels).toBeGreaterThan(0);
  expect(evidence.coloredPixels / evidence.sampledPixels).toBeLessThan(0.5);
  expect(evidence.orangeCoverage).toBeLessThan(0.01);
  const names = await readOverlay(canvas);
  expect(names.coloredPixels / names.sampledPixels).toBeLessThan(0.02);
  expect(names.orangeCoverage).toBeLessThan(0.01);

  await regions.click();
  await expect(regions).toHaveAttribute("aria-pressed", "false");
  await expect(canvas).toBeHidden();
  await expect(areas).toBeHidden();
  await display.click();
  await expect(page.getByRole("group", { name: "Show in graph" })).toHaveCount(0);
  await page.mouse.move(1, 1);
  // Check the rendered map itself as well as the transparent overlays. Read a
  // screenshot because WebGL may discard its drawing buffer after presenting.
  const graphBox = await graph.boundingBox();
  expect(graphBox!.height).toBeGreaterThan(700);
  const mapImage = await graph.screenshot();
  const pixels = await page.evaluate(async (base64) => {
    const image = new Image();
    image.src = `data:image/png;base64,${base64}`;
    await image.decode();
    const surface = document.createElement("canvas");
    surface.width = image.width;
    surface.height = image.height;
    const ctx = surface.getContext("2d")!;
    ctx.drawImage(image, 0, 0);
    const data = ctx.getImageData(0, 0, image.width, image.height).data;
    let colored = 0;
    let orange = 0;
    for (let i = 0; i < data.length; i += 4) {
      const [r, g, b] = [data[i], data[i + 1], data[i + 2]];
      if (Math.max(r, g, b) - Math.min(r, g, b) > 30) colored++;
      if (r > 170 && g > 55 && g < 175 && b < 100) orange++;
    }
    return { colored, orange, total: image.width * image.height };
  }, mapImage.toString("base64"));
  expect(pixels.colored, "the graph must actually draw colored nodes").toBeGreaterThan(100);
  expect(pixels.colored / pixels.total, "nodes must leave a readable background").toBeLessThan(0.05);
  expect(pixels.orange / pixels.total, "no orange flood over the map").toBeLessThan(0.01);
  // Persist the image even with CI's text reporter, so artifact upload sees it.
  const screenshotPath = testInfo.outputPath("graph-1280x900-light.png");
  await page.screenshot({ path: screenshotPath, animations: "disabled", fullPage: false });
  await testInfo.attach("graph-1280x900-light", {
    path: screenshotPath,
    contentType: "image/png",
  });
  expect(browserErrors.pageErrors).toEqual([]);
  expect(browserErrors.consoleErrors).toEqual([]);
});

test("Graph keeps app search focused and canvas controls reachable", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  await page.getByRole("navigation", { name: "Primary navigation" })
    .getByRole("button", { name: "Graph", exact: true }).click();
  const jump = page.getByRole("combobox", { name: "Filter nodes" });
  await expect(jump).toBeVisible();
  await jump.click();
  await page.evaluate(async () => {
    await window.__TAURI_INTERNALS__?.invoke("plugin:event|emit", { event: "toggle-spotlight", payload: null });
  });
  const search = page.getByRole("dialog", { name: "Search", exact: true });
  const input = search.getByPlaceholder("Search pages, memories, sources...");
  await expect(input).toBeFocused();
  await input.press("Meta+k");
  await expect(input).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await expect(jump).toBeFocused();
  for (const name of ["Zoom in", "Zoom out", "Reset view", "Fit entire graph"]) {
    await page.getByRole("button", { name, exact: true }).click();
  }
});


test("Graph can open a page while its focus camera is still moving", async ({ page }) => {
  const errors = collectBrowserErrors(page);
  await page.setViewportSize({ width: 375, height: 900 });
  await installTauriMock(page, { locale: "en", rawActions: [] });
  await page.goto("/");
  await openPrimaryDestination(page, "Graph");
  const search = page.getByRole("combobox", { name: "Filter nodes" });
  await search.fill("Fixture architecture");
  await page.getByRole("option", { name: /Fixture architecture/ }).click();
  const panel = page.getByRole("dialog", { name: "Fixture architecture", exact: true });
  // Deliberately navigate during the 450 ms focus fly; do not wait for it to settle.
  await panel.getByRole("button", { name: "Open details", exact: true }).click();
  await expect(panel).toHaveCount(0);
  await expect(page.getByRole("tab", { name: "Fixture architecture", exact: true })).toHaveAttribute("aria-selected", "true");
  expect(errors.pageErrors).toEqual([]);
  expect(errors.consoleErrors).toEqual([]);
});

for (const width of [1440, 375]) {
  test(`keyboard filter selection keeps the graph inspector open at ${width}px`, async ({ page }) => {
    const errors = collectBrowserErrors(page);
    await page.setViewportSize({ width, height: 900 });
    await installTauriMock(page, { locale: "en", rawActions: [] });
    await page.goto("/");
    await openPrimaryDestination(page, "Graph");
    const filter = page.getByRole("combobox", { name: "Filter nodes" });
    await filter.fill("Fixture architecture");
    await expect(page.getByRole("option", { name: "Fixture architecture", exact: true })).toBeVisible();
    await filter.press("ArrowDown");
    await filter.press("Enter");
    const inspector = page.locator(".page-info-drawer--inspector");
    await expect(inspector).toBeVisible();
    await expect(inspector.getByRole("heading", { name: "Fixture architecture", exact: true })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(inspector).toHaveCount(0);
    await expect(filter).toBeFocused();
    expect(errors.pageErrors).toEqual([]);
    expect(errors.consoleErrors).toEqual([]);
  });
}
