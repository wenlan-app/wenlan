// SPDX-License-Identifier: AGPL-3.0-only
// Guards against the ConstellationMap.tsx "--mem-text-primary" bug: a
// var(--mem-*) / var(--kg-*) reference to a token that was never defined in
// index.css computes to `initial` (often invisible text) instead of erroring.
import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { repoRelativePath } from "./test/sourceText";

const SRC_DIR = path.resolve("src");
const CSS_FILE = path.resolve("src/index.css");
const TOKEN_RE = /var\(\s*(--(?:mem|kg)-[a-zA-Z0-9-]+)/g;

// Pre-existing undefined-token reference this guard caught but that isn't an
// obvious one-line typo (no existing token is an unambiguous stand-in for a
// literal "blue" accent — the hardcoded #60a5fa fallback suggests a new token
// was intended, not a rename of an existing one). Reported upstream instead
// of guessed at. New occurrences anywhere else still fail.
const KNOWN_MISSING_TOKENS = new Set<string>([]);

function listTsxFiles(dir: string): string[] {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return listTsxFiles(fullPath);
    if (!entry.name.endsWith(".tsx")) return [];
    if (entry.name.endsWith(".test.tsx")) return [];
    return [fullPath];
  });
}

function definedTokens(css: string): Set<string> {
  const defined = new Set<string>();
  const re = /(--(?:mem|kg)-[a-zA-Z0-9-]+)\s*:/g;
  let match: RegExpExecArray | null;
  while ((match = re.exec(css))) defined.add(match[1]);
  return defined;
}

describe("CSS custom property guard", () => {
  it("does not reference a --mem-*/--kg-* token that index.css never defines", () => {
    const css = fs.readFileSync(CSS_FILE, "utf8");
    const defined = definedTokens(css);

    const missing: string[] = [];
    for (const file of listTsxFiles(SRC_DIR)) {
      const text = fs.readFileSync(file, "utf8");
      const relativeFile = repoRelativePath(file);
      const seenInFile = new Set<string>();
      let match: RegExpExecArray | null;
      TOKEN_RE.lastIndex = 0;
      while ((match = TOKEN_RE.exec(text))) {
        const token = match[1];
        const key = `${relativeFile}\t${token}`;
        if (!defined.has(token) && !seenInFile.has(token) && !KNOWN_MISSING_TOKENS.has(key)) {
          seenInFile.add(token);
          missing.push(key);
        }
      }
    }

    expect(missing).toEqual([]);
  });
});

function listFiles(dir: string, ext: string): string[] {
  const entries = fs.readdirSync(dir, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return listFiles(fullPath, ext);
    if (!entry.name.endsWith(ext) || entry.name.includes(".test.")) return [];
    return [fullPath];
  });
}

// Custom themes change corner roundness through --mem-radius-scale, so a
// literal px radius would ignore the theme. Pills (99px/999px/9999px), circles
// (%) and square corners (0) are shapes and stay literal.
describe("corner radius guard", () => {
  const PX = /(?<![\w.-])(\d+(?:\.\d+)?)px\b/g;
  const SHAPE_PX = new Set(["99", "999", "9999"]);
  const literalPx = (value: string) =>
    [...value.matchAll(PX)].map((m) => m[1]).filter((n) => !SHAPE_PX.has(n) && Number(n) !== 0);

  it("every CSS border radius uses a --mem-r-* step or a shape value", () => {
    const offenders: string[] = [];
    const decl = /border(?:-[a-z]+)*-radius\s*:\s*([^;}]+)/g;
    for (const file of listFiles(SRC_DIR, ".css")) {
      const text = fs.readFileSync(file, "utf8");
      for (const match of text.matchAll(decl)) {
        if (literalPx(match[1]).length) offenders.push(`${repoRelativePath(file)}\t${match[0].trim()}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("every inline or CodeMirror borderRadius uses a --mem-r-* step or a shape value", () => {
    const offenders: string[] = [];
    const prop = /border(?:Top|Bottom)?(?:Left|Right)?Radius\s*:\s*([^,}\n]+)/g;
    const scripts = [...listFiles(SRC_DIR, ".tsx"), ...listFiles(SRC_DIR, ".ts")].filter((file) => !file.endsWith(".d.ts"));
    for (const file of scripts) {
      const text = fs.readFileSync(file, "utf8");
      for (const match of text.matchAll(prop)) {
        const value = match[1].trim();
        const bareNumber = /^\d+(?:\.\d+)?$/.test(value) && Number(value) !== 0 && !SHAPE_PX.has(value);
        if (bareNumber || literalPx(value).length) offenders.push(`${repoRelativePath(file)}\t${match[0].trim()}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("every radius step scales with --mem-radius-scale", () => {
    const css = fs.readFileSync(CSS_FILE, "utf8");
    const steps = [...css.matchAll(/(--mem-r-(\d+)):\s*([^;]+);/g)];
    expect(steps.length).toBeGreaterThan(0);
    for (const [, , size, value] of steps) {
      expect(value.trim()).toBe(`calc(${size}px * var(--mem-radius-scale))`);
    }
  });
});

// Custom themes recolor the app through index.css tokens, so a color
// written as a literal inside a component ignores the theme. Allowed:
// black scrims and shadows (theme-neutral), third-party logo colors, and
// files that only use literals as fallbacks or canvas plumbing.
describe("component color literal guard", () => {
  const COLOR_LITERAL = /["'`](#[0-9A-Fa-f]{3,8}|rgba?\([^"'`]*\))["'`]/g;
  const BLACK = /^rgba?\(\s*0\s*,\s*0\s*,\s*0\s*(,\s*[\d.]+\s*)?\)$/;
  const ALLOWED_FILES = new Set([
    "src/components/memory/assets/toolMarks.tsx", // third-party logo colors
    "src/components/memory/MermaidDiagram.tsx", // fallbacks for unset tokens
    "src/lib/graph/cartography.ts", // builds rgba() from token channels
    "src/lib/graph/atlas.ts", // canvas default when sigma passes no color
    "src/components/memory/page/look/lookDocument.ts", // fallbacks for unset tokens in a look frame, which cannot read the app's CSS
  ]);

  it("components take colors from index.css tokens", () => {
    const offenders: string[] = [];
    const scripts = [...listFiles(SRC_DIR, ".tsx"), ...listFiles(SRC_DIR, ".ts")].filter((file) => !file.endsWith(".d.ts"));
    for (const file of scripts) {
      const relativeFile = repoRelativePath(file);
      if (ALLOWED_FILES.has(relativeFile) || relativeFile.includes("/__tests__/") || relativeFile.includes("/test/")) continue;
      const text = fs.readFileSync(file, "utf8");
      for (const match of text.matchAll(COLOR_LITERAL)) {
        if (!BLACK.test(match[1])) offenders.push(`${relativeFile}\t${match[1]}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
