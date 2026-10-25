// SPDX-License-Identifier: AGPL-3.0-only

// Added by sharing and ad tracking; they never change which page a link opens.
const TRACKING = /^(utm_.*|fbclid|gclid|dclid|gbraid|wbraid|msclkid|mc_cid|mc_eid|igsh|igshid|si|feature|ref|ref_src|ref_url|s|t|xsec_token|xsec_source|share_id|share_from|app_platform|app_version|_ga|spm)$/i;

/** A link's site, path and meaningful query, so tracking parameters, the
 *  fragment, `www.` or a trailing slash do not make another page, while
 *  `watch?v=` or `item?id=` still tell pages apart. */
export function sourceLinkKey(link: string): string {
  try {
    const url = new URL(link);
    let host = url.hostname.replace(/^(www|m)\./, "");
    let path = url.pathname.replace(/\/+$/, "");
    const query = new URLSearchParams(url.search);
    // A YouTube short link is the same video as its watch page.
    if (host === "youtu.be" && path.length > 1) {
      query.set("v", path.slice(1));
      [host, path] = ["youtube.com", "/watch"];
    }
    // A video is one page whichever playlist it was opened from.
    const video = host === "youtube.com" && path === "/watch";
    const kept = [...query]
      .filter(([name]) => (video ? name === "v" : !TRACKING.test(name)))
      .sort(([a], [b]) => a.localeCompare(b));
    const search = kept.length ? `?${new URLSearchParams(kept).toString()}` : "";
    return `${host}${path}${search}`;
  } catch {
    return link;
  }
}
