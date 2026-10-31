// SPDX-License-Identifier: AGPL-3.0-only
// Keeps docs/ux/spec.md and the tests that enforce it in step: every rule
// marked `shipped` needs a test whose name carries its id, and no test may name
// an id the spec does not define. The spec explains who may change a rule.
import { describe, expect, it } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { readSourceText, repoRelativePath } from "./test/sourceText";

const SPEC_FILE = path.resolve("docs/ux/spec.md");
const THIS_FILE = path.resolve("src/uxSpec.test.ts");

const RULE_RE = /^- \*\*(UX-[A-Z]+-\d+)\*\* `(shipped|planned)`/gm;
const TS_ID_RE = /\bUX-[A-Z]+-\d+\b/g;
const RUST_ID_RE = /\bux_([a-z]+)_(\d+)\b/g;

// Test files live next to the code; Rust tests are inline modules too, so
// every .rs file counts. Build output and dependencies are skipped.
const ROOTS: ReadonlyArray<{ dir: string; isTest: (name: string) => boolean }> = [
  { dir: "src", isTest: (name) => /\.test\.tsx?$/.test(name) },
  { dir: "e2e", isTest: (name) => /\.spec\.ts$/.test(name) },
  { dir: "preview", isTest: (name) => /\.test\.ts$/.test(name) },
  { dir: "scripts", isTest: (name) => /\.test\.m?[jt]s$/.test(name) },
  { dir: "crates", isTest: (name) => name.endsWith(".rs") },
  { dir: "app/src", isTest: (name) => name.endsWith(".rs") },
];
const SKIP_DIRS = new Set(["node_modules", "target", "dist", ".git"]);

function listFiles(dir: string, isTest: (name: string) => boolean): string[] {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return SKIP_DIRS.has(entry.name) ? [] : listFiles(fullPath, isTest);
    return isTest(entry.name) ? [fullPath] : [];
  });
}

function specRules(): Map<string, "shipped" | "planned"> {
  const text = readSourceText(SPEC_FILE);
  const rules = new Map<string, "shipped" | "planned">();
  const duplicates: string[] = [];
  for (const match of text.matchAll(RULE_RE)) {
    if (rules.has(match[1])) duplicates.push(match[1]);
    rules.set(match[1], match[2] as "shipped" | "planned");
  }
  expect(duplicates, "ids defined twice in docs/ux/spec.md").toEqual([]);
  return rules;
}

/** Every UX id a test file names, mapped to the files that name it. */
function idsNamedByTests(): Map<string, string[]> {
  const named = new Map<string, string[]>();
  const add = (id: string, file: string) => {
    const files = named.get(id) ?? [];
    if (!files.includes(file)) files.push(file);
    named.set(id, files);
  };
  for (const root of ROOTS) {
    for (const file of listFiles(path.resolve(root.dir), root.isTest)) {
      if (file === THIS_FILE) continue;
      const text = readSourceText(file);
      const relative = repoRelativePath(file);
      for (const match of text.matchAll(TS_ID_RE)) add(match[0], relative);
      if (file.endsWith(".rs")) {
        for (const match of text.matchAll(RUST_ID_RE)) {
          add(`UX-${match[1].toUpperCase()}-${match[2]}`, relative);
        }
      }
    }
  }
  return named;
}

describe("UX spec coverage (docs/ux/spec.md)", () => {
  const rules = specRules();
  const named = idsNamedByTests();

  it("defines rules in the expected format", () => {
    expect(rules.size).toBeGreaterThan(0);
  });

  it("has a test for every shipped rule", () => {
    const untested = [...rules]
      .filter(([id, status]) => status === "shipped" && !named.has(id))
      .map(([id]) => id);
    expect(untested, "shipped rules need a test whose name contains the id").toEqual([]);
  });

  it("has no test naming an id the spec does not define", () => {
    const unknown = [...named]
      .filter(([id]) => !rules.has(id))
      .map(([id, files]) => `${id} in ${files.join(", ")}`);
    expect(unknown, "add the rule to docs/ux/spec.md or fix the id").toEqual([]);
  });
});
