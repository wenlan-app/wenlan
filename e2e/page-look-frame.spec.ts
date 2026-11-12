// SPDX-License-Identifier: AGPL-3.0-only
import http from "node:http";
import type { AddressInfo } from "node:net";
import { expect, test, type Frame, type Page } from "@playwright/test";
import {
  buildLookDocument,
  DEFAULT_LOOK_THEMES,
  escapeAttribute,
  parseLookMessage,
  knownPageIds,
} from "../src/components/memory/page/look/lookDocument";
import type { LookData } from "../src/lib/tauri";

// UX-LOOK-4: the document the app builds for a look, run in a real browser.
// An AI-drawn look runs scripts, so what it can reach is the whole promise:
// nothing leaves the frame, nothing of the app is readable, and the app hears
// four kinds of message from its own frame and nothing else.

const DATA: LookData = {
  api: 1,
  page: {
    id: "pg_kyoto",
    title: "Kyoto trip </script><b>",
    space: "Travel",
    created: "2026-11-01",
    modified: "2026-11-10",
    text: "## Plan\n\n- [ ] book the ryokan",
    html: "<h2>Plan</h2>",
    sections: [],
    links: [{ title: "Rail pass", id: "pg_rail" }],
  },
  space: { name: "Travel", pages: [{ id: "pg_rail", title: "Rail pass", created: "", modified: "", summary: "" }], more: false },
  theme: { mode: "dark" },
};

let server: http.Server;
let origin = "";
let hits: string[] = [];

test.beforeAll(async () => {
  server = http.createServer((request, response) => {
    hits.push(request.url ?? "");
    response.end("x");
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
test.afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()));
});
test.beforeEach(() => { hits = []; });

/** Everything a hostile look could try to reach out with, in one document. */
function probeLook(navigate: string): string {
  return `<h1 id="t"></h1>
<a data-wenlan-page="pg_rail" id="rail">Rail pass</a>
<img src="${origin}/img"><link rel="stylesheet" href="${origin}/css">
<link rel="dns-prefetch" href="${origin}/dns"><link rel="preconnect" href="${origin}/pre">
<div style="background:url(${origin}/cssurl);width:5px;height:5px"></div>
<script>
document.getElementById("t").textContent = window.wenlan.page.title;
window.__probe = {};
const t = (name, fn) => { try { const r = fn(); if (r && r.catch) r.catch(() => {}); window.__probe[name] = "ok"; } catch (e) { window.__probe[name] = "threw"; } };
t("fetch", () => fetch("${origin}/fetch"));
t("xhr", () => { const x = new XMLHttpRequest(); x.open("GET", "${origin}/xhr"); x.send(); });
t("ws", () => new WebSocket("ws://${origin.slice(7)}/ws"));
t("eventsource", () => new EventSource("${origin}/es"));
t("beacon", () => navigator.sendBeacon("${origin}/beacon", "x"));
t("rtc", () => new RTCPeerConnection({ iceServers: [{ urls: "stun:${origin.slice(7)}" }] }));
t("localStorage", () => localStorage.length);
t("cookie", () => document.cookie);
t("parentDocument", () => parent.document.title);
t("topDocument", () => top.document.title);
t("tauri", () => { if (window.__TAURI_INTERNALS__ || window.__TAURI__) throw 0; });
t("open", () => { if (!window.open("${origin}/open")) throw 0; });
setTimeout(() => {
  const nav = ${JSON.stringify(navigate)};
  if (nav === "href") location.href = "${origin}/nav-href";
  if (nav === "form") { const f = document.createElement("form"); f.action = "${origin}/form"; document.body.append(f); f.submit(); }
  if (nav === "meta") { const m = document.createElement("meta"); m.httpEquiv = "refresh"; m.content = "0;url=${origin}/meta"; document.head.append(m); }
  if (nav === "anchor") { const a = document.createElement("a"); a.href = "${origin}/a"; document.body.append(a); a.click(); }
  if (nav === "top") { try { top.location = "${origin}/top"; } catch (e) {} }
}, 300);
</script>`;
}

interface Seen { fromOuter: boolean; data: unknown }

async function mountLook(page: Page, html: string, data: LookData = DATA) {
  const outer = buildLookDocument({ data, html, theme: { mode: "dark", vars: DEFAULT_LOOK_THEMES.dark } });
  await page.setContent(`<!doctype html><body><iframe id="host" sandbox="allow-scripts" srcdoc="${escapeAttribute(outer)}"></iframe>
<script>
window.__seen = [];
addEventListener("message", (event) => {
  window.__seen.push({ fromOuter: event.source === document.getElementById("host").contentWindow, data: event.data });
});
</script></body>`);
}

const seen = (page: Page) => page.evaluate(() => (window as unknown as { __seen: Seen[] }).__seen);

function innerFrame(page: Page): Frame {
  const frame = page.frames().find((f) => f.parentFrame()?.parentFrame() === page.mainFrame());
  if (!frame) throw new Error("the look's frame is not mounted");
  return frame;
}

async function waitForReady(page: Page) {
  await expect.poll(async () => (await seen(page)).some((m) => (m.data as { wenlanLook?: { kind?: string } })?.wenlanLook?.kind === "ready"))
    .toBe(true);
}

