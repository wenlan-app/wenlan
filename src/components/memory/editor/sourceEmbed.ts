// SPDX-License-Identifier: AGPL-3.0-only
import { getSourceAppFromUrl, sourceHandleFromUrl } from "../sources/sourcePresentation";

/** What a saved link's card can show from the site itself: a YouTube video
 *  to play in place, or a post through the site's own embed. */
export type SourceEmbed =
  | { kind: "video"; id: string }
  | { kind: "post"; app: "x" | "threads" | "instagram"; permalink: string };

const VIDEO_ID = /^[A-Za-z0-9_-]{11}$/;

export function sourceEmbed(value: string): SourceEmbed | null {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  const app = getSourceAppFromUrl(url);
  const parts = url.pathname.split("/").filter(Boolean);
  if (app === "youtube") {
    const id = url.hostname === "youtu.be"
      ? parts[0]
      : parts[0] === "watch" ? url.searchParams.get("v") : ["shorts", "live", "embed"].includes(parts[0] ?? "") ? parts[1] : null;
    return id && VIDEO_ID.test(id) ? { kind: "video", id } : null;
  }
  if (app === "x") {
    const at = parts.indexOf("status");
    const id = at >= 0 ? parts[at + 1] : undefined;
    if (!id || !/^\d{1,25}$/.test(id)) return null;
    return { kind: "post", app, permalink: `https://twitter.com/${sourceHandleFromUrl(value) ?? "i"}/status/${id}` };
  }
  if (app === "threads") {
    const handle = sourceHandleFromUrl(value);
    const code = parts[1] === "post" ? parts[2] : undefined;
    if (!handle || !code || !/^[A-Za-z0-9_-]{1,40}$/.test(code)) return null;
    return { kind: "post", app, permalink: `https://www.threads.com/@${handle}/post/${code}` };
  }
  if (app === "instagram") {
    const kind = parts[0] === "reels" ? "reel" : parts[0];
    const code = parts[1];
    if (!["p", "reel", "tv"].includes(kind ?? "") || !code || !/^[A-Za-z0-9_-]{1,40}$/.test(code)) return null;
    return { kind: "post", app, permalink: `https://www.instagram.com/${kind}/${code}/` };
  }
  return null;
}

/** YouTube's own cover image for a video. */
export function videoCoverUrl(id: string): string {
  return `https://i.ytimg.com/vi/${id}/hqdefault.jpg`;
}

/** Wenlan's page on its website that holds YouTube's player. YouTube's
 *  embedded player needs a web page address as the referrer, which the app's
 *  own window (tauri://localhost) does not have, and shows Error 153 without
 *  it. A test build can point at another copy of the page. */
const VIDEO_PLAYER_PAGE = import.meta.env.VITE_WENLAN_VIDEO_PLAYER_PAGE || "https://wenlan.app/embed/youtube.html";

/** The player for a video: YouTube's privacy-enhanced player, inside
 *  Wenlan's player page. The video id goes after the #, so it never reaches
 *  the website's server. */
export function videoPlayerUrl(id: string): string {
  return `${VIDEO_PLAYER_PAGE}#${id}`;
}

/** The site's own embed page for a post: the page its embed script puts in
 *  a frame, loaded straight away so it keeps its own origin and storage. */
export function postEmbedUrl(embed: Extract<SourceEmbed, { kind: "post" }>, dark: boolean): string {
  if (embed.app === "x") {
    const id = embed.permalink.split("/").pop() ?? "";
    return `https://platform.twitter.com/embed/Tweet.html?id=${id}&dnt=true&theme=${dark ? "dark" : "light"}`;
  }
  if (embed.app === "threads") return `${embed.permalink}/embed`;
  return `${embed.permalink}embed/captioned/`;
}

const EMBED_ORIGINS: Readonly<Record<Extract<SourceEmbed, { kind: "post" }>["app"], readonly string[]>> = {
  x: ["https://platform.twitter.com"],
  threads: ["https://www.threads.com", "https://www.threads.net"],
  instagram: ["https://www.instagram.com"],
};

/** Whether a message comes from the site whose post is embedded. */
export function postEmbedOrigin(embed: Extract<SourceEmbed, { kind: "post" }>, origin: string): boolean {
  return EMBED_ORIGINS[embed.app].includes(origin);
}

/** The height an embed page asks for in its own resize message, such as X's
 *  `twttr.private.resize`, Instagram's `MEASURE` or Threads' bare number;
 *  null for anything else. */
export function postEmbedHeight(data: unknown): number | null {
  if (typeof data === "number") return Number.isFinite(data) ? data : null;
  if (typeof data === "string" && /^\d+(\.\d+)?$/.test(data.trim())) return Number.parseFloat(data);
  let value = data;
  if (typeof value === "string") {
    if (!value.includes("height")) return null;
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  const find = (node: unknown, depth: number): number | null => {
    if (!node || typeof node !== "object" || depth > 5) return null;
    const record = node as Record<string, unknown>;
    const height = record.height;
    if (typeof height === "number" && Number.isFinite(height)) return height;
    if (typeof height === "string" && /^\d+(\.\d+)?(px)?$/.test(height)) return Number.parseFloat(height);
    for (const child of Object.values(record)) {
      const found = find(child, depth + 1);
      if (found !== null) return found;
    }
    return null;
  };
  return find(value, 0);
}
