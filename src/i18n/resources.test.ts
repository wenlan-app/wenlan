import { describe, expect, it } from "vitest";

import { resources, supportedAppLocales } from "./resources";

function flattenKeys(value: unknown, prefix = ""): string[] {
  if (!value || typeof value !== "object") {
    return [prefix];
  }

  return Object.entries(value as Record<string, unknown>).flatMap(
    ([key, child]) => flattenKeys(child, prefix ? `${prefix}.${key}` : key),
  );
}

describe("translation resources", () => {
  it("defines the supported app locales in a stable order", () => {
    expect(supportedAppLocales).toEqual(["en", "zh-Hans", "zh-Hant"]);
  });

  it("contains representative primary interface keys", () => {
    expect(resources.en.translation.setup.welcomeTitle).toBe(
      "Welcome to Wenlan",
    );
    expect(resources.en.translation.main.searchPlaceholder).toBe(
      "Search pages, memories, sources...",
    );
    expect(resources.en.translation.settings.language.label).toBe("Language");
  });

  it("keeps the Connections subtitles and concise status copy exact in every locale", () => {
    const copy = supportedAppLocales.map((locale) => resources[locale].translation.connectMatrix);
    expect(copy.map((locale) => locale.subtitleCodex)).toEqual([
      "ChatGPT desktop and Codex CLI share one setup",
      "ChatGPT 桌面版和 Codex CLI 共用一份设置",
      "ChatGPT 桌面版和 Codex CLI 共用一份設定",
    ]);
    expect(copy.map((locale) => locale.subtitleKimiDesktop)).toEqual([
      "Kimi desktop app",
      "Kimi 桌面版",
      "Kimi 桌面版",
    ]);
    expect(copy.map((locale) => locale.subtitleKimiCode)).toEqual([
      "Coding CLI and the Kimi Code app",
      "写代码用的 CLI 和 Kimi Code App",
      "寫程式用的 CLI 和 Kimi Code App",
    ]);
    expect(copy.map((locale) => locale.needsRepair)).toEqual([
      "Needs a quick fix",
      "需要修一下",
      "需要修一下",
    ]);
    expect(copy.map((locale) => locale.restartNameCodex)).toEqual([
      "ChatGPT or Codex",
      "ChatGPT 或 Codex",
      "ChatGPT 或 Codex",
    ]);
    expect(copy.map((locale) => locale.restartToFinish.replace("{{name}}", locale.restartNameCodex))).toEqual([
      "Restart ChatGPT or Codex to finish",
      "重新启动 ChatGPT 或 Codex 即可完成",
      "重新啟動 ChatGPT 或 Codex 即可完成",
    ]);
    expect(copy.map((locale) => locale.showMore)).toEqual([
      "Show {{count}} more",
      "再显示 {{count}} 个",
      "再顯示 {{count}} 個",
    ]);
  });

  it("explains the page-review requirement before rename in every locale", () => {
    expect(supportedAppLocales.map((locale) => resources[locale].translation.pageDetail.renameReviewRequired)).toEqual([
      "Review this page before renaming it.",
      "请先复核此笔记，再进行重命名。",
      "請先覆核此筆記，再進行重新命名。",
    ]);
  });

  it("localizes Settings disclosure labels and the compact telemetry summary", () => {
    expect(supportedAppLocales.map((locale) => resources[locale].translation.settings.detailsLabel)).toEqual([
      "Details",
      "详细说明",
      "詳細說明",
    ]);
    expect(supportedAppLocales.map((locale) => resources[locale].translation.settings.privacyLabel)).toEqual([
      "Privacy",
      "隐私",
      "隱私",
    ]);
    expect(supportedAppLocales.map((locale) => resources[locale].translation.settings.general.telemetrySummary)).toEqual([
      "Usage counts and app details only; never your content.",
      "仅使用次数与应用信息，不含内容",
      "僅使用次數與應用資訊，不含內容",
    ]);
  });

  it("keeps telemetry consent and conditional privacy copy explicit in every locale", () => {
    const telemetry = supportedAppLocales.map(
      (locale) => resources[locale].translation.settings.general,
    );

    expect(telemetry.map((copy) => copy.telemetryTitle)).toEqual([
      "Share optional usage stats",
      "分享可选使用统计",
      "分享選用使用統計",
    ]);
    expect(
      telemetry.every(
        (copy) =>
          copy.telemetryDescription.includes("wenlan.app") &&
          copy.telemetryDescription.includes("ID") &&
          copy.telemetryPending_one.includes("{{count}}"),
      ),
    ).toBe(true);
    expect(telemetry.map((copy) => copy.telemetrySaveFailed)).toEqual([
      "Wenlan could not persist your usage-stats setting. Check the current switch state and retry before restarting Wenlan.",
      "文澜无法持久保存使用统计设置。请确认开关当前状态，并在重启文澜前重试。",
      "文瀾無法持久儲存使用統計設定。請確認開關目前狀態，並在重啟文瀾前重試。",
    ]);
    expect(
      supportedAppLocales.map((locale) => resources[locale].translation.settings.footer),
    ).toEqual([
      "Your memories stay on this machine unless you configure a feature that sends them, such as a cloud model or Remote Access. Optional usage stats are sent only with your permission; normal network metadata may still be visible to services you use.",
      "你的记忆会保留在这台设备上，除非你配置了会发送数据的功能，例如云端模型或远程访问。可选使用统计只有在你许可后才会发送；你使用的服务仍可能看到正常的网络元数据。",
      "你的記憶會保留在這台裝置上，除非你設定了會傳送資料的功能，例如雲端模型或遠端存取。選用使用統計只有在你許可後才會傳送；你使用的服務仍可能看見正常的網路中繼資料。",
    ]);
  });

  it("distinguishes local storage from connected tools and network traffic", () => {
    expect(resources.en.translation.settings.footer).toContain(
      "unless you configure a feature that sends them",
    );
    expect(resources["zh-Hans"].translation.settings.footer).toContain(
      "除非你配置了会发送数据的功能",
    );
    expect(resources["zh-Hant"].translation.settings.footer).toContain(
      "除非你設定了會傳送資料的功能",
    );
  });

  it("describes one editing view without exposing Markdown as a peer mode", () => {
    for (const locale of supportedAppLocales) {
      const editor = resources[locale].translation.pageDetail.editor;
      expect(editor.description).toBeTruthy();
      expect(editor.descriptionFallback).toBeTruthy();
      expect(editor.toolbar).not.toHaveProperty("modeGroup");
      expect(editor.toolbar).not.toHaveProperty("writing");
      expect(editor.toolbar).not.toHaveProperty("markdown");
    }
  });

  it("labels deliberate draft discard in each editor locale", () => {
    expect(supportedAppLocales.map((locale) =>
      resources[locale].translation.pageDetail.editor.discard,
    )).toEqual(["Discard draft", "丢弃草稿", "捨棄草稿"]);
    for (const locale of supportedAppLocales) {
      expect(resources[locale].translation.pageDetail.editor.discardConfirm).toBeTruthy();
    }
  });

  it("keeps the Review fixture boundary explicit in every locale", () => {
    expect([
      resources.en.translation.reviewEnvironment.testData,
      resources["zh-Hans"].translation.reviewEnvironment.testData,
      resources["zh-Hant"].translation.reviewEnvironment.testData,
    ]).toEqual(["TEST DATA", "测试数据", "測試資料"]);
    expect([
      resources.en.translation.reviewEnvironment.fixtureNotice,
      resources["zh-Hans"].translation.reviewEnvironment.fixtureNotice,
      resources["zh-Hant"].translation.reviewEnvironment.fixtureNotice,
    ]).toEqual([
      "Fixture data · resets on relaunch",
      "固定测试数据 · 重新启动后重置",
      "固定測試資料 · 重新啟動後重設",
    ]);
    expect([
      resources.en.translation.reviewEnvironment.reset,
      resources["zh-Hans"].translation.reviewEnvironment.reset,
      resources["zh-Hant"].translation.reviewEnvironment.reset,
    ]).toEqual(["Reset test data", "重置测试数据", "重設測試資料"]);
  });

  it("presents the Pages route as Wiki in every locale", () => {
    expect([
      resources.en.translation.sidebar.pages,
      resources["zh-Hans"].translation.sidebar.pages,
      resources["zh-Hant"].translation.sidebar.pages,
    ]).toEqual(["Wiki", "Wiki", "Wiki"]);
    expect([
      resources.en.translation.pages.overview.title,
      resources["zh-Hans"].translation.pages.overview.title,
      resources["zh-Hant"].translation.pages.overview.title,
    ]).toEqual(["Wiki", "Wiki", "Wiki"]);
  });

  it("lists pages only in the Wiki: no Kind filter, column, or type labels", () => {
    const overviews = [
      resources.en.translation.pages.overview,
      resources["zh-Hans"].translation.pages.overview,
      resources["zh-Hant"].translation.pages.overview,
    ] as Array<Record<string, unknown> & { columns: Record<string, unknown> }>;

    for (const overview of overviews) {
      expect(overview).not.toHaveProperty("typeLabel");
      expect(overview).not.toHaveProperty("typeAll");
      expect(overview).not.toHaveProperty("types");
      expect(overview.columns).not.toHaveProperty("type");
    }
    expect(overviews.map((overview) => Object.keys(overview.columns))).toEqual([
      ["page", "space", "updated"],
      ["page", "space", "updated"],
      ["page", "space", "updated"],
    ]);
  });

  it("defines the direct Page draft lifecycle copy in every locale", () => {
    const editors = [
      resources.en.translation.pages.editor,
      resources["zh-Hans"].translation.pages.editor,
      resources["zh-Hant"].translation.pages.editor,
    ];

    expect(editors.map((editor) => editor.publish)).toEqual([
      "Publish",
      "发布",
      "發佈",
    ]);
    expect(editors.map((editor) => editor.reloadLatest)).toEqual([
      "Reload latest",
      "载入最新版本",
      "載入最新版本",
    ]);
    expect(editors.map((editor) => editor.openExisting)).toEqual([
      "Open existing",
      "打开现有笔记",
      "開啟現有筆記",
    ]);
  });

  it("names the Wiki Space context-link destination in every locale", () => {
    expect([
      resources.en.translation.pages.overview.openSpace,
      resources["zh-Hans"].translation.pages.overview.openSpace,
      resources["zh-Hant"].translation.pages.overview.openSpace,
    ]).toEqual([
      "Open Space: {{space}}",
      "打开空间：{{space}}",
      "開啟空間：{{space}}",
    ]);
  });

  it("keeps repair controls in the selected Chinese writing system", () => {
    expect(resources["zh-Hans"].translation.sourceRepair.prepareChange).toBe("准备变更");
    expect(resources["zh-Hant"].translation.sourceRepair.prepareChange).toBe("準備變更");
    expect(resources["zh-Hans"].translation.sourceRepair.extractionHint).toContain("实体提取");
    expect(resources["zh-Hant"].translation.sourceRepair.extractionHint).toContain("實體擷取");
  });

  it("localizes the recovery retry copy in every supported locale", () => {
    expect(supportedAppLocales.map((locale) => resources[locale].translation.sourceRepair.recoveryFailed)).toEqual([
      "The pending repair could not be recovered. No new change will be prepared.",
      "无法恢复待处理的修复，不会准备新的变更。",
      "無法復原待處理的修復，不會準備新的變更。",
    ]);
    expect(supportedAppLocales.map((locale) => resources[locale].translation.sourceRepair.retryRecovery)).toEqual([
      "Retry recovery",
      "重试恢复",
      "重試復原",
    ]);
  });

  it("keeps the setup check memory findable and inside the daemon's duplicate window in every locale", () => {
    // The daemon rejects a store whose first 200 characters prefix-match an
    // existing memory, so the per-run nonce must land inside that window or
    // a second setup run fails as a duplicate.
    const nonce = "00000000-0000-4000-8000-000000000000";
    for (const locale of supportedAppLocales) {
      const copy = resources[locale].translation;
      const probe = copy.setup.settingUp.probeMemory.replace("{{nonce}}", nonce);
      expect(Array.from(probe).length).toBeLessThanOrEqual(200);
      expect(probe.endsWith(`(${nonce})`)).toBe(true);
      // The cleanup warning tells people what to search for; it has to be
      // the words the leftover memory opens with.
      expect(probe.startsWith(copy.connections.internalProbe)).toBe(true);
      expect(copy.setup.settingUp.daemonCleanupWarning).toContain(copy.connections.internalProbe);
    }
  });

  it("keeps Simplified and Traditional Chinese key sets in parity with English", () => {
    const englishKeys = flattenKeys(resources.en.translation).sort();

    expect(flattenKeys(resources["zh-Hans"].translation).sort()).toEqual(
      englishKeys,
    );
    expect(flattenKeys(resources["zh-Hant"].translation).sort()).toEqual(
      englishKeys,
    );
  });

  it("defines the complete Page dateline vocabulary in every locale", () => {
    const datelines = [
      resources.en.translation.pageDetail.dateline,
      resources["zh-Hans"].translation.pageDetail.dateline,
      resources["zh-Hant"].translation.pageDetail.dateline,
    ];

    expect(datelines).toEqual([
      {
        lastDistilled: "Last organized {{time}}",
        lastUpdated: "Last updated {{time}}",
        sourceMemories_one: "from {{count}} memory",
        sourceMemories_other: "from {{count}} memories",
        relativeJustNow: "just now",
        relativeMinutesAgo: "{{count}}m ago",
        relativeHoursAgo: "{{count}}h ago",
        relativeDaysAgo: "{{count}}d ago",
        needsReview: "needs review",
        updating: "updating...",
        updateBlocked: "update blocked: citations couldn't be verified",
      },
      {
        lastDistilled: "上次整理：{{time}}",
        lastUpdated: "上次更新：{{time}}",
        sourceMemories_one: "来自 {{count}} 条记忆",
        sourceMemories_other: "来自 {{count}} 条记忆",
        relativeJustNow: "刚刚",
        relativeMinutesAgo: "{{count}} 分钟前",
        relativeHoursAgo: "{{count}} 小时前",
        relativeDaysAgo: "{{count}} 天前",
        needsReview: "需要审核",
        updating: "正在更新…",
        updateBlocked: "更新受阻：引用未通过验证",
      },
      {
        lastDistilled: "上次整理：{{time}}",
        lastUpdated: "上次更新：{{time}}",
        sourceMemories_one: "來自 {{count}} 則記憶",
        sourceMemories_other: "來自 {{count}} 則記憶",
        relativeJustNow: "剛剛",
        relativeMinutesAgo: "{{count}} 分鐘前",
        relativeHoursAgo: "{{count}} 小時前",
        relativeDaysAgo: "{{count}} 天前",
        needsReview: "需要審核",
        updating: "正在更新…",
        updateBlocked: "更新受阻：引用未通過驗證",
      },
    ]);
  });

  it("defines Spaces navigation, overview, and dossier copy in every locale", () => {
    expect([
      resources.en.translation.sidebar.recentSpaces,
      resources["zh-Hans"].translation.sidebar.recentSpaces,
      resources["zh-Hant"].translation.sidebar.recentSpaces,
    ]).toEqual(["Recent spaces", "最近空间", "最近空間"]);
    expect([
      resources.en.translation.spaces.overview.newSpace,
      resources["zh-Hans"].translation.spaces.overview.newSpace,
      resources["zh-Hant"].translation.spaces.overview.newSpace,
    ]).toEqual(["New", "新增", "新增"]);
    expect([
      resources.en.translation.spaceDetail.backToSpaces,
      resources["zh-Hans"].translation.spaceDetail.backToSpaces,
      resources["zh-Hant"].translation.spaceDetail.backToSpaces,
    ]).toEqual(["Spaces", "返回空间", "返回空間"]);
  });

  it("uses the approved inventory heading in every locale", () => {
    // Given the existing confirmed inventory heading key
    const headings = [
      resources.en.translation.spaces.overview.confirmedHeading,
      resources["zh-Hans"].translation.spaces.overview.confirmedHeading,
      resources["zh-Hant"].translation.spaces.overview.confirmedHeading,
    ];

    // When the three supported locale values are read
    // Then they match the approved All spaces copy
    expect(headings).toEqual(["All spaces", "所有空间", "所有空間"]);
  });

  it("defines Home overview copy without the user-facing Index label", () => {
    // Given the three supported Home resource groups
    const homeResources = [
      resources.en.translation.home,
      resources["zh-Hans"].translation.home,
      resources["zh-Hant"].translation.home,
    ];

    // When their overview labels are read
    // Then every locale has meaningful Home copy and no legacy Index key
    expect(homeResources.map((home) => home.overview)).toEqual([
      "Home overview",
      "首页概览",
      "首頁概覽",
    ]);
    expect(homeResources.every((home) => !("index" in home))).toBe(true);
  });

  it("localizes all Spaces overview metadata labels", () => {
    // Given the desktop and mobile overview labels share one resource contract
    const overviewResources = [
      resources.en.translation.spaces.overview,
      resources["zh-Hans"].translation.spaces.overview,
      resources["zh-Hant"].translation.spaces.overview,
    ];

    // When Pages, Memories, and Updated are read
    // Then all supported locales provide the approved copy
    expect(overviewResources.map((overview) => overview.pages)).toEqual([
      "Pages",
      "笔记",
      "筆記",
    ]);
    expect(overviewResources.map((overview) => overview.memories)).toEqual([
      "Memories",
      "记忆",
      "記憶",
    ]);
    expect(overviewResources.map((overview) => overview.updated)).toEqual([
      "Updated",
      "更新",
      "更新",
    ]);
  });
});

