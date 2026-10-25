// SPDX-License-Identifier: AGPL-3.0-only
import { sourceHandleFromUrl, type SourceApp } from "./sourcePresentation";

/** Sites that show their posts to link previews without a sign-in. Mail and
 *  chat stay private: their links are never fetched. */
export const SHARE_PREVIEW_APPS: ReadonlySet<SourceApp> = new Set(["x", "threads", "instagram", "xiaohongshu"]);

/** What a post shows in a link preview (iMessage, Slack): who wrote it, and
 *  its words. */
export interface SharePreview {
  who: string | null;
  text: string;
}

interface FetchedPage {
  url: string;
  final_url: string;
  title: string;
  description?: string | null;
}

const SITE_NAMES = new Set(["x", "twitter", "threads", "instagram", "xiaohongshu", "rednote", "小红书", "小紅書"]);
// A wall's whole text, not a post that mentions signing in.
const WALL = /^(log ?in|sign ?in|sign ?up|page not found|something went wrong|error)\b|^[^\s]{0,6}(登录|登入|註冊|注册)|页面不见了|頁面不見了|你的生活(兴趣社区|指南)/i;
// Where these sites send a post they will not show.
const WALL_PATHS = /^\/(404|login|website-login|accounts\/login|i\/flow\/login)(\/|$)/;

function tidy(text: string | null | undefined): string {
  return (text ?? "").replace(/\s+/g, " ").trim();
}

function unquote(text: string): string {
  return text.replace(/^["“]/, "").replace(/["”]$/, "").trim();
}

/** The words and author a social site gives link previews for a post, or
 *  null when it served a sign-in or error page instead. */
export function sharePreview(app: SourceApp, page: FetchedPage): SharePreview | null {
  try {
    if (WALL_PATHS.test(new URL(page.final_url).pathname)) return null;
  } catch {
    return null;
  }
  const title = tidy(page.title);
  const description = tidy(page.description);
  const handle = sourceHandleFromUrl(page.url);
  let who: string | null = null;
  let text = "";
  let match: RegExpExecArray | null;
  if (app === "x" && (match = /^(.+?) on X: "(.+)"(?:\s*\/\s*X)?$/.exec(title))) {
    [who, text] = [match[1], match[2]];
  } else if (app === "instagram" && (match = /^[\d.,]+[KMB]? likes?, [\d.,]+[KMB]? comments? - ([\w.]+) on [^:]+: (.+)$/i.exec(description))) {
    [who, text] = [`@${match[1]}`, match[2]];
  } else if (app === "instagram" && (match = /^(.+?) on Instagram: (.+)$/.exec(title))) {
    [who, text] = [match[1], match[2]];
  } else if ((match = /^(.+?) \(@([\w.]+)\) on (X|Threads)$/.exec(title))) {
    [who, text] = [`@${match[2]}`, description];
  } else {
    // "… / X" and "… • Instagram photos and videos" name only the site.
    text = title
      .replace(/\s*[/|•·-]\s*(X|Twitter|Threads|Instagram)(\s+photos and videos)?$/i, "")
      .replace(/\s*-\s*小[红紅]书$/, "")
      .trim();
    if (SITE_NAMES.has(text.toLowerCase())) text = description;
    // Threads can title a post with its words alone; the address names who wrote it.
    if (handle) who = `@${handle}`;
  }
  text = unquote(text.replace(/"\.$/, "\""));
  if (!text || SITE_NAMES.has(text.toLowerCase()) || WALL.test(text)) return null;
  if (handle && who && !who.startsWith("@")) who = `@${handle}`;
  return { who, text };
}

/** A preview's words, short enough for a link. */
export function sharePreviewTitle(text: string): string {
  return text.length > 100 ? `${text.slice(0, 99).trimEnd()}…` : text;
}
