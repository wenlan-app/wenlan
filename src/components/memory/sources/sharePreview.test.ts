// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { sharePreview, sharePreviewTitle } from "./sharePreview";

const page = (url: string, title: string, description?: string, final_url = url) => ({ url, final_url, title, description });

describe("a social link's preview text", () => {
  it("keeps who wrote a post and its words, as link previews show them", () => {
    expect(sharePreview("x", page("https://x.com/jack/status/20", 'jack on X: "just setting up my twttr" / X'))).toEqual({ who: "@jack", text: "just setting up my twttr" });
    expect(sharePreview("x", page("https://x.com/i/web/status/20", "jack (@jack) on X", "just setting up my twttr"))).toEqual({ who: "@jack", text: "just setting up my twttr" });
    expect(sharePreview("threads", page(
      "https://www.threads.com/@zuck/post/DMu508TRUyp",
      "Mark Zuckerberg (@zuck) on Threads",
      "We're building personal superintelligence for everyone.",
    ))).toEqual({ who: "@zuck", text: "We're building personal superintelligence for everyone." });
    expect(sharePreview("instagram", page(
      "https://www.instagram.com/p/BsOGulcndj-/",
      'Just An Egg 🥚 on Instagram: "Let\'s set a world record together and get the most liked post on Instag…"',
      '60M likes, 4M comments - world_record_egg on January 4, 2019: "Let\'s set a world record together and get the most liked post on Instagram.".',
    ))).toEqual({ who: "@world_record_egg", text: "Let's set a world record together and get the most liked post on Instagram." });
    expect(sharePreview("xiaohongshu", page("https://www.xiaohongshu.com/explore/1", "东京三日游攻略 - 小红书"))).toEqual({ who: null, text: "东京三日游攻略" });
    // Threads served to Wenlan titles a post with its words.
    expect(sharePreview("threads", page("https://www.threads.com/@zuck/post/DMu508TRUyp", "We're building personal superintelligence for everyone."))).toEqual({ who: "@zuck", text: "We're building personal superintelligence for everyone." });
  });

  it("keeps a post that mentions signing in", () => {
    expect(sharePreview("x", page("https://x.com/ana/status/1", 'ana on X: "Why does every app make me log in twice?" / X'))?.text).toBe("Why does every app make me log in twice?");
  });

  it("is nothing for a sign-in or missing page", () => {
    for (const title of ["Log in • Instagram", "X", "Instagram", "小红书", "你访问的页面不见了", "Page not found • Instagram", "Sign up for X", "小红书 - 你的生活兴趣社区"]) {
      expect(sharePreview("instagram", page("https://www.instagram.com/p/1/", title))).toBeNull();
    }
    expect(sharePreview("threads", page("https://www.threads.com/@a/post/1", "Threads"))).toBeNull();
    expect(sharePreview("xiaohongshu", page("https://www.xiaohongshu.com/discovery/item/1", "东京三日游攻略 - 小红书", undefined, "https://www.xiaohongshu.com/404/sec_abc"))).toBeNull();
    expect(sharePreview("xiaohongshu", page("https://xhslink.com/a", "小红书", "登录后查看更多", "https://www.xiaohongshu.com/login"))).toBeNull();
  });

  it("shortens a long post for its link", () => {
    const title = sharePreviewTitle("a".repeat(200));
    expect(title.length).toBe(100);
    expect(title.endsWith("…")).toBe(true);
    expect(sharePreviewTitle("short")).toBe("short");
  });
});
