// SPDX-License-Identifier: AGPL-3.0-only
//
// The documents a page's look runs in (UX-LOOK-4). Pure functions, so the
// exact bytes can be tested, and driven in a real browser by the Playwright
// leak test.
//
// Two frames, one inside the other, both `sandbox="allow-scripts"` (never
// `allow-same-origin`):
//
//   host app > OUTER srcdoc frame > INNER srcdoc frame (the look)
//
// A `<meta>` CSP alone does not stop a frame navigating itself
// (`location.href`, meta refresh, a link click). The outer frame's CSP has
// `frame-src 'none'`, and a child's navigations are checked against its
// parent's: with it nothing the look does can send a request. The outer frame
// also relays the four messages the app accepts and nothing else.

import { parse } from "acorn";
import type { LookData } from "../../../../lib/tauri";
import { normalizeSite } from "./lookSites";

/** Both frames' policy with no sites allowed. It is the first element of each document. */
export const LOOK_CSP = [
  "default-src 'none'",
  "script-src 'unsafe-inline'",
  "style-src 'unsafe-inline'",
  "img-src data: blob:",
  "font-src data:",
  "media-src data: blob:",
  "connect-src 'none'",
  "frame-src 'none'",
  "form-action 'none'",
  "base-uri 'none'",
  "object-src 'none'",
  "worker-src 'none'",
].join("; ");

/** The only sandbox flag a look frame gets. */
export const LOOK_SANDBOX = "allow-scripts";

/** What the app tells a look about its colors, as `--look-*` variables. */
export interface LookTheme {
  mode: "light" | "dark";
  vars: Readonly<Record<string, string>>;
}

export const DEFAULT_LOOK_THEMES: Readonly<Record<"light" | "dark", LookTheme["vars"]>> = {
  dark: {
    "--look-bg": "#191C24",
    "--look-surface": "#202430",
    "--look-text": "#F1EFE8",
    "--look-muted": "#C9C8D1",
    "--look-border": "#333A4B",
    "--look-accent": "#8FB3EA",
    "--look-radius": "8px",
    "--look-font": "'Instrument Sans', -apple-system, 'Segoe UI', sans-serif",
    "--look-mono": "'JetBrains Mono', ui-monospace, monospace",
  },
  light: {
    "--look-bg": "#FCFCFB",
    "--look-surface": "#FFFFFF",
    "--look-text": "#1A1A2E",
    "--look-muted": "#586174",
    "--look-border": "#E3E7EE",
    "--look-accent": "#5E58C8",
    "--look-radius": "8px",
    "--look-font": "'Instrument Sans', -apple-system, 'Segoe UI', sans-serif",
    "--look-mono": "'JetBrains Mono', ui-monospace, monospace",
  },
};

/** The four things a look may tell the app. */
export type LookEvent =
  | { kind: "ready" }
  | { kind: "error"; message: string }
  | { kind: "height"; value: number }
  | { kind: "open"; pageId: string }
  | { kind: "check"; text: string; occurrence: number; done: boolean };

/**
 * The policy for a look the person let reach `sites` (UX-LOOK-9): those
 * hosts over https only, for requests and pictures, nothing else. A site that
 * is not a plain public host name is dropped here too.
 */
export function lookCsp(sites: readonly string[] = []): string {
  const allowed = sites.map(normalizeSite).filter((site): site is string => site !== null).map((site) => `https://${site}`);
  if (allowed.length === 0) return LOOK_CSP;
  const list = allowed.join(" ");
  return LOOK_CSP
    .replace("img-src data: blob:", `img-src data: blob: ${list}`)
    .replace("connect-src 'none'", `connect-src ${list}`);
}

const cspMeta = (sites: readonly string[]) => `<meta http-equiv="Content-Security-Policy" content="${lookCsp(sites)}">`;

/** `&` and `"` made safe for a double-quoted HTML attribute. */
export function escapeAttribute(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
}

/**
 * JSON that can sit inside a `<script>` element: `<` becomes `\u003c`, so
 * page text holding `</script>` or `<!--` stays data. The line separators
 * are escaped too, for engines that still treat them as line ends.
 */
