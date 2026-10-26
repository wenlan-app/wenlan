#!/usr/bin/env node

const assert = require("node:assert/strict");
const path = require("node:path");
const test = require("node:test");

const {
  checkPngMatchesExpected,
  makeBanner,
} = require("./generate-readme-visuals.cjs");
const {
  family,
  makeOverview,
  makeLifecycle,
  makeKnowledgeNetwork,
} = require("./readme-product-visuals.cjs");

const ASSET_DIR = path.resolve(__dirname, "..", "docs", "assets");

test("borderless banners preserve four-locale copy in desktop and mobile layouts", () => {
  const expected = {
    en: ["Your living wiki", "AI organizes. You stay in control."],
    "zh-Hant": ["持續更新的知識庫", "AI 幫你整理，你保有主導權"],
    "zh-Hans": ["持续更新的知识库", "AI 帮你整理，你保有主导权"],
    "es-ES": ["Tu wiki viva", "La IA organiza. Tú tienes el control."],
  };

  for (const [locale, lines] of Object.entries(expected)) {
    for (const viewport of ["desktop", "mobile"]) {
      const banner = makeBanner(viewport, locale);
      const suffix = locale === "en" ? "" : `-${locale}`;
      assert.equal(banner.name, `readme-banner${suffix}${viewport === "mobile" ? "-mobile" : ""}`);
      assert.deepEqual([banner.width, banner.height], viewport === "desktop" ? [1120, 260] : [720, 256]);
      assert.match(banner.svg, new RegExp(`>${lines[0].replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}<`, "u"));
      assert.doesNotMatch(banner.svg, new RegExp(`>${lines[1].replace(/[.*+?^${}()|[\]\\]/gu, "\\$&")}<`, "u"));
      assert.doesNotMatch(banner.svg, /letter-spacing="4"|>WENLAN</u);
      assert.match(banner.svg, />Wenlan<\/tspan>/u);
      assert.doesNotMatch(banner.svg, /data-fit-region|input-1|input-2|topic-page|\[1\]|\[2\]/u);
      assert.doesNotMatch(banner.svg, /stroke="#2F3769"/u);
      assert.equal((banner.svg.match(/<rect /gu) ?? []).length, 2, "only background and logo tile");

    }
  }
});

test("PNG verification rejects different content with identical dimensions", async () => {
  const expectedPath = path.join(ASSET_DIR, "wenlan-system.png");
  const differentPath = path.join(ASSET_DIR, "wenlan-system-zh-Hans.png");

  const errors = await checkPngMatchesExpected(differentPath, expectedPath);

  assert.equal(errors.length, 1);
  assert.match(errors[0], /does not match generated output/u);
});

test("PNG verification accepts byte-identical generated output", async () => {
  const expectedPath = path.join(ASSET_DIR, "wenlan-system.png");

  const errors = await checkPngMatchesExpected(expectedPath, expectedPath);

  assert.deepEqual(errors, []);
});

test("PNG verification reports dimension mismatches", async () => {
  const currentPath = path.join(ASSET_DIR, "wenlan-system-mobile.png");
  const expectedPath = path.join(ASSET_DIR, "wenlan-system.png");

  const errors = await checkPngMatchesExpected(currentPath, expectedPath);

  assert.equal(errors.length, 1);
  assert.match(errors[0], /is \d+x\d+; generated output is \d+x\d+/u);
});

test("all diagram locales reuse the app's typography tokens", () => {
  const css = require("node:fs").readFileSync(path.join(__dirname, "../src/index.css"), "utf8");
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const kind of ["heading", "body", "mono"]) {
      const expected = css.match(new RegExp(`--mem-font-${kind}:\\s*([^;]+);`))[1];
      assert.equal(family(locale, kind), expected);
      assert.doesNotMatch(family(locale, kind), /Songti|STSong/u);
    }
  }
});

test("knowledge-network visual distinguishes entities and relation semantics", () => {
  const asset = makeKnowledgeNetwork("en", "desktop");

  assert.equal(asset.name, "wenlan-knowledge-network");
  assert.match(asset.svg, />KNOWLEDGE PAGE</u);
  assert.match(asset.svg, />ENTITY</u);
  assert.doesNotMatch(asset.svg, /ENTITY PAGE/u);
  assert.match(asset.svg, />SOURCE PAGE</u);
  assert.match(asset.svg, />MEMORY</u);
  assert.match(asset.svg, /marker-end="url\(#network-en-desktop-relation-arrow\)"/u);
  assert.match(asset.svg, />PART OF</u);
  assert.match(asset.svg, />RELATED TO</u);
  assert.match(asset.svg, />GROUPED BY RELATION DENSITY</u);
  assert.doesNotMatch(asset.svg, /IN PROGRESS/u);
});

