// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { postEmbedDocument, sourceEmbed } from "./sourceEmbed";

describe("what a saved link's card can show from its site", () => {
  it("finds a YouTube video however its link is written", () => {
    for (const url of [
      "https://www.youtube.com/watch?v=msVuCEs8Ydo&list=PL1",
      "https://youtu.be/msVuCEs8Ydo?si=abc",
      "https://m.youtube.com/shorts/msVuCEs8Ydo",
      "https://www.youtube.com/live/msVuCEs8Ydo",
    ]) expect(sourceEmbed(url)).toEqual({ kind: "video", id: "msVuCEs8Ydo" });
    expect(sourceEmbed("https://www.youtube.com/@veritasium")).toBeNull();
    expect(sourceEmbed("https://www.youtube.com/watch?v=short")).toBeNull();
  });

  it("finds a post on X, Threads and Instagram, and nothing else", () => {
    expect(sourceEmbed("https://x.com/jack/status/20?s=46")).toEqual({ kind: "post", app: "x", permalink: "https://twitter.com/jack/status/20" });
    expect(sourceEmbed("https://www.threads.com/@zuck/post/DAbc_1-x?xmt=1")).toEqual({ kind: "post", app: "threads", permalink: "https://www.threads.com/@zuck/post/DAbc_1-x" });
    expect(sourceEmbed("https://www.instagram.com/reels/C1x2y3/?igsh=1")).toEqual({ kind: "post", app: "instagram", permalink: "https://www.instagram.com/reel/C1x2y3/" });
    expect(sourceEmbed("https://x.com/jack")).toBeNull();
    expect(sourceEmbed("https://www.instagram.com/natgeo/")).toBeNull();
    expect(sourceEmbed("https://www.xiaohongshu.com/explore/123")).toBeNull();
    expect(sourceEmbed("https://en.wikipedia.org/wiki/Local-first_software")).toBeNull();
  });

  it("puts the site's own embed code in the frame, and nothing it could break out of", () => {
    const threads = sourceEmbed("https://www.threads.com/@zuck/post/DAbc")!;
    if (threads.kind !== "post") throw new Error("not a post");
    const html = postEmbedDocument(threads, "frame-1", false);
    expect(html).toContain('data-text-post-permalink="https://www.threads.com/@zuck/post/DAbc"');
    expect(html).toContain('src="https://www.threads.com/embed.js"');
    expect(html).toContain('var id="frame-1"');
  });
});