export function jsonForScript(value: unknown): string {
  return JSON.stringify(value)
    .replace(/</g, "\\u003c")
    .replace(/\u2028/g, "\\u2028")
    .replace(/\u2029/g, "\\u2029");
}

/**
 * Elements a look may never put in its page. Some engines (WebKit, seen on
 * macOS) open a connection for `<link rel="preconnect">` even under
 * `connect-src 'none'`, and a nested frame would bring fresh, unguarded
 * built-ins. A look needs none of them: styles go in `<style>`, and nothing
 * outside the page can load anyway.
 */
export const LOOK_FORBIDDEN_TAGS = ["link", "iframe", "frame", "frameset", "object", "embed", "portal", "fencedframe", "meta", "base"];

/**
 * Runs before anything else in the look's frame and cannot be undone by it:
 * every way to put an element into the page drops the forbidden ones, every
 * way to parse HTML into the page strips them first, and the replaced
 * built-ins are locked (not writable, not configurable).
 */
export const INNER_GUARD_SCRIPT = `(function () {
  var FORBIDDEN = ${JSON.stringify(LOOK_FORBIDDEN_TAGS)};
  var SELECTOR = FORBIDDEN.join(",");
  var isForbidden = function (node) {
    return !!node && node.nodeType === 1 && FORBIDDEN.indexOf(String(node.localName).toLowerCase()) !== -1;
  };
  var scrub = function (node) {
    if (!node || typeof node !== "object") return node;
    if (isForbidden(node)) return document.createTextNode("");
    if (node.querySelectorAll) {
      var found = node.querySelectorAll(SELECTOR);
      for (var i = 0; i < found.length; i++) found[i].remove();
    }
    return node;
  };
  var lock = function (target, name, value) {
    try { Object.defineProperty(target, name, { value: value, writable: false, configurable: false }); } catch (e) {}
  };
  var lockAccessor = function (target, name, wrapSet) {
    var d = Object.getOwnPropertyDescriptor(target, name);
    if (!d || !d.set) return;
    try {
      Object.defineProperty(target, name, { get: d.get, set: wrapSet(d.set), configurable: false, enumerable: d.enumerable });
    } catch (e) {}
  };
  var template = document.createElement("template");
  var templateSet = Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML") ||
    Object.getOwnPropertyDescriptor(HTMLElement.prototype, "innerHTML");
  var clean = function (html) {
    var t = template.cloneNode(false);
    templateSet.set.call(t, String(html));
    scrub(t.content);
    return templateSet.get.call(t);
  };
  var wrapNodes = function (proto, name) {
    var original = proto && proto[name];
    if (typeof original !== "function") return;
    lock(proto, name, function () {
      var args = Array.prototype.slice.call(arguments);
      for (var i = 0; i < args.length; i++) args[i] = scrub(args[i]);
      return original.apply(this, args);
    });
  };
  [Node.prototype].forEach(function (p) { ["appendChild", "insertBefore", "replaceChild"].forEach(function (n) { wrapNodes(p, n); }); });
  [Element.prototype, Document.prototype, DocumentFragment.prototype, CharacterData.prototype].forEach(function (p) {
    ["append", "prepend", "before", "after", "replaceWith", "replaceChildren", "insertAdjacentElement"].forEach(function (n) { wrapNodes(p, n); });
  });
  if (window.Range) ["insertNode", "surroundContents"].forEach(function (n) { wrapNodes(Range.prototype, n); });
  [Element.prototype, HTMLElement.prototype, window.ShadowRoot && ShadowRoot.prototype].forEach(function (p) {
    if (!p) return;
    ["innerHTML", "outerHTML"].forEach(function (n) {
      lockAccessor(p, n, function (set) { return function (value) { return set.call(this, clean(value)); }; });
    });
  });
  var wrapHtmlArg = function (proto, name, index) {
    var original = proto && proto[name];
    if (typeof original !== "function") return;
    lock(proto, name, function () {
      var args = Array.prototype.slice.call(arguments);
      if (args.length > index) args[index] = clean(args[index]);
      return original.apply(this, args);
    });
  };
  wrapHtmlArg(Element.prototype, "insertAdjacentHTML", 1);
  wrapHtmlArg(Element.prototype, "setHTMLUnsafe", 0);
  if (window.ShadowRoot) wrapHtmlArg(ShadowRoot.prototype, "setHTMLUnsafe", 0);
  if (window.Range) wrapHtmlArg(Range.prototype, "createContextualFragment", 0);
  wrapHtmlArg(Document.prototype, "write", 0);
  wrapHtmlArg(Document.prototype, "writeln", 0);
  lock(Document.prototype, "execCommand", function () { return false; });
  try {
    new MutationObserver(function (records) {
      for (var i = 0; i < records.length; i++) {
        var added = records[i].addedNodes;
        for (var j = 0; j < added.length; j++) {
          if (isForbidden(added[j])) added[j].remove(); else scrub(added[j]);
        }
      }
    }).observe(document, { childList: true, subtree: true });
  } catch (e) {}
})();`;