test("overview memory icon matches the app navigation brain geometry", () => {
  const navigation = require("node:fs").readFileSync(
    path.join(__dirname, "../src/components/memory/navigation/PrimaryNavigation.tsx"), "utf8",
  );
  const brain = navigation.match(/data-navigation-icon="brain"[\s\S]*?<\/svg>/u)?.[0];
  assert.ok(brain, "App memory navigation icon must exist");
  const appPaths = [...brain.matchAll(/"(M[^"\n]+)"/gu)].map((match) => match[1]);
  assert.equal(appPaths.length, 6);
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const viewport of ["desktop", "mobile"]) {
      const icon = makeOverview(locale, viewport).svg.match(/<g data-icon="app-memory-brain"[\s\S]*?<\/g>/u)?.[0];
      assert.ok(icon, `${locale}/${viewport} memory icon`);
      assert.deepEqual([...icon.matchAll(/<path d="([^"]+)"/gu)].map((match) => match[1]), appPaths);
      assert.ok(icon.includes(`scale(${28 / 24})`));
      assert.match(icon, /stroke-width="1\.8"/u);
      assert.match(icon, /stroke-linejoin="round"/u);
    }
  }
});

test("knowledge-network visual has desktop and mobile assets for both Chinese locales", () => {
  assert.equal(
    makeKnowledgeNetwork("zh-Hans", "desktop").name,
    "wenlan-knowledge-network-zh-Hans",
  );
  assert.equal(
    makeKnowledgeNetwork("zh-Hant", "mobile").name,
    "wenlan-knowledge-network-zh-Hant-mobile",
  );
});

test("every knowledge-network locale keeps direction and strength semantics", () => {
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const viewport of ["desktop", "mobile"]) {
      const { svg } = makeKnowledgeNetwork(locale, viewport);
      const graphBody = svg.split("<defs>")[0];
      const sagePaths = graphBody.match(/<path[^>]+stroke="#6F8F76"[^>]+\/>/gu) ?? [];
      const directedPaths = graphBody.match(/<path[^>]+marker-end=[^>]+\/>/gu) ?? [];

      assert.equal(sagePaths.length, 5, `${locale}/${viewport} sage relation count`);
      assert.equal(directedPaths.length, 5, `${locale}/${viewport} directed relation count`);
      for (const pathMarkup of sagePaths) {
        assert.match(pathMarkup, /marker-end=/u, `${locale}/${viewport} sage relation direction`);
      }
      assert.doesNotMatch(svg, /0\.82/u, `${locale}/${viewport} avoids invented numerical confidence`);
      assert.doesNotMatch(svg, /ENTITY PAGE|实体页面|實體頁面/u);
    }
  }
});

test("lifecycle explains traceable history, evidence updates, and review in plain language", () => {
  for (const viewport of ["desktop", "mobile"]) {
    const overview = makeOverview("en", viewport).svg;
    const lifecycle = makeLifecycle("en", viewport).svg;
    const network = makeKnowledgeNetwork("en", viewport).svg;
    assert.match(overview, /configured model/u);
    assert.match(overview, /Automatic refresh proposes changes/u);
    assert.match(lifecycle, /Earlier knowledge stays traceable/u);
    assert.match(lifecycle, /New evidence improves the note/u);
    assert.match(lifecycle, /AI changes to your edited page wait for review/u);
    assert.doesNotMatch(lifecycle, /Model steps need a configured model|Conflict reconciliation is off by default/u);
    assert.doesNotMatch(lifecycle, /VERIFIED REBUILD|Archive, never delete|On-device \/ off/u);
    assert.match(network, /Illustrative example/u);
    assert.doesNotMatch(network, /Pages connect through Entities/u);
  }
});

test("overview shows an illustrative, source-linked Agent Loop note instead of placeholder IDs", () => {
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const viewport of ["desktop", "mobile"]) {
      const { svg } = makeOverview(locale, viewport);
      assert.match(svg, />Agent Loop</u);
      assert.match(svg, /Claude/u);
      assert.match(svg, /Codex/u);
      assert.match(svg, /\[1\]/u);
      assert.match(svg, /\[2\]/u);
      assert.match(svg, /Illustrative example|示例内容|示例內容/u);
      assert.doesNotMatch(svg, /source_\d+|mem_\d+|fill="#DDE2EA"/u);
      assert.match(svg, /ADDED AFTER A UI FIX|一次实践后补上的规则|一次實作後補上的規則/u);
      assert.match(svg, /Tests passed; a line still covered a label|测试全过，手机标签仍被连线遮住|測試全過，手機標籤仍被連線遮住/u);
      assert.match(svg, /desktop and mobile views|桌面与手机画面都要看|桌面與手機畫面都要看/u);
      assert.doesNotMatch(svg, /THE CORE IDEA|核心概念|Choose an action, use a tool|先留下目标|先留下目標/u);
    }
  }
});

