#!/usr/bin/env node

const fs = require("node:fs");
const crypto = require("node:crypto");
const { createRequire } = require("node:module");
const os = require("node:os");
const path = require("node:path");

const {
  makeOverview,
  makeKnowledgeNetwork,
  makeLifecycle,
} = require("./readme-product-visuals.cjs");

const ROOT = path.resolve(__dirname, "..");
const ASSET_DIR = path.join(ROOT, "docs", "assets");
const FONT_DIR = path.join(ROOT, "public", "fonts");
const TOOL_DIR = path.join(__dirname, "readme-visuals");
const visualRequire = createRequire(path.join(TOOL_DIR, "package.json"));

const BANNER_VIEWPORTS = {
  desktop: { width: 1120, height: 260 },
  mobile: { width: 720, height: 256 },
};

const BANNER = {
  // Deep ink blue complements the logo; preserve the established text colors.
  dark: "#151D2B",
  white: "#F7F8FF",
  accent: "#93E3F2",
  secondary: "#BAC4D2",
  latin: '-apple-system, BlinkMacSystemFont, "SF Pro Display", "Helvetica Neue", Arial, sans-serif',
};

const BANNER_COPY = {
  en: {
    title: ["Your living wiki"],
    tagline: "Free · Open source · Local-first",
    font: BANNER.latin,
  },
  "zh-Hant": {
    title: ["持續更新的知識庫"],
    tagline: "免費・開源・本地優先",
    font: '"PingFang TC", "Noto Sans TC", -apple-system, BlinkMacSystemFont, sans-serif',
  },
  "zh-Hans": {
    title: ["持续更新的知识库"],
    tagline: "免费・开源・本地优先",
    font: '"PingFang SC", "Noto Sans SC", -apple-system, BlinkMacSystemFont, sans-serif',
  },
  "es-ES": {
    title: ["Tu wiki viva"],
    tagline: "Gratis · Código abierto · Local-first",
    font: BANNER.latin,
  },
};

