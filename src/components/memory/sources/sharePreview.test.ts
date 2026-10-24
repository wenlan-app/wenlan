// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { sharePreviewText } from "./sharePreview";

describe("a social link's preview text", () => {
  it("keeps what the site shows link previews", () => {
    expect(sharePreviewText('jack on X: "just setting up my twttr" / X')).toBe('jack on X: "just setting up my twttr"');
    expect(sharePreviewText("Zuck (@zuck) • Threads, Say more")).toBe("Zuck (@zuck) • Threads, Say more");
    expect(sharePreviewText("东京三日游攻略 - 小红书")).toBe("东京三日游攻略");
    expect(sharePreviewText('ana on X: "Why does every app make me log in twice?"')).toBe('ana on X: "Why does every app make me log in twice?"');
  });

  it("is nothing for a sign-in or missing page", () => {
    for (const title of ["Log in • Instagram", "X", "Instagram", "小红书", "你访问的页面不见了", "Page not found • Instagram", "Sign up for X"]) {
      expect(sharePreviewText(title)).toBeNull();
    }
  });

  it("shortens a long post", () => {
    const text = sharePreviewText(`someone on X: "${"a".repeat(200)}"`)!;
    expect(text.length).toBe(100);
    expect(text.endsWith("…")).toBe(true);
  });
});