/** Runs first inside the look's frame: defines `window.wenlan` and reports. */
export const INNER_HELPER_SCRIPT = `(function () {
  var send = function (message) {
    try { parent.postMessage({ wenlanLook: message }, "*"); } catch (e) {}
  };
  ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel"].forEach(function (name) {
    try { delete window[name]; } catch (e) {}
    try { if (name in window) Object.defineProperty(window, name, { value: undefined, configurable: true }); } catch (e) {}
  });
  try { window.open = function () { return null; }; } catch (e) {}
  var data = {};
  try {
    data = JSON.parse(document.getElementById("wenlan-data").textContent);
  } catch (e) {
    send({ kind: "error", message: "The page data could not be read." });
  }
  // The page's pictures (UX-LOOK-8) come in as data: URLs; put each in its
  // place in the page's html so a look that shows the html shows them.
  var pictures = data.page && data.page.images ? data.page.images : [];
  var PICTURE = /^data:image\\/[a-z0-9.+-]+;base64,[A-Za-z0-9+\\/=]*$/;
  var placePictures = function (html) {
    if (typeof html !== "string") return html;
    return html.replace(/<img data-wenlan-image="(\\d+)"/g, function (whole, n) {
      var picture = pictures[Number(n)];
      if (!picture || typeof picture.url !== "string" || !PICTURE.test(picture.url)) return whole;
      return '<img src="' + picture.url + '" data-wenlan-image="' + n + '"';
    });
  };
  if (data.page) {
    data.page.html = placePictures(data.page.html);
    (data.page.sections || []).forEach(function (section) { section.html = placePictures(section.html); });
  }
  var listeners = [];
  data.onChange = function (fn) { if (typeof fn === "function") listeners.push(fn); };
  data.open = function (pageId) { send({ kind: "open", pageId: String(pageId) }); };
  // wenlan.check(item, done) ticks a checklist item of this page. The app
  // finds it by its text and which same-text item it is.
  data.check = function (item, done) {
    if (!item || typeof item.text !== "string") return;
    var sections = data.page && data.page.sections ? data.page.sections : [];
    var occurrence = 0;
    for (var s = 0; s < sections.length; s++) {
      var items = sections[s].items || [];
      for (var i = 0; i < items.length; i++) {
        if (items[i] === item) {
          send({ kind: "check", text: item.text, occurrence: occurrence, done: !!done });
          return;
        }
        if (items[i].done !== null && items[i].text === item.text) occurrence++;
      }
    }
  };
  window.wenlan = data;
  // WebKit hides the cause of any error thrown in a sandboxed frame. So the
  // look's own code runs inside catches that hold the real error: each of its
  // scripts (see prepareLookScript) and every callback it hands the browser.
  var describe = function (error) {
    if (error && typeof error === "object" && error.message) {
      return (error.name ? String(error.name) + ": " : "") + String(error.message);
    }
    return String(error);
  };
  window.__wenlanLookError = function (error, script) {
    send({ kind: "error", message: describe(error) + (script ? " (look script " + script + ")" : "") });
  };
  var wrapped = new WeakMap();
  var wrap = function (fn) {
    if (typeof fn !== "function" && !(fn && typeof fn === "object" && typeof fn.handleEvent === "function")) return fn;
    var known = wrapped.get(fn);
    if (known) return known;
    var callback = function () {
      try {
        return typeof fn === "function" ? fn.apply(this, arguments) : fn.handleEvent.apply(fn, arguments);
      } catch (error) {
        window.__wenlanLookError(error);
      }
    };
    wrapped.set(fn, callback);
    return callback;
  };
  ["setTimeout", "setInterval", "requestAnimationFrame", "queueMicrotask", "requestIdleCallback"].forEach(function (name) {
    var original = window[name];
    if (typeof original !== "function") return;
    window[name] = function () {
      var args = Array.prototype.slice.call(arguments);
      args[0] = wrap(args[0]);
      return original.apply(window, args);
    };
  });
  try {
    var addListener = EventTarget.prototype.addEventListener;
    var removeListener = EventTarget.prototype.removeEventListener;
    EventTarget.prototype.addEventListener = function (type, listener, options) {
      return addListener.call(this, type, wrap(listener), options);
    };
    EventTarget.prototype.removeEventListener = function (type, listener, options) {
      return removeListener.call(this, type, (listener && wrapped.get(listener)) || listener, options);
    };
  } catch (e) {}
  [window, Window.prototype, Document.prototype, HTMLElement.prototype, SVGElement.prototype].forEach(function (target) {
    Object.getOwnPropertyNames(target).forEach(function (name) {
      if (name.slice(0, 2) !== "on" || name === "onerror" || name === "onunhandledrejection") return;
      var descriptor = Object.getOwnPropertyDescriptor(target, name);
      if (!descriptor || !descriptor.set || !descriptor.configurable) return;
      try {
        Object.defineProperty(target, name, {
          configurable: true,
          enumerable: descriptor.enumerable,
          get: descriptor.get,
          set: function (value) { descriptor.set.call(this, wrap(value)); },
        });
      } catch (e) {}
    });
  });
  window.onerror = function (message, source, line, column, error) {
    var text = error && error.message ? String(error.message) : String(message || "");
    // WebKit hides the cause of an error in a sandboxed frame. Say so, so the
    // AI that wrote the look knows to check its own script.
    if (!text || /^Script error\.?$/.test(text)) {
      text = "A script in the look failed; this browser hides the cause. Check the look's script with read_page_look";
    }
    send({ kind: "error", message: text + (line ? " (line " + line + ")" : "") });
  };
  window.addEventListener("unhandledrejection", function (event) {
    var reason = event.reason;
    send({ kind: "error", message: String(reason && reason.message ? reason.message : reason) });
  });
  var last = -1;
  var measure = function () {
    var root = document.documentElement;
    var body = document.body;
    var height = Math.ceil(Math.max(root.getBoundingClientRect().height, body ? body.getBoundingClientRect().bottom : 0));
    if (height !== last) { last = height; send({ kind: "height", value: height }); }
  };
  var decorate = function () {
    var anchors = document.querySelectorAll("a[data-wenlan-page]");
    for (var i = 0; i < anchors.length; i++) {
      if (!anchors[i].hasAttribute("tabindex")) anchors[i].setAttribute("tabindex", "0");
      if (!anchors[i].hasAttribute("role")) anchors[i].setAttribute("role", "link");
    }
  };
  var openAnchor = function (event) {
    var target = event.target;
    var anchor = target && target.closest ? target.closest("a, area") : null;
    if (!anchor) return false;
    event.preventDefault();
    var pageId = anchor.getAttribute("data-wenlan-page");
    if (pageId) data.open(pageId);
    return true;
  };
  document.addEventListener("click", openAnchor, true);
  document.addEventListener("keydown", function (event) {
    if (event.key === "Enter") openAnchor(event);
  }, true);
  document.addEventListener("DOMContentLoaded", function () {
    decorate();
    try { new MutationObserver(function () { decorate(); measure(); }).observe(document.documentElement, { childList: true, subtree: true }); } catch (e) {}
  });
  window.addEventListener("load", function () {
    measure();
    try {
      var observer = new ResizeObserver(measure);
      observer.observe(document.documentElement);
      if (document.body) observer.observe(document.body);
    } catch (e) {}
    // Not requestAnimationFrame: a hidden or occluded window never paints,
    // so a look that works would be reported as stuck.
    setTimeout(function () { send({ kind: "ready" }); }, 0);
  });
})();`;