function esc(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function logoDefs(prefix) {
  return `<linearGradient id="${prefix}-ring" x1="96" y1="256" x2="416" y2="256" gradientUnits="userSpaceOnUse">
      <stop stop-color="#6C63FF"/>
      <stop offset="0.5" stop-color="#5BA3E6"/>
      <stop offset="1" stop-color="#4AC8E8"/>
    </linearGradient>
    <radialGradient id="${prefix}-orb" cx="0" cy="0" r="1" gradientUnits="userSpaceOnUse" gradientTransform="translate(310 144) rotate(52.125) scale(76.0263)">
      <stop stop-color="#FFFFFF"/>
      <stop offset="0.45" stop-color="#A5C4F7"/>
      <stop offset="1" stop-color="#4AC8E8"/>
    </radialGradient>`;
}

function logoMarkup({ x, y, size, prefix }) {
  const scale = size / 512;
  return `<g transform="translate(${x} ${y}) scale(${scale})">
    <rect width="512" height="512" rx="112" fill="#1A1A2E"/>
    <circle cx="256" cy="256" r="160" fill="none" stroke="url(#${prefix}-ring)" stroke-width="76"/>
    <circle cx="322" cy="160" r="42" fill="url(#${prefix}-orb)"/>
  </g>`;
}

function makeSimpleBanner(viewport, locale) {
  const { width, height } = BANNER_VIEWPORTS[viewport];
  const mobile = viewport === "mobile";
  const copy = BANNER_COPY[locale];
  const prefix = `banner-${locale}-${viewport}`;
  const text = (x, y, value, size, color = BANNER.white, weight = 400, anchor = "start", letterSpacing = null) =>
    `<text x="${x}" y="${y}" fill="${color}" font-family="${esc(copy.font)}" font-size="${size}" font-weight="${weight}" text-anchor="${anchor}"${letterSpacing === null ? "" : ` letter-spacing="${letterSpacing}"`}>${esc(value)}</text>`;
  const chineseName = locale === "zh-Hant" ? "文瀾" : locale === "zh-Hans" ? "文澜" : "";
  const brand = (x, y, size, anchor) =>
    `<text x="${x}" y="${y}" fill="${BANNER.accent}" font-size="${size}" font-weight="500" text-anchor="${anchor}"><tspan font-family="${esc(BANNER.latin)}">Wenlan</tspan>${chineseName ? `<tspan dx="${+(size * 0.25).toFixed(2)}" font-family="${esc(copy.font)}">${chineseName}</tspan>` : ""}</text>`;
  const tagline = copy.tagline ?? null;
  // With a tagline the three lines stay centred on the logo, and the tagline sits above the
  // ring's lower edge: the dark tile blends into the background, so the ring is what reads.
  // The Wenlan name and the title stay close in size, which reads as one heading.
  const content = mobile
    ? tagline
      ? `${logoMarkup({ x: 320, y: chineseName ? 32 : 34, size: 80, prefix })}
  ${brand(360, chineseName ? 146 : 148, chineseName ? 29.12 : 30.24, "middle")}
  ${text(360, chineseName ? 186 : 183, copy.title[0], 30, BANNER.white, 500, "middle")}
  ${text(360, chineseName ? 218 : 215, tagline, 18, BANNER.secondary, 400, "middle")}`
      : `${logoMarkup({ x: 320, y: 32, size: 80, prefix })}
  ${brand(360, 150, chineseName ? 29.12 : 30.24, "middle")}
  ${text(360, 195, copy.title[0], 32, BANNER.white, 500, "middle")}`
    : tagline
      ? `${logoMarkup({ x: 81.28, y: 84.28, size: 181.44, prefix })}
  ${brand(300, chineseName ? 141 : 146, chineseName ? 41.6 : 43.2, "start")}
  ${text(300, chineseName ? 196 : 191, copy.title[0], 40, BANNER.white, 500, "start", chineseName ? 0.65 : null)}
  ${text(300, chineseName ? 234 : 229, tagline, 20, BANNER.secondary, 400, "start", chineseName ? 0.65 : null)}`
      : `${logoMarkup({ x: 81.28, y: 84.28, size: 181.44, prefix })}
  ${brand(300, 153, chineseName ? 41.6 : 43.2, "start")}
  ${text(300, 219, copy.title[0], 40, BANNER.white, 500, "start", chineseName ? 0.65 : null)}`;

  const svg = `<svg width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" fill="none" xmlns="http://www.w3.org/2000/svg" role="img" aria-labelledby="title desc">
  <title id="title">Wenlan README banner</title>
  <desc id="desc">${esc(`Wenlan: ${copy.title.join(" ")}`)}</desc>
  <rect width="${width}" height="${height}" fill="${BANNER.dark}"/>
  ${mobile ? content : `<g transform="translate(172 -45)">${content}</g>`}
  <defs>
    ${logoDefs(prefix)}
  </defs>
</svg>
`;

  return {
    group: "banner",
    name: `readme-banner${locale === "en" ? "" : `-${locale}`}${mobile ? "-mobile" : ""}`,
    width,
    height,
    background: BANNER.dark,
    requiredCopy: ["Wenlan", ...(chineseName ? [chineseName] : []), ...copy.title, ...(tagline ? [tagline] : [])],
    svg,
  };
}

function makeBanner(viewport, locale = "en") {
  return makeSimpleBanner(viewport, locale);
}

function fontData(filename) {
  const file = path.join(FONT_DIR, filename);
  if (!fs.existsSync(file)) {
    throw new Error(`Missing README render font: ${path.relative(ROOT, file)}`);
  }
  return fs.readFileSync(file).toString("base64");
}

let renderFontCss;

function embeddedFonts() {
  if (renderFontCss) return renderFontCss;
  // Embed the same bundled variable fonts used by the desktop app. No network
  // requests and no separate static-font copies with different weight metrics.
  renderFontCss = fs.readFileSync(path.join(FONT_DIR, "fonts.css"), "utf8")
    .replace(/\/\*[\s\S]*?\*\//gu, "")
    .replace(/url\(\/fonts\/([A-Za-z0-9_-]+\.woff2)\)/gu,
      (_, filename) => `url(data:font/woff2;base64,${fontData(filename)})`);
  return renderFontCss;
}

function selectedAssets(only) {
  const banner = Object.keys(BANNER_COPY).flatMap((locale) =>
    [makeBanner("desktop", locale), makeBanner("mobile", locale)]);
  const overview = ["en", "zh-Hans", "zh-Hant"].flatMap((locale) => [
    makeOverview(locale, "desktop"),
    makeOverview(locale, "mobile"),
  ]);
  const lifecycle = ["en", "zh-Hans", "zh-Hant"].flatMap((locale) => [
    makeLifecycle(locale, "desktop"),
    makeLifecycle(locale, "mobile"),
  ]);
  const network = ["en", "zh-Hans", "zh-Hant"].flatMap((locale) => [
    makeKnowledgeNetwork(locale, "desktop"),
    makeKnowledgeNetwork(locale, "mobile"),
  ]);
  if (only === "banner") return banner;
  if (only === "overview") return overview;
  if (only === "lifecycle") return lifecycle;
  if (only === "network") return network;
  return [...banner, ...overview, ...network, ...lifecycle];
}

async function renderSvgToPng(asset, pngPath) {
  let chromium;
  try {
    ({ chromium } = visualRequire("playwright"));
  } catch (error) {
    throw new Error(
      "Missing README visual dependencies. Run "
      + "`npm ci --ignore-scripts --prefix scripts/readme-visuals` and "
      + "`npm run --prefix scripts/readme-visuals install-browser`.",
      { cause: error },
    );
  }
  const browser = await chromium.launch({ headless: true });
  try {
    const rasterScale = asset.group === "banner" ? 3 : 1;
    const context = await browser.newContext({
      viewport: { width: asset.width, height: asset.height },
      deviceScaleFactor: rasterScale,
    });
    const page = await context.newPage();
    await page.setContent(
      `<style>
        ${embeddedFonts()}
        html, body {
          margin: 0;
          width: ${asset.width}px;
          height: ${asset.height}px;
          overflow: hidden;
        }
        svg { display: block; }
      </style>${asset.svg}`,
      { waitUntil: "load" },
    );
    await page.evaluate(() => document.fonts.ready);
    const layoutErrors = await page.locator("svg").evaluate((svg) => {
      const errors = [];
      const tolerance = 1;
      const view = svg.viewBox.baseVal;
      for (const region of svg.querySelectorAll("[data-fit-region]")) {
        const id = region.getAttribute("data-fit-region");
        const bounds = {
          x: Number(region.getAttribute("data-fit-x")),
          y: Number(region.getAttribute("data-fit-y")),
          width: Number(region.getAttribute("data-fit-width")),
          height: Number(region.getAttribute("data-fit-height")),
        };
        const texts = Array.from(region.querySelectorAll("text")).map((node) => ({
          text: node.textContent || "",
          box: node.getBBox(),
        }));
        for (const item of texts) {
          const { box } = item;
          if (
            box.width <= 0
            || box.height <= 0
            || box.x < bounds.x - tolerance
            || box.y < bounds.y - tolerance
            || box.x + box.width > bounds.x + bounds.width + tolerance
            || box.y + box.height > bounds.y + bounds.height + tolerance
          ) {
            errors.push(
              `${id}: ${JSON.stringify(item.text)} leaves its assigned region `
              + `(text ${box.x.toFixed(1)},${box.y.toFixed(1)},${box.width.toFixed(1)},${box.height.toFixed(1)}; `
              + `region ${bounds.x},${bounds.y},${bounds.width},${bounds.height})`,
            );
          }
        }
        if (region.getAttribute("data-check-overlap") !== "false") {
          for (let left = 0; left < texts.length; left += 1) {
            for (let right = left + 1; right < texts.length; right += 1) {
              const a = texts[left].box;
              const b = texts[right].box;
              const overlapX = Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x);
              const overlapY = Math.min(a.y + a.height, b.y + b.height) - Math.max(a.y, b.y);
              if (overlapX > tolerance && overlapY > tolerance) {
                errors.push(
                  `${id}: ${JSON.stringify(texts[left].text)} overlaps ${JSON.stringify(texts[right].text)}`,
                );
              }
            }
          }
        }
      }
      for (const node of svg.querySelectorAll("text")) {
        // getBBox is in the text's own coordinates; map it through the parent transforms
        // (the desktop banner's translate) so the check measures where it is drawn.
        const local = node.getBBox();
        const ctm = node.getCTM();
        const corners = [
          [local.x, local.y],
          [local.x + local.width, local.y],
          [local.x, local.y + local.height],
          [local.x + local.width, local.y + local.height],
        ].map(([x, y]) => new DOMPoint(x, y).matrixTransform(ctm));
        const xs = corners.map((point) => point.x);
        const ys = corners.map((point) => point.y);
        const box = {
          x: Math.min(...xs),
          y: Math.min(...ys),
          width: Math.max(...xs) - Math.min(...xs),
          height: Math.max(...ys) - Math.min(...ys),
        };
        if (
          box.x < -tolerance
          || box.y < -tolerance
          || box.x + box.width > view.width + tolerance
          || box.y + box.height > view.height + tolerance
        ) {
          errors.push(`viewport: ${JSON.stringify(node.textContent || "")} is clipped`);
        }
      }
      return errors;
    });
    if (layoutErrors.length > 0) {
      throw new Error(`${asset.name} layout check failed:\n- ${layoutErrors.join("\n- ")}`);
    }
    if (
      asset.name.includes("-zh-Hans-mobile")
      || asset.name.includes("-zh-Hant-mobile")
    ) {
      // Chromium's element screenshot produces scale-sensitive tile artifacts
      // in the tall CJK SVGs. Offscreen rasterization preserves every glyph.
      const pngBase64 = await page.evaluate(async ({
        svgSource,
        fontCss,
        width,
        height,
        rasterScale,
        background,
      }) => {
        const parsed = new DOMParser().parseFromString(svgSource, "image/svg+xml");
        const style = parsed.createElementNS("http://www.w3.org/2000/svg", "style");
        style.textContent = fontCss;
        parsed.documentElement.prepend(style);

        const serialized = new XMLSerializer().serializeToString(parsed.documentElement);
        const url = URL.createObjectURL(new Blob([serialized], { type: "image/svg+xml" }));
        try {
          const image = new Image();
          image.src = url;
          await new Promise((resolve, reject) => {
            image.onload = resolve;
            image.onerror = () => reject(new Error("Failed to rasterize README SVG"));
          });

          const canvas = document.createElement("canvas");
          canvas.width = width * rasterScale;
          canvas.height = height * rasterScale;
          const context2d = canvas.getContext("2d");
          if (!context2d) throw new Error("README canvas context is unavailable");
          context2d.fillStyle = background;
          context2d.fillRect(0, 0, canvas.width, canvas.height);
          context2d.drawImage(image, 0, 0, canvas.width, canvas.height);
          return canvas.toDataURL("image/png").split(",", 2)[1];
        } finally {
          URL.revokeObjectURL(url);
        }
      }, {
        svgSource: asset.svg,
        fontCss: embeddedFonts(),
        width: asset.width,
        height: asset.height,
        rasterScale,
        background: asset.background,
      });
      fs.writeFileSync(pngPath, Buffer.from(pngBase64, "base64"));
    } else {
      await page.locator("svg").screenshot({ path: pngPath, omitBackground: false });
    }
    await context.close();
  } finally {
    await browser.close();
  }
}

async function writeAsset(asset) {
  fs.mkdirSync(ASSET_DIR, { recursive: true });
  const svgPath = path.join(ASSET_DIR, `${asset.name}.svg`);
  const pngPath = path.join(ASSET_DIR, `${asset.name}.png`);
  fs.writeFileSync(svgPath, asset.svg, "utf8");
  await renderSvgToPng(asset, pngPath);
}

function shortHash(buffer) {
  return crypto.createHash("sha256").update(buffer).digest("hex").slice(0, 12);
}

function pngDimensions(buffer) {
  if (buffer.length < 24 || buffer.toString("ascii", 12, 16) !== "IHDR") {
    return null;
  }
  return {
    width: buffer.readUInt32BE(16),
    height: buffer.readUInt32BE(20),
  };
}

async function checkPngMatchesExpected(currentPath, expectedPath) {
  if (!fs.existsSync(currentPath)) {
    return [`missing ${path.relative(ROOT, currentPath)}`];
  }
  if (!fs.existsSync(expectedPath)) {
    return [`missing generated comparison PNG ${expectedPath}`];
  }

  const current = fs.readFileSync(currentPath);
  const expected = fs.readFileSync(expectedPath);
  if (current.equals(expected)) {
    return [];
  }

  const currentSize = pngDimensions(current);
  const expectedSize = pngDimensions(expected);
  if (
    currentSize
    && expectedSize
    && (
      currentSize.width !== expectedSize.width
      || currentSize.height !== expectedSize.height
    )
  ) {
    return [
      `${path.relative(ROOT, currentPath)} is `
      + `${currentSize.width}x${currentSize.height}; generated output is `
      + `${expectedSize.width}x${expectedSize.height}`,
    ];
  }

  return [
    `${path.relative(ROOT, currentPath)} does not match generated output `
    + `(current ${shortHash(current)}, expected ${shortHash(expected)})`,
  ];
}

async function checkPng(asset, pngPath) {
  if (!fs.existsSync(pngPath)) {
    return [`missing ${path.relative(ROOT, pngPath)}`];
  }

  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "wenlan-readme-visual-"));
  const expectedPath = path.join(tempDir, `${asset.name}.png`);
  try {
    await renderSvgToPng(asset, expectedPath);
    return await checkPngMatchesExpected(pngPath, expectedPath);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
}

