// SPDX-License-Identifier: AGPL-3.0-only
import { runInNewContext } from "node:vm";
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
  prepareLookHtml,
  prepareLookScript,
  stripForbidden,
  INNER_GUARD_SCRIPT,
  INNER_HELPER_SCRIPT,
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

describe("UX-LOOK-4 a look cannot add elements that reach out", () => {
  it("takes link, frame, object, meta and base elements out of the look before it is parsed", () => {
    const out = stripForbidden(
      '<link rel="preconnect" href="https://x.test"><div id="keep">ok</div><iframe srcdoc="x"></iframe>'
      + '<object data="x"></object><embed src="x"><meta http-equiv="refresh" content="0"><base href="https://x.test/">'
      + '<script>var s = "<link>";</script>',
    );
    expect(out.replace(/<script>[\s\S]*?<\/script>/g, "")).not.toMatch(/<(link|iframe|object|embed|meta|base)\b/i);
    expect(out).toContain('<div id="keep">ok</div>');
    expect(out).toContain('var s = "<link>";');
  });

  it("runs the guard before the helper and before any of the look's own markup", () => {
    const doc = buildInnerDocument({ data: DATA, html: "<p>look</p>", theme: THEME });
    const guard = doc.indexOf(INNER_GUARD_SCRIPT);
    expect(guard).toBeGreaterThan(doc.indexOf(LOOK_CSP));
    expect(guard).toBeLessThan(doc.indexOf(INNER_HELPER_SCRIPT));
    expect(doc.indexOf(INNER_HELPER_SCRIPT)).toBeLessThan(doc.indexOf("<p>look</p>"));
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

describe("UX-LOOK-4 a look's errors reach the app with their cause", () => {
  // Runs prepared scripts one after another in one global, like a page does.
  const run = (...scripts: string[]) => {
    const errors: string[] = [];
    const context: Record<string, unknown> = {};
    context.window = context;
    context.__wenlanLookError = (error: Error, script: number) => errors.push(`${error.name}: ${error.message} @${script}`);
    for (const [i, source] of scripts.entries()) runInNewContext(prepareLookScript(source, i + 1), context);
    return { errors, context };
  };

  it("reports the real error of a script that throws", () => {
    const { errors } = run("const w = 1;\nundefinedFn();");
    expect(errors).toEqual(["ReferenceError: undefinedFn is not defined @1"]);
  });

  it("keeps top-level names shared between the look's scripts", () => {
    const { errors, context } = run(
      "const shared = 42; let { a, b: [c] } = { a: 1, b: [2] }; class Box { get v() { return shared; } } function f() { return a + c; }",
      "var seen = [typeof shared, new Box().v, f(), typeof Box].join(',');",
    );
    expect(errors).toEqual([]);
    expect(context.seen).toBe("number,42,3,function");
  });

  it("reports a script that does not parse instead of running it", () => {
    const { errors } = run("const = ;");
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatch(/^SyntaxError: .+ @1$/);
  });

  it("leaves a strict script as it is", () => {
    expect(prepareLookScript('"use strict";\nconst x = 1;', 1)).toBe('"use strict";\nconst x = 1;');
  });

  it("prepares inline classic scripts only, numbered in order", () => {
    const out = prepareLookHtml(
      '<script>a()</script><script type="application/json">{"x":1}</script><script type="module">b()</script><script>c()</script>',
    );
    expect(out).toContain("__wenlanLookError(error, 1)");
    expect(out).toContain("__wenlanLookError(error, 2)");
    expect(out).toContain('{"x":1}');
    expect(out).toContain("<script type=\"module\">b()</script>");
  });
});
