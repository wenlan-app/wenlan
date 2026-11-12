// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it } from "vitest";
import { buildLookDocument, DEFAULT_LOOK_THEMES, LOOK_CSP, lookCsp } from "./lookDocument";
import { allowedSites, allowSites, declaredSites, LOOK_SITES_MAX, normalizeSite } from "./lookSites";
import type { LookData } from "../../../../lib/tauri";

const declare = (sites: unknown) => `<p>x</p><script type="application/json" id="wenlan-sites">${JSON.stringify(sites)}</script>`;

describe("UX-LOOK-9 the sites a look may reach", () => {
  beforeEach(() => window.localStorage.clear());

  it("reads the sites a look names, each once, at most five", () => {
    expect(declaredSites(declare(["api.open-meteo.com", "https://API.Open-Meteo.com/", "tiles.example.org"])))
      .toEqual(["api.open-meteo.com", "tiles.example.org"]);
    expect(declaredSites(declare(Array.from({ length: 9 }, (_, i) => `s${i}.example.org`)))).toHaveLength(LOOK_SITES_MAX);
    expect(declaredSites("<p>no sites</p>")).toEqual([]);
    expect(declaredSites('<script id="wenlan-sites">not json</script>')).toEqual([]);
    expect(declaredSites(declare({ site: "a.example.org" }))).toEqual([]);
  });

  it("never lets a look reach this computer, its network, a port or a path", () => {
    for (const bad of [
      "localhost", "127.0.0.1", "10.0.0.2", "192.168.1.1", "[::1]", "printer.local", "wenlan.localhost",
      "nas.home.arpa", "http://api.example.org", "api.example.org:7878", "api.example.org/path",
      "*.example.org", "a.example.org; script-src *", "'self'", "", 3, null,
    ]) {
      expect(normalizeSite(bad), String(bad)).toBeNull();
    }
  });

  it("puts only allowed sites into both frames' policy, over https, for requests and pictures", () => {
    expect(lookCsp([])).toBe(LOOK_CSP);
    const csp = lookCsp(["api.open-meteo.com", "localhost", "evil.org; connect-src *"]);
    expect(csp).toContain("connect-src https://api.open-meteo.com;");
    expect(csp).toContain("img-src data: blob: https://api.open-meteo.com;");
    expect(csp).not.toContain("localhost");
    expect(csp).not.toContain("evil");
    expect(csp).toContain("frame-src 'none'");
    const data = { api: 1, page: { id: "p", title: "", space: "", created: "", modified: "", text: "", html: "", sections: [], links: [] }, space: { name: "", pages: [], more: false }, theme: { mode: "dark" } } as LookData;
    const outer = buildLookDocument({ data, html: "<p>x</p>", theme: { mode: "dark", vars: DEFAULT_LOOK_THEMES.dark }, sites: ["api.open-meteo.com"] });
    // Once in the outer frame's policy and once in the look's own, inside its srcdoc.
    expect(outer.match(/connect-src https:\/\/api\.open-meteo\.com;/g)?.length).toBe(2);
    expect(buildLookDocument({ data, html: "<p>x</p>", theme: { mode: "dark", vars: DEFAULT_LOOK_THEMES.dark } })).not.toContain("https://");
  });

  it("remembers what the person allowed for each page", () => {
    expect(allowedSites("pg_a")).toEqual([]);
    allowSites("pg_a", ["api.open-meteo.com", "localhost"]);
    allowSites("pg_a", ["api.open-meteo.com"]);
    expect(allowedSites("pg_a")).toEqual(["api.open-meteo.com"]);
    expect(allowedSites("pg_b")).toEqual([]);
  });
});
