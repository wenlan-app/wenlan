// SPDX-License-Identifier: AGPL-3.0-only
// Run with: node --test scripts/gen-plugin-verbs.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const script = join(repo, "scripts/gen-plugin-verbs.mjs");
const FILES = [
  "plugin/skills/verbs.json",
  "plugin/README.md",
  "plugin/skills/README.md",
];

function tempTree() {
  const root = mkdtempSync(join(tmpdir(), "gen-plugin-verbs-"));
  for (const f of FILES) {
    mkdirSync(dirname(join(root, f)), { recursive: true });
    cpSync(join(repo, f), join(root, f));
  }
  return root;
}

const run = (root, ...flags) =>
  spawnSync(process.execPath, [script, "--root", root, ...flags], { encoding: "utf8" });

test("generation is idempotent", () => {
  const root = tempTree();
  try {
    assert.equal(run(root).status, 0);
    const first = FILES.map((f) => readFileSync(join(root, f), "utf8"));
    const second = run(root);
    assert.equal(second.status, 0);
    assert.equal(second.stdout, "", "second run must not rewrite anything");
    assert.deepEqual(FILES.map((f) => readFileSync(join(root, f), "utf8")), first);
    assert.equal(run(root, "--check").status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("--check fails on a hand edit inside the markers", () => {
  const root = tempTree();
  try {
    run(root);
    const file = join(root, "plugin/README.md");
    const text = readFileSync(file, "utf8");
    writeFileSync(file, text.replace("search local memory", "search everything"));
    const res = run(root, "--check");
    assert.equal(res.status, 1);
    assert.match(res.stderr, /plugin\/README\.md is out of date/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("every skill dir has a verb entry and vice versa", () => {
  const verbs = JSON.parse(readFileSync(join(repo, "plugin/skills/verbs.json"), "utf8"));
  const fromVerbs = verbs.map((v) => v.skill).sort();
  const dirs = readdirSync(join(repo, "plugin/skills"), { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => e.name)
    .sort();
  assert.deepEqual(fromVerbs, dirs);
});

test("a bare --root is a usage error, not a crash", () => {
  const res = spawnSync(process.execPath, [script, "--root"], { encoding: "utf8" });
  assert.equal(res.status, 64);
  assert.match(res.stderr, /usage:/);
});

test("menu entries are exactly the user-invocable skills", () => {
  const verbs = JSON.parse(readFileSync(join(repo, "plugin/skills/verbs.json"), "utf8"));
  for (const v of verbs) {
    const text = readFileSync(join(repo, "plugin/skills", v.skill, "SKILL.md"), "utf8");
    const hidden = /^user-invocable: false$/m.test(text);
    assert.equal(hidden, !v.menu, `${v.skill}: menu=${v.menu} but hidden=${hidden}`);
    assert.equal(v.verb.startsWith("/wenlan "), !v.menu, `${v.skill}: hidden verbs are reached through /wenlan`);
  }
});

test("the real tree is in sync and the docs keep the lint grammar", () => {
  assert.equal(run(repo, "--check").status, 0);
  for (const f of ["plugin/README.md", "plugin/skills/README.md"]) {
    assert.ok(readFileSync(join(repo, f), "utf8").includes("/wenlan lint [deep"), f);
  }
});
