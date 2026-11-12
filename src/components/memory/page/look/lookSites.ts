// SPDX-License-Identifier: AGPL-3.0-only
//
// The sites a look may reach (UX-LOOK-9). A look names them in its HTML:
//
//   <script type="application/json" id="wenlan-sites">["api.open-meteo.com"]</script>
//
// The look runs with no network until the person allows those sites once for
// the page. Allowed sites go into the frame's policy as `https://host`, so
// only that host, over https, on its usual port, is reachable; anything not
// named stays blocked. Never a local address: a look cannot reach this
// computer or its network.

/** Most sites one look may name. */
export const LOOK_SITES_MAX = 5;

const HOST = /^(?=.{4,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const LOCAL_SUFFIXES = [".local", ".localhost", ".internal", ".lan", ".home", ".home.arpa", ".corp", ".test", ".invalid", ".example"];
const DECLARATION = /<script\b[^>]*\bid\s*=\s*["']?wenlan-sites["']?[^>]*>([\s\S]*?)<\/script\s*>/i;

/** A site as the policy will name it, or null when it is not one a look may reach. */
export function normalizeSite(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  let host = raw.trim().toLowerCase();
  if (host.startsWith("https://")) host = host.slice("https://".length);
  if (host.endsWith("/")) host = host.slice(0, -1);
  if (!HOST.test(host)) return null;
  if (LOCAL_SUFFIXES.some((suffix) => host.endsWith(suffix))) return null;
  return host;
}

/** The sites `html` names, each once, at most {@link LOOK_SITES_MAX}; [] when none or unreadable. */
export function declaredSites(html: string): string[] {
  const match = DECLARATION.exec(html);
  if (!match) return [];
  let list: unknown;
  try { list = JSON.parse(match[1]); } catch { return []; }
  if (!Array.isArray(list)) return [];
  const sites: string[] = [];
  for (const raw of list) {
    const site = normalizeSite(raw);
    if (site && !sites.includes(site)) sites.push(site);
    if (sites.length === LOOK_SITES_MAX) break;
  }
  return sites;
}

const storageKey = (pageId: string) => `wenlan-look-sites:${pageId}`;

/** Sites the person allowed for this page's look. */
export function allowedSites(pageId: string): string[] {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(storageKey(pageId)) ?? "[]");
    return Array.isArray(value) ? value.map(normalizeSite).filter((site): site is string => site !== null) : [];
  } catch {
    return [];
  }
}

/** Remember that the person allowed `sites` for this page's look. */
export function allowSites(pageId: string, sites: readonly string[]): void {
  const next = [...new Set([...allowedSites(pageId), ...sites.map(normalizeSite).filter((s): s is string => s !== null)])];
  try { window.localStorage.setItem(storageKey(pageId), JSON.stringify(next)); } catch { /* asked again next time */ }
}
