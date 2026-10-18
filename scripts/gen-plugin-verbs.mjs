// SPDX-License-Identifier: AGPL-3.0-only
// Generates the slash-command lists in the plugin docs from plugin/skills/verbs.json.
// Rewrites the region between `<!-- verbs:start -->` and `<!-- verbs:end -->` in each
// target. `--check` exits 1 when a target differs; `--root <dir>` points at another tree.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const START = "<!-- verbs:start -->";
const END = "<!-- verbs:end -->";
const NOTE =
  "<!-- generated from plugin/skills/verbs.json by scripts/gen-plugin-verbs.mjs; do not edit -->";

const cmd = (v) => (v.args ? `${v.verb} ${v.args}` : v.verb);

const list = (verbs) => [
  NOTE,
  "",
  ...verbs.map((v) => `- \`${cmd(v)}\` - ${v.line}`),
];

const cell = (text) => text.replaceAll("|", "\\|");

const table = (verbs) => [
  NOTE,
  "",
  "| Command | Purpose |",
  "| --- | --- |",
  ...verbs.map((v) => `| \`${cell(cmd(v))}\` | ${cell(v.line)} |`),
];

const TARGETS = [
  { path: "plugin/README.md", render: list },
  { path: "plugin/skills/README.md", render: table },
];

function splice(text, generated, path) {
  const lines = text.split("\n");
  const start = lines.findIndex((l) => l.trim() === START);
  const end = lines.findIndex((l) => l.trim() === END);
  if (start < 0 || end < start) {
    throw new Error(`${path}: missing ${START} / ${END} markers`);
  }
  return [...lines.slice(0, start + 1), ...generated, ...lines.slice(end)].join("\n");
}

function diffHint(path, actual, expected) {
  const a = actual.split("\n");
  const e = expected.split("\n");
  const out = [`${path} is out of date with plugin/skills/verbs.json:`];
  for (let i = 0; i < Math.max(a.length, e.length); i++) {
    if (a[i] === e[i]) continue;
    out.push(`  line ${i + 1}:`, `  - ${a[i] ?? "<missing>"}`, `  + ${e[i] ?? "<missing>"}`);
    if (out.length > 13) {
      out.push("  ...");
      break;
    }
  }
  return out.join("\n");
}

function rootValue(argv, idx) {
  const value = argv[idx + 1];
  if (value === undefined || value.startsWith("--")) {
    console.error("usage: gen-plugin-verbs.mjs [--check] [--root <dir>]");
    process.exit(64);
  }
  return value;
}

const args = process.argv.slice(2);
const check = args.includes("--check");
const rootIdx = args.indexOf("--root");
const root =
  rootIdx >= 0
    ? resolve(rootValue(args, rootIdx))
    : resolve(dirname(fileURLToPath(import.meta.url)), "..");

const verbs = JSON.parse(readFileSync(resolve(root, "plugin/skills/verbs.json"), "utf8"));
let stale = 0;
for (const { path, render } of TARGETS) {
  const file = resolve(root, path);
  const actual = readFileSync(file, "utf8");
  const expected = splice(actual, render(verbs), path);
  if (actual === expected) continue;
  if (check) {
    stale++;
    console.error(diffHint(path, actual, expected));
  } else {
    writeFileSync(file, expected);
    console.log(`updated ${path}`);
  }
}
if (stale) {
  console.error("run: node scripts/gen-plugin-verbs.mjs");
  process.exit(1);
}