/** Runs in the outer frame: relays the four messages, checked, and no others. */
export const OUTER_RELAY_SCRIPT = `(function () {
  var inner = document.getElementById("look");
  var kinds = ["ready", "error", "height", "open", "check"];
  window.addEventListener("message", function (event) {
    if (event.source !== inner.contentWindow) return;
    var message = event.data && event.data.wenlanLook;
    if (!message || typeof message !== "object" || kinds.indexOf(message.kind) === -1) return;
    var out = { kind: message.kind };
    if (message.kind === "error") {
      out.message = String(message.message == null ? "" : message.message).slice(0, 500);
    } else if (message.kind === "height") {
      var value = Number(message.value);
      if (!isFinite(value)) return;
      out.value = Math.max(0, Math.min(100000, Math.ceil(value)));
    } else if (message.kind === "open") {
      out.pageId = String(message.pageId == null ? "" : message.pageId).slice(0, 200);
    } else if (message.kind === "check") {
      out.text = String(message.text == null ? "" : message.text).slice(0, 2000);
      out.occurrence = Number(message.occurrence);
      out.done = message.done === true;
    }
    parent.postMessage({ wenlanLook: out }, "*");
  });
})();`;

function themeStyle(theme: LookTheme): string {
  const vars = Object.entries(theme.vars)
    .filter(([name]) => /^--look-[a-z-]+$/.test(name))
    // A value is one CSS token run; nothing in it may end the declaration.
    .map(([name, value]) => `${name}: ${value.replace(/[;{}<>]/g, "")};`)
    .join(" ");
  return `<style>:root { ${vars} color-scheme: ${theme.mode}; } body { margin: 0; }</style>`;
}