test("a look is shown with the page's data, its theme and its height, and says ready", async ({ page }) => {
  await mountLook(page, probeLook("none"));
  await waitForReady(page);
  const inner = innerFrame(page);
  // The data is the page's, with `</script>` in a title kept as text.
  expect(await inner.evaluate(() => document.getElementById("t")!.textContent)).toBe("Kyoto trip </script><b>");
  expect(await inner.evaluate(() => (window as unknown as { wenlan: LookData }).wenlan.theme.mode)).toBe("dark");
  expect(await inner.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--look-bg").trim())).toBe("#191C24");
  const messages = await seen(page);
  const heights = messages.filter((m) => (m.data as { wenlanLook?: { kind?: string } })?.wenlanLook?.kind === "height");
  expect(heights.length).toBeGreaterThan(0);
  // Every message the app hears came through its own frame.
  expect(messages.every((m) => m.fromOuter)).toBe(true);
});

test("both frames are sandboxed with scripts only, never the app's origin", async ({ page }) => {
  await mountLook(page, "<p>x</p>");
  await waitForReady(page);
  const outerFrame = page.frames().find((f) => f.parentFrame() === page.mainFrame())!;
  expect(await outerFrame.evaluate(() => document.getElementById("look")!.getAttribute("sandbox"))).toBe("allow-scripts");
  expect(await page.evaluate(() => document.getElementById("host")!.getAttribute("sandbox"))).toBe("allow-scripts");
});

for (const navigate of ["none", "href", "form", "meta", "anchor", "top"]) {
  test(`a hostile look reaches nothing: ${navigate}`, async ({ page }) => {
    await mountLook(page, probeLook(navigate));
    await waitForReady(page);
    // Read before the look tries to leave: a blocked navigation replaces its document.
    const probe = await innerFrame(page).evaluate(() => (window as unknown as { __probe: Record<string, string> }).__probe);
    await page.waitForTimeout(1200);
    expect(hits, "a request left the frame").toEqual([]);
    // Storage, cookies, the app's document and its commands cannot be read.
    for (const name of ["localStorage", "cookie", "parentDocument", "topDocument", "rtc"]) {
      expect(probe[name], name).toBe("threw");
    }
    expect(probe.tauri).toBe("ok");
    expect(probe.open).toBe("threw");
  });
}

test("the app hears four kinds of message and nothing else", async ({ page }) => {
  await mountLook(page, "<p>x</p>");
  await waitForReady(page);
  const inner = innerFrame(page);
  await inner.evaluate(() => {
    const post = (message: unknown) => parent.postMessage(message, "*");
    post({ evil: true });
    post({ wenlanLook: { kind: "navigate", url: "https://example.com" } });
    post({ wenlanLook: { kind: "height", value: "tall" } });
    post({ wenlanLook: { kind: "height", value: 1e12 } });
    post({ wenlanLook: { kind: "error", message: "x".repeat(5000) } });
    post({ wenlanLook: { kind: "open", pageId: "pg_rail", extra: "ignored" } });
    // Straight to the app, around the outer frame.
    top!.postMessage({ wenlanLook: { kind: "open", pageId: "pg_evil" } }, "*");
  });
  await page.waitForTimeout(300);
  const messages = await seen(page);
  const direct = messages.filter((m) => !m.fromOuter);
  expect(direct.length).toBe(1);
  const relayed = messages.filter((m) => m.fromOuter).map((m) => (m.data as { wenlanLook: Record<string, unknown> }).wenlanLook);
  expect(relayed.every((m) => ["ready", "error", "height", "open"].includes(m.kind as string))).toBe(true);
  expect(relayed.find((m) => m.kind === "navigate")).toBeUndefined();
  expect(relayed.filter((m) => m.kind === "height").every((m) => Number.isFinite(m.value) && (m.value as number) <= 100000)).toBe(true);
  expect((relayed.find((m) => m.kind === "error")!.message as string).length).toBeLessThanOrEqual(500);
  expect(relayed.find((m) => m.kind === "open")).toEqual({ kind: "open", pageId: "pg_rail" });
  // What the app does with them: it would drop the one that skipped the outer frame (the
  // source check in LookFrame), and any open for a page the look was not given.
  const known = knownPageIds(DATA);
  expect(parseLookMessage(direct[0].data, known)).toBeNull();
  const accepted = [...relayed.map((m) => parseLookMessage({ wenlanLook: m }, known))].filter(Boolean);
  expect(accepted.some((m) => m && m.kind === "open" && m.pageId === "pg_rail")).toBe(true);
  expect(parseLookMessage({ wenlanLook: { kind: "open", pageId: "pg_evil" } }, known)).toBeNull();
});

test("a link to a page asks the app to open it and goes nowhere itself", async ({ page }) => {
  await mountLook(page, probeLook("none"));
  await waitForReady(page);
  await innerFrame(page).locator("#rail").click();
  await expect.poll(async () =>
    (await seen(page)).some((m) => m.fromOuter && JSON.stringify(m.data).includes('"open"') && JSON.stringify(m.data).includes("pg_rail")),
  ).toBe(true);
  expect(hits).toEqual([]);
});

test("a look that throws before it is ready says so", async ({ page }) => {
  await mountLook(page, "<script>throw new Error('boom')</script>");
  await expect.poll(async () => JSON.stringify(await seen(page))).toContain("boom");
});
