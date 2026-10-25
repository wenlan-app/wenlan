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

/** YouTube's embedded player in privacy-enhanced mode: no cookies until the
 *  video plays. */
export function videoPlayerUrl(id: string): string {
  return `https://www.youtube-nocookie.com/embed/${id}?autoplay=1&rel=0`;
}

function attribute(value: string): string {
  return value.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
}

/** The site's own embed code, as its share menu gives it. */
function embedCode(embed: Extract<SourceEmbed, { kind: "post" }>, dark: boolean): { quote: string; script: string } {
  const link = attribute(embed.permalink);
  if (embed.app === "x") {
    return {
      quote: `<blockquote class="twitter-tweet" data-dnt="true" data-theme="${dark ? "dark" : "light"}"><a href="${link}"></a></blockquote>`,
      script: "https://platform.twitter.com/widgets.js",
    };
  }
  if (embed.app === "threads") {
    return {
      quote: `<blockquote class="text-post-media" data-text-post-permalink="${link}" data-text-post-version="0"><a href="${link}"></a></blockquote>`,
      script: "https://www.threads.com/embed.js",
    };
  }
  return {
    quote: `<blockquote class="instagram-media" data-instgrm-captioned data-instgrm-permalink="${link}" data-instgrm-version="14" style="margin:0;min-width:0;width:100%;max-width:540px"><a href="${link}"></a></blockquote>`,
    script: "https://www.instagram.com/embed.js",
  };
}

/** A document holding the site's embed code, for a sandboxed frame. It tells
 *  the page its height, and whether the post showed (the site's script drew
 *  its frame) or failed (the script did not load, or drew nothing). */
export function postEmbedDocument(embed: Extract<SourceEmbed, { kind: "post" }>, frame: string, dark: boolean): string {
  const { quote, script } = embedCode(embed, dark);
  const id = JSON.stringify(frame);
  return `<!doctype html><html><head><meta charset="utf-8"><meta name="referrer" content="strict-origin-when-cross-origin">
<style>html,body{margin:0;padding:0;background:transparent;overflow:hidden}body>*{margin-left:auto!important;margin-right:auto!important}</style></head>
<body>${quote}<script async src="${script}" onerror="parent.postMessage({wenlanEmbed:${attribute(id)},failed:true},'*')"></script>
<script>(function(){var id=${id},shown=false,done=false,start=Date.now(),timer;
function tell(m){m.wenlanEmbed=id;parent.postMessage(m,'*')}
function check(){if(done)return;var f=document.querySelector('iframe');var h=f?f.getBoundingClientRect().height:0;
if(!shown&&f&&h>80){shown=true;clearInterval(timer);tell({shown:true})}
if(shown)tell({height:Math.ceil(document.body.getBoundingClientRect().height)});
else if(Date.now()-start>15000){done=true;clearInterval(timer);tell({failed:true})}}
new ResizeObserver(check).observe(document.body);timer=setInterval(check,500)})();</script>
</body></html>`;
}
