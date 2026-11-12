// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { LookData } from "../../../../lib/tauri";
import {
  buildInnerDocument,
  buildLookDocument,
  DEFAULT_LOOK_THEMES,
  jsonForScript,
  knownPageIds,
  LOOK_CSP,
  LOOK_SANDBOX,
  parseLookMessage,
} from "./lookDocument";

const DATA: LookData = {
  api: 1,
  page: {
    id: "pg_a", title: "A </script><script>alert(1)</script>", space: "", created: "", modified: "",
    text: "<!-- x -->", html: "", sections: [], links: [{ title: "B", id: "pg_b" }, { title: "Nowhere", id: null }],
  },
  space: { name: "", pages: [{ id: "pg_c", title: "C", created: "", modified: "", summary: "" }], more: false },
  theme: { mode: "dark" },
};
const THEME = { mode: "dark" as const, vars: DEFAULT_LOOK_THEMES.dark };

describe("UX-LOOK-4 the documents a look runs in", () => {
  it("puts the policy first in both documents and allows scripts and nothing else", () => {
    const inner = buildInnerDocument({ data: DATA, html: "<p>x</p>", theme: THEME });
    expect(inner.indexOf("<meta http-equiv=\"Content-Security-Policy\"")).toBeGreaterThan(-1);
    expect(inner.indexOf("Content-Security-Policy")).toBeLessThan(inner.indexOf("<script"));
    expect(inner.indexOf("Content-Security-Policy")).toBeLessThan(inner.indexOf("<style"));
    const outer = buildLookDocument({ data: DATA, html: "<p>x</p>", theme: THEME });
    expect(outer.indexOf("Content-Security-Policy")).toBeLessThan(outer.indexOf("<iframe"));
    expect(LOOK_SANDBOX).toBe("allow-scripts");
    expect(outer).toContain(`<iframe id="look" sandbox="allow-scripts"`);
    expect(outer).not.toContain("allow-same-origin");
    expect(LOOK_CSP).toContain("default-src 'none'");
    expect(LOOK_CSP).toContain("frame-src 'none'");
    expect(LOOK_CSP).toContain("connect-src 'none'");
  });

  it("keeps page text that looks like markup as data, never as a tag", () => {
    const inner = buildInnerDocument({ data: DATA, html: "", theme: THEME });
    const block = inner.match(/<script type="application\/json" id="wenlan-data">([\s\S]*?)<\/script>/);
    expect(block).not.toBeNull();
    expect(block![1]).not.toContain("<");
    expect(JSON.parse(block![1]).page.title).toBe(DATA.page.title);
    expect(jsonForScript({ a: " <!--" })).toBe('{"a":"\\u2028\\u003c!--"}');
  });

  it("fills in the app's theme mode and colors", () => {
    const inner = buildInnerDocument({ data: DATA, html: "", theme: { mode: "light", vars: DEFAULT_LOOK_THEMES.light } });
    expect(inner).toContain("--look-bg: #FCFCFB");
    expect(inner).toContain('"theme":{"mode":"light"}');
  });

  it("drops a theme variable that is not a look variable or would end its declaration", () => {
    const inner = buildInnerDocument({
      data: DATA, html: "",
      theme: { mode: "dark", vars: { "--look-bg": "red; } body { display:none", "--evil": "x" } },
    });
    expect(inner).not.toContain("--evil");
    // The braces and semicolon are gone, so it stays one (useless) value.
    const style = inner.match(/<style>:root \{([^}]*)\}/)![1];
    expect(style).not.toContain(";  body");
    expect(inner.match(/--look-bg: [^;]*;/)![0]).not.toMatch(/[{}]/);
  });
});

describe("UX-LOOK-4 what the app accepts from the frame", () => {
  const known = knownPageIds(DATA);

  it("knows the page, its links and its Space's pages", () => {
    expect([...known].sort()).toEqual(["pg_a", "pg_b", "pg_c"]);
  });

  it("accepts the four kinds and nothing else", () => {
    expect(parseLookMessage({ wenlanLook: { kind: "ready" } }, known)).toEqual({ kind: "ready" });
    expect(parseLookMessage({ wenlanLook: { kind: "error", message: "boom" } }, known)).toEqual({ kind: "error", message: "boom" });
    expect(parseLookMessage({ wenlanLook: { kind: "height", value: 300.2 } }, known)).toEqual({ kind: "height", value: 301 });
    expect(parseLookMessage({ wenlanLook: { kind: "open", pageId: "pg_b" } }, known)).toEqual({ kind: "open", pageId: "pg_b" });
    for (const bad of [
      null, "x", 3, {}, { wenlanLook: null }, { wenlanLook: { kind: "navigate" } },
      { wenlanLook: { kind: "height", value: "300" } }, { wenlanLook: { kind: "height", value: -1 } },
      { wenlanLook: { kind: "height", value: 1e9 } }, { wenlanLook: { kind: "height", value: Number.NaN } },
      { wenlanLook: { kind: "open", pageId: "pg_unknown" } }, { wenlanLook: { kind: "open" } },
      { wenlanLook: { kind: "open", pageId: 3 } },
    ]) {
      expect(parseLookMessage(bad, known), JSON.stringify(bad)).toBeNull();
    }
  });

  it("cuts an error to 500 characters", () => {
    const message = parseLookMessage({ wenlanLook: { kind: "error", message: "x".repeat(900) } }, known);
    expect(message && message.kind === "error" && message.message.length).toBe(500);
  });
});