/**
 * The look's own markup with the forbidden elements taken out before it is
 * parsed, so not even the first parse can open a connection. Uses the app's
 * DOMParser (an inert document) where there is one.
 */
export function stripForbidden(html: string): string {
  if (typeof DOMParser === "undefined") {
    const names = LOOK_FORBIDDEN_TAGS.join("|");
    return html
      .replace(new RegExp(`<(${names})\\b[\\s\\S]*?<\\/\\1\\s*>`, "gi"), "")
      .replace(new RegExp(`<\\/?(${names})\\b[^>]*>`, "gi"), "");
  }
  const doc = new DOMParser().parseFromString(`<!doctype html><html><head></head><body>${html}</body></html>`, "text/html");
  doc.querySelectorAll(LOOK_FORBIDDEN_TAGS.join(",")).forEach((node) => node.remove());
  return doc.head.innerHTML + doc.body.innerHTML;
}

/** Scripts the browser runs as classic scripts, where a catch can be added. */
const CLASSIC_SCRIPT_TYPES = ["", "text/javascript", "application/javascript", "text/ecmascript"];

type Node_ = { type: string; start: number; end: number; [key: string]: unknown };

function boundNames(pattern: Node_, out: string[]): void {
  switch (pattern.type) {
    case "Identifier": out.push(pattern.name as string); return;
    case "ObjectPattern":
      for (const property of pattern.properties as Node_[]) {
        boundNames((property.type === "RestElement" ? property.argument : property.value) as Node_, out);
      }
      return;
    case "ArrayPattern":
      for (const element of pattern.elements as (Node_ | null)[]) if (element) boundNames(element, out);
      return;
    case "RestElement": boundNames(pattern.argument as Node_, out); return;
    case "AssignmentPattern": boundNames(pattern.left as Node_, out); return;
  }
}

/**
 * One of the look's inline scripts, run inside a catch that reports the real
 * error: WebKit tells a sandboxed frame only "Script error.". Top-level
 * `let`, `const` and `class` names are declared before the `try`, so they
 * stay shared with the look's other scripts and its `onclick` attributes.
 * A script that does not parse is replaced by one that reports why; a strict
 * script is left as it is, since a block would scope its functions.
 */
