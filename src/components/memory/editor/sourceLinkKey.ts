// SPDX-License-Identifier: AGPL-3.0-only

/** A link's site and path, so an edited query, fragment or trailing slash is the same page. */
export function sourceLinkKey(link: string): string {
  try {
    const url = new URL(link);
    return `${url.hostname.replace(/^www\./, "")}${url.pathname.replace(/\/+$/, "")}`;
  } catch {
    return link;
  }
}
