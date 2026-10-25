// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { sourceLinkKey } from "./sourceLinkKey";

describe("which page a link opens", () => {
  it("ignores tracking, the fragment, www. and a trailing slash", () => {
    expect(sourceLinkKey("https://www.example.com/post/?utm_source=x&fbclid=1#top")).toBe(sourceLinkKey("https://example.com/post"));
    expect(sourceLinkKey("https://x.com/jack/status/20?s=20&t=abc")).toBe(sourceLinkKey("https://x.com/jack/status/20"));
  });

  it("keeps the query that names the page", () => {
    expect(sourceLinkKey("https://www.youtube.com/watch?v=dQw4w9WgXcQ")).not.toBe(sourceLinkKey("https://www.youtube.com/watch?v=jNQXAC9IVRw"));
    expect(sourceLinkKey("https://news.ycombinator.com/item?id=1")).not.toBe(sourceLinkKey("https://news.ycombinator.com/item?id=2"));
  });

  it("treats a YouTube short or mobile link as its watch page", () => {
    const watch = sourceLinkKey("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    expect(sourceLinkKey("https://youtu.be/dQw4w9WgXcQ?si=abc")).toBe(watch);
    expect(sourceLinkKey("https://m.youtube.com/watch?v=dQw4w9WgXcQ&feature=share")).toBe(watch);
    expect(sourceLinkKey("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=RDdQw4w9WgXcQ&index=2")).toBe(watch);
  });
});