export function prepareLookScript(source: string, index: number): string {
  let program: Node_;
  try {
    program = parse(source, { ecmaVersion: "latest", sourceType: "script", allowHashBang: true }) as unknown as Node_;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return `window.__wenlanLookError(${jsonForScript({ name: "SyntaxError", message })}, ${index});`;
  }
  const body = program.body as Node_[];
  if (body.some((node) => node.type === "ExpressionStatement" && node.directive === "use strict")) return source;
  const names: string[] = [];
  const edits: { start: number; end: number; text: string }[] = [];
  for (const node of body) {
    if (node.type === "VariableDeclaration" && node.kind !== "var") {
      const parts: string[] = [];
      let pattern = false;
      for (const declarator of node.declarations as Node_[]) {
        const id = declarator.id as Node_;
        boundNames(id, names);
        if (id.type !== "Identifier") pattern = true;
        const init = declarator.init as Node_ | null;
        if (init) parts.push(`${source.slice(id.start, id.end)} = ${source.slice(init.start, init.end)}`);
      }
      const text = parts.length === 0 ? ";" : pattern ? `(${parts.join(", ")});` : `${parts.join(", ")};`;
      edits.push({ start: node.start, end: node.end, text });
    } else if (node.type === "ClassDeclaration" && node.id) {
      const name = (node.id as Node_).name as string;
      names.push(name);
      edits.push({ start: node.start, end: node.end, text: `${name} = ${source.slice(node.start, node.end)};` });
    }
  }
  let out = source;
  for (const edit of edits.reverse()) out = out.slice(0, edit.start) + edit.text + out.slice(edit.end);
  const declared = [...new Set(names)];
  return (declared.length ? `let ${declared.join(", ")};\n` : "")
    + `try {\n${out}\n} catch (error) { window.__wenlanLookError(error, ${index}); }`;
}

/**
 * The look's HTML as it goes into its frame: without elements that reach out
 * (see `stripForbidden`), and with each inline classic script run through
 * `prepareLookScript`.
 */
export function prepareLookHtml(html: string): string {
  if (typeof DOMParser === "undefined") return stripForbidden(html);
  const doc = new DOMParser().parseFromString(`<!doctype html><html><head></head><body>${html}</body></html>`, "text/html");
  doc.querySelectorAll(LOOK_FORBIDDEN_TAGS.join(",")).forEach((node) => node.remove());
  let index = 0;
  doc.querySelectorAll("script").forEach((script) => {
    if (script.hasAttribute("src")) return;
    const type = (script.getAttribute("type") ?? "").trim().toLowerCase();
    if (!CLASSIC_SCRIPT_TYPES.includes(type)) return;
    index += 1;
    script.textContent = prepareLookScript(script.textContent ?? "", index);
  });
  return doc.head.innerHTML + doc.body.innerHTML;
}

/**
 * The look's own document: doctype, the CSP first, the theme variables, the
 * page's data as inline JSON, the helper script, then the look's HTML.
 */
export function buildInnerDocument({ data, html, theme, sites = [] }: {
  data: LookData;
  html: string;
  theme: LookTheme;
  /** Sites the person let this look reach (UX-LOOK-9); none by default. */
  sites?: readonly string[];
}): string {
  const withTheme: LookData = { ...data, theme: { mode: theme.mode } };
  return [
    "<!doctype html>",
    `<html><head><meta charset="utf-8">${cspMeta(sites)}`,
    themeStyle(theme),
    `<script type="application/json" id="wenlan-data">${jsonForScript(withTheme)}</script>`,
    `<script>${INNER_GUARD_SCRIPT}</script>`,
    `<script>${INNER_HELPER_SCRIPT}</script>`,
    "</head><body>",
    prepareLookHtml(html),
    // Last, so it wins: the look sits in the page's own column, which already
    // has its margins. Outer padding of the look's own would shift it off the
    // text it replaces.
    "<style>html, body { margin: 0 !important; padding: 0 !important; max-width: none !important; background: transparent !important; }</style>",
    "</body></html>",
  ].join("\n");
}