test("desktop and mobile graph layouts preserve the exact same evidence relationships", () => {
  const expected = [
    "sourceOne>page:citation", "sourceTwo>page:citation",
    "memoryOne>page:support", "memoryTwo>page:support",
    "memoryOne>entityOne:about", "page>linkedPage:refines",
    "sourceOne>linkedPage:citation", "entityOne>entityTwo:related_to",
    "entityOne>entityThree:related_to", "entityFour>entityOne:part_of",
    "entityTwo>entityThree:related_to", "entityThree>entityFour:related_to",
  ];
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const viewport of ["desktop", "mobile"]) {
      const { svg } = makeKnowledgeNetwork(locale, viewport);
      const actual = [...svg.matchAll(/data-from="([^"]+)" data-to="([^"]+)" data-relation="([^"]+)"/gu)]
        .map(([, from, to, relation]) => `${from}>${to}:${relation}`);
      assert.deepEqual(actual, expected, `${locale}/${viewport}`);
    }
  }
});

test("network and lifecycle continue the same concrete Agent Loop example", () => {
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const viewport of ["desktop", "mobile"]) {
      const network = makeKnowledgeNetwork(locale, viewport).svg;
      const lifecycle = makeLifecycle(locale, viewport).svg;
      for (const svg of [network, lifecycle]) {
        assert.match(svg, />Agent Loop</u);
        assert.match(svg, /Illustrative example|ILLUSTRATIVE EXAMPLE|示例内容|示例內容/u);
        assert.match(svg, /\[1\]/u);
        assert.match(svg, /\[2\]/u);
        assert.doesNotMatch(svg, /mem_\d+|source_\d+|fill="#D9DEE7"|v12|Wenlan positioning|Wenlan 定位/u);
      }
      assert.match(network, /UI checklist|验收清单|驗收清單/u);
      assert.match(network, /Retry rule|重试准则|重試準則/u);
      assert.match(lifecycle, /Tests passed|测试通过就算完成|測試通過就算完成/u);
      assert.match(lifecycle, /Tests \+ visual checks|测试之外，也要看画面|測試之外，也要看畫面/u);
      assert.match(lifecycle, /ADDED FROM THE UI REVIEW|从这次审查补上的规则|從這次審查補上的規則/u);
      assert.match(lifecycle, /Earlier knowledge stays traceable|先前的判斷仍可追溯|先前的判断仍可追溯/u);
      assert.match(lifecycle, /AI changes to your edited page wait for review|AI 對你編輯過的頁面所做的修改，會先等你審核|AI 对你编辑过的页面所做的修改，会先等你审核/u);
    }
  }
});

test("lifecycle removes technical upkeep jargon and keeps localized update labels", () => {
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    for (const viewport of ["desktop", "mobile"]) {
      const { svg } = makeLifecycle(locale, viewport);
      assert.doesNotMatch(svg, />UNDERSTAND<|>RECONCILE<|>VERIFY<|>理解<|>核對</u);
      assert.doesNotMatch(svg, /configured model|模型步骤需配置模型|模型步驟需設定模型|默认关闭|預設關閉|off by default|Conflict reconciliation is off by default|冲突校正默认关闭|衝突校正預設關閉/u);
      assert.doesNotMatch(svg, />REFINERY<|精炼|精煉|>ENRICH<|丰富|豐富|>RECONCILE<|校正|SUPERSEDES|OPTIONAL CONFLICT REVIEW|ENRICH|CONNECT/u);
      if (locale !== "en") {
        assert.doesNotMatch(svg, />STALE</u);
        assert.match(svg, />待更新</u);
      }
    }
  }
});

test("mobile diagrams retain content while reducing the previous combined height by at least 20 percent", () => {
  for (const locale of ["en", "zh-Hans", "zh-Hant"]) {
    const assets = [makeOverview(locale, "mobile"), makeKnowledgeNetwork(locale, "mobile"), makeLifecycle(locale, "mobile")];
    assert.ok(assets.every((asset) => asset.width === 720));
    assert.ok(assets.reduce((sum, asset) => sum + asset.height, 0) <= (2210 + 2000 + 2820) * 0.8);
  }
});