function compactVisibleSvgText(svg) {
  return svg
    .replace(/<[^>]*>/g, "")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/\s+/gu, "");
}

async function checkAsset(asset) {
  const errors = [];
  const svgPath = path.join(ASSET_DIR, `${asset.name}.svg`);
  const pngPath = path.join(ASSET_DIR, `${asset.name}.png`);
  if (!fs.existsSync(svgPath)) {
    errors.push(`missing ${path.relative(ROOT, svgPath)}`);
  } else {
    const current = fs.readFileSync(svgPath, "utf8");
    if (current !== asset.svg) {
      errors.push(`${path.relative(ROOT, svgPath)} is not current`);
    }
    const visibleText = compactVisibleSvgText(current);
    for (const required of asset.requiredCopy) {
      if (!visibleText.includes(compactVisibleSvgText(required))) {
        errors.push(`${path.relative(ROOT, svgPath)} is missing ${JSON.stringify(required)}`);
      }
    }
  }
  errors.push(...(await checkPng(asset, pngPath)));
  return errors;
}

function parseArgs(argv) {
  const mode = argv.includes("--write") ? "write" : argv.includes("--check") ? "check" : null;
  const onlyIndex = argv.indexOf("--only");
  const only = onlyIndex >= 0 ? argv[onlyIndex + 1] : "all";
  if (!mode) {
    throw new Error("Use --write or --check");
  }
  if (!["all", "banner", "overview", "network", "lifecycle"].includes(only)) {
    throw new Error(`Unknown --only value: ${only}`);
  }
  return { mode, only };
}

async function main() {
  const { mode, only } = parseArgs(process.argv.slice(2));
  const assets = selectedAssets(only);
  if (mode === "write") {
    for (const asset of assets) {
      await writeAsset(asset);
    }
    console.log(`${assets.length * 2} assets generated`);
    return;
  }

  const errors = [];
  for (const asset of assets) {
    errors.push(...(await checkAsset(asset)));
  }
  if (errors.length > 0) {
    for (const error of errors) {
      console.error(`- ${error}`);
    }
    process.exitCode = 1;
    return;
  }
  console.log(`${only} assets are current`);
}

if (require.main === module) {
  main().catch((error) => {
    console.error(error.stack || error.message);
    process.exitCode = 1;
  });
}

module.exports = {
  checkPngMatchesExpected,
  checkAsset,
  makeBanner,
  writeAsset,
};
