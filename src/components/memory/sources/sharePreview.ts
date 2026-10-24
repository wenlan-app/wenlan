// SPDX-License-Identifier: AGPL-3.0-only
import type { SourceApp } from "./sourcePresentation";

/** Sites that show their posts to link previews without a sign-in. Mail and
 *  chat stay private: their links are never fetched. */
export const SHARE_PREVIEW_APPS: ReadonlySet<SourceApp> = new Set(["x", "threads", "instagram", "xiaohongshu"]);

const SITE_NAMES = new Set(["x", "twitter", "threads", "instagram", "xiaohongshu", "rednote", "小红书", "小紅書"]);
// A wall's whole title, not a post that mentions signing in.
const WALL = /^(log ?in|sign ?in|sign ?up|page not found|something went wrong|error)\b|^[^\s]{0,6}(登录|登入|註冊|注册)|页面不见了|頁面不見了/i;

/** The words a social site gives link previews (iMessage, Slack) for a post,
 *  or null when it served a sign-in or error page instead. */
export function sharePreviewText(title: string): string | null {
  const text = title.replace(/\s+/g, " ").trim()
    // "… / X" and "… • Instagram photos and videos" name only the site.
    .replace(/\s*[/|•·-]\s*(X|Twitter|Threads|Instagram)(\s+photos and videos)?$/i, "")
    .replace(/\s*-\s*小[红紅]书$/, "")
    .trim();
  if (!text || SITE_NAMES.has(text.toLowerCase()) || WALL.test(text)) return null;
  return text.length > 100 ? `${text.slice(0, 99).trimEnd()}…` : text;
}
