// SPDX-License-Identifier: AGPL-3.0-only

/**
 * How a bare web address reads in page text: the site and path, without the
 * scheme, a leading "www." or a trailing slash. The link's arrow already says
 * "this opens the browser", so "https://" adds length and no meaning.
 */
export function displayUrl(url: string): string {
  const shown = url
    .trim()
    .replace(/^https?:\/\//i, "")
    .replace(/^www\./i, "")
    .replace(/\/$/, "");
  return shown || url;
}

/** Length of the prefix that `displayUrl` hides ("https://", "www."). */
export function hiddenUrlPrefixLength(url: string): number {
  const match = url.match(/^(https?:\/\/)(www\.)?/i);
  return match ? match[0].length : 0;
}

/**
 * True when a link's visible text is just its own address, as with a bare
 * URL that Markdown turned into a link. Labelled links keep their label.
 */
export function isBareUrlLink(href: string | undefined, text: unknown): text is string {
  if (!href || typeof text !== "string") return false;
  if (!/^https?:\/\//i.test(href)) return false;
  const trimmed = text.trim();
  return trimmed === href || `http://${trimmed}` === href || `https://${trimmed}` === href;
}