// Standing rule: never announce that Wenlan itself is a plugin. The only
// exceptions are real integration options, CLI commands/slash-commands, and
// provider menu names. Wenlan itself remains an app, not a plugin.
function flattenStringEntries(
  value: unknown,
  prefix = "",
): Array<[string, string]> {
  if (typeof value === "string") return [[prefix, value]];
  if (!value || typeof value !== "object") return [];
  return Object.entries(value as Record<string, unknown>).flatMap(
    ([key, child]) =>
      flattenStringEntries(child, prefix ? `${prefix}.${key}` : key),
  );
}

const PLUGIN_WORD_ALLOWLIST = new Set([
  "intelligence.synthesisBetaHelp", // Wenlan's actual plugin/MCP integration, not the app's identity
  "connectMatrix.claudeCodeCommand1",
  "connectMatrix.claudeCodeCommand2",
  "connectMatrix.claudeCodeReload",
  "connectMatrix.claudeCodePrompt", // real `/plugin` menu + `claude plugin` CLI commands
  "connectMatrix.claudePluginStep1",
  "connectMatrix.claudePluginStep2",
  "connectMatrix.claudePluginStep3",
]);
const BANNED_PLUGIN_WORDS = ["plugin", "插件", "外掛"];

describe("banned self-referential 'plugin' copy", () => {
  it.each(supportedAppLocales)(
    "%s never describes Wenlan itself as a plugin",
    (locale) => {
      const entries = flattenStringEntries(resources[locale].translation);
      const offenders = entries.filter(
        ([key, value]) =>
          !PLUGIN_WORD_ALLOWLIST.has(key) &&
          BANNED_PLUGIN_WORDS.some((word) => value.includes(word)),
      );
      expect(offenders).toEqual([]);
    },
  );
});