/** The frame the app mounts: the CSP first, the look's frame, the relay. */
export function buildOuterDocument(innerDocument: string, sites: readonly string[] = [], mode: LookTheme["mode"] = "dark"): string {
  // The look's frame also keeps this frame's policy, so both name the sites.
  // Each frame's color scheme matches the one around it: a mismatch makes the
  // browser paint the frame an opaque default background.
  return [
    "<!doctype html>",
    `<html><head><meta charset="utf-8">${cspMeta(sites)}`,
    `<style>:root { color-scheme: ${mode === "light" ? "light" : "dark"}; } html, body { height: 100%; margin: 0; background: transparent; } iframe { display: block; width: 100%; height: 100%; border: 0; color-scheme: inherit; }</style>`,
    "</head><body>",
    `<iframe id="look" sandbox="${LOOK_SANDBOX}" srcdoc="${escapeAttribute(innerDocument)}"></iframe>`,
    `<script>${OUTER_RELAY_SCRIPT}</script>`,
    "</body></html>",
  ].join("\n");
}

/** Both documents for one look. */
export function buildLookDocument(args: { data: LookData; html: string; theme: LookTheme; sites?: readonly string[] }): string {
  return buildOuterDocument(buildInnerDocument(args), args.sites, args.theme.mode);
}

/**
 * What the app accepts from its outer frame, or null for anything else.
 * `open` is honored only for a page named in the data the look was given.
 */
export function parseLookMessage(
  raw: unknown,
  knownPageIds: ReadonlySet<string>,
  taskTexts: ReadonlySet<string> = new Set(),
): LookEvent | null {
  if (typeof raw !== "object" || raw === null) return null;
  const body = (raw as { wenlanLook?: unknown }).wenlanLook;
  if (typeof body !== "object" || body === null) return null;
  const message = body as Record<string, unknown>;
  switch (message.kind) {
    case "ready":
      return { kind: "ready" };
    case "error":
      return { kind: "error", message: typeof message.message === "string" ? message.message.slice(0, 500) : "" };
    case "height": {
      const value = message.value;
      if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > 100_000) return null;
      return { kind: "height", value: Math.ceil(value) };
    }
    case "open": {
      const pageId = message.pageId;
      if (typeof pageId !== "string" || !knownPageIds.has(pageId)) return null;
      return { kind: "open", pageId };
    }
    case "check": {
      const { text, occurrence, done } = message;
      if (typeof text !== "string" || !taskTexts.has(text)) return null;
      if (typeof occurrence !== "number" || !Number.isInteger(occurrence) || occurrence < 0 || occurrence > 10_000) return null;
      if (typeof done !== "boolean") return null;
      return { kind: "check", text, occurrence, done };
    }
    default:
      return null;
  }
}

/** Every page id a look may open: its own, its Space's, and its links'. */
export function knownPageIds(data: LookData): Set<string> {
  const ids = new Set<string>([data.page.id]);
  for (const page of data.space.pages) ids.add(page.id);
  for (const link of data.page.links) if (link.id) ids.add(link.id);
  for (const page of data.home?.pages ?? []) ids.add(page.id);
  return ids;
}

/** The text of every checklist item a look may tick. */
export function taskTexts(data: LookData): Set<string> {
  const texts = new Set<string>();
  for (const section of data.page.sections) for (const item of section.items) if (item.done !== null) texts.add(item.text);
  return texts;
}

/** The app's colors for a look, read from the tokens on `root`. */
export function readLookTheme(root: HTMLElement = document.documentElement): LookTheme {
  const mode = root.getAttribute("data-theme") === "light" ? "light" : "dark";
  const style = getComputedStyle(root);
  const read = (token: string, fallback: string) => style.getPropertyValue(token).trim() || fallback;
  const base = DEFAULT_LOOK_THEMES[mode];
  return {
    mode,
    vars: {
      "--look-bg": read("--mem-bg", base["--look-bg"]),
      "--look-surface": read("--mem-surface", base["--look-surface"]),
      "--look-text": read("--mem-text", base["--look-text"]),
      "--look-muted": read("--mem-text-secondary", base["--look-muted"]),
      "--look-border": read("--mem-border", base["--look-border"]),
      "--look-accent": read("--mem-accent-page", base["--look-accent"]),
      "--look-radius": base["--look-radius"],
      "--look-font": base["--look-font"],
      "--look-mono": base["--look-mono"],
    },
  };
}
