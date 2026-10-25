// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { postEmbedHeight, postEmbedOrigin, postEmbedUrl, sourceEmbed } from "./sourceEmbed";

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

  it("loads each site's own embed page, and listens only to that site", () => {
    const post = (url: string) => {
      const embed = sourceEmbed(url);
      if (embed?.kind !== "post") throw new Error("not a post");
      return embed;
    };
    expect(postEmbedUrl(post("https://x.com/jack/status/20"), true)).toBe("https://platform.twitter.com/embed/Tweet.html?id=20&dnt=true&theme=dark");
    expect(postEmbedUrl(post("https://www.threads.net/@zuck/post/DAbc"), false)).toBe("https://www.threads.com/@zuck/post/DAbc/embed");
    expect(postEmbedUrl(post("https://www.instagram.com/p/C1x2y3/"), false)).toBe("https://www.instagram.com/p/C1x2y3/embed/captioned/");
    expect(postEmbedOrigin(post("https://x.com/jack/status/20"), "https://platform.twitter.com")).toBe(true);
    expect(postEmbedOrigin(post("https://x.com/jack/status/20"), "https://www.instagram.com")).toBe(false);
  });

  it("reads the height an embed page asks for", () => {
    expect(postEmbedHeight({ "twttr.embed": { method: "twttr.private.resize", params: [{ height: 412, width: 550 }] } })).toBe(412);
    expect(postEmbedHeight(JSON.stringify({ type: "MEASURE", details: { height: 655 } }))).toBe(655);
    expect(postEmbedHeight(218)).toBe(218);
    expect(postEmbedHeight("226")).toBe(226);
    expect(postEmbedHeight(Number.NaN)).toBeNull();
    expect(postEmbedHeight("not json, no size")).toBeNull();
    expect(postEmbedHeight({ type: "ready" })).toBeNull();
  });
});
