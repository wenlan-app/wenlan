// SPDX-License-Identifier: AGPL-3.0-only
// Run with: node --test scripts/gen-directory-plugin.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { GenError } from "./gen-codex-plugin.mjs";
import { applyReplaceRuns, directoryViolations, resolverProse, swapResolverFences, transformFile } from "./gen-directory-plugin.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const script = join(repo, "scripts/gen-directory-plugin.mjs");

function tempTree() {
  const root = mkdtempSync(join(tmpdir(), "gen-directory-plugin-"));
  for (const f of ["plugin", "plugin-agents", "plugin-directory", "plugin-contract.json", "app/icons/128x128.png"]) {
    mkdirSync(dirname(join(root, f)), { recursive: true });
    cpSync(join(repo, f), join(root, f), { recursive: true });
  }
  return root;
}

const run = (root, ...args) => spawnSync(process.execPath, [script, "--root", root, ...args], { encoding: "utf8" });

test("the committed tree is current and follows the directory rules", () => {
  const r = spawnSync(process.execPath, [script, "--check"], { encoding: "utf8" });
  assert.equal(r.status, 0, r.stderr);
  assert.deepEqual(directoryViolations(repo), []);
});

test("the resolver fence becomes prose that keeps every variable and step", () => {
  const fence = [
    'raw_args="<the full argument string passed to /wenlan-recall>"',
    'resolved="$("{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"',
    `space="$(printf '%s\\n' "$resolved" | cut -f1)"`,
    `source_layer="$(printf '%s\\n' "$resolved" | cut -f2)"`,
    `query="$(printf '%s\\n' "$resolved" | cut -f3-)"`,
  ];
  const text = resolverProse(fence, "").join("\n");
  for (const label of ["locked-env", "`arg`", "default-env", "cwd-config", "cwd-repo", "unscoped"]) {
    assert.ok(text.includes(label), label);
  }
  assert.ok(text.includes("/wenlan:recall"));
  assert.match(text, /`space` is the name/);
  assert.match(text, /`source_layer` is the label/);
  assert.match(text, /`query` is\s+the arguments/);
  assert.ok(!text.includes("cwd-repo-new"));
  assert.ok(!text.includes("```"));
});

test("--new-repo-fallback, --arg and extra fence lines carry over", () => {
  const text = swapResolverFences(
    "x.md",
    [
      "  ```bash",
      '  resolved="$("{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh" --cwd "$PWD" --arg "$a" --new-repo-fallback 2>/dev/null)"',
      `  space="$(printf '%s\\n' "$resolved" | cut -f1)"`,
      '  repo_root="$(git -C "$PWD" rev-parse --show-toplevel)"',
      "  ```",
    ].join("\n"),
  );
  assert.ok(text.includes("cwd-repo-new"));
  assert.ok(text.includes("A Space name the user passed"));
  assert.ok(text.includes('  repo_root="$(git -C "$PWD" rev-parse --show-toplevel)"'));
  assert.ok(!text.includes("resolve-space"));
  for (const line of text.split("\n").filter(Boolean)) assert.ok(line.startsWith("  "), line);
});

test("a replace run that does not match exactly once fails", () => {
  const rule = { file: "a.md", from: ["x"], to: ["y"] };
  assert.throws(() => applyReplaceRuns("a.md", "x\nx", [rule]), GenError);
  assert.throws(() => applyReplaceRuns("a.md", "z", [rule]), GenError);
  assert.equal(applyReplaceRuns("a.md", "x\nz", [rule]), "y\nz");
});

test("skill names and commands use the plugin form, and leftovers fail", () => {
  const out = transformFile("demo/SKILL.md", "---\nname: wenlan-demo\n---\nRun `/wenlan-brief` or `/wenlan-recall`.\n");
  assert.match(out, /^name: demo$/m);
  assert.ok(out.includes("`/wenlan:brief` or `/wenlan:recall`"));
  assert.throws(() => transformFile("demo/SKILL.md", "---\nname: x\n---\nCall mcp__plugin_wenlan_wenlan__brief\n"), GenError);
});

test("--check fails on a forbidden file, a #! file, allowed-tools and a stale file", () => {
  const root = tempTree();
  try {
    assert.equal(run(root, "--check").status, 0);

    writeFileSync(join(root, "plugin-directory/run.sh"), "echo hi\n");
    let r = run(root, "--check");
    assert.equal(r.status, 1);
    assert.match(r.stderr, /run\.sh: file type not allowed/);
    rmSync(join(root, "plugin-directory/run.sh"));

    writeFileSync(join(root, "plugin-directory/notes.md"), "#!/bin/sh\n");
    r = run(root, "--check");
    assert.match(r.stderr, /notes\.md: starts with #!/);
    rmSync(join(root, "plugin-directory/notes.md"));

    const skill = join(root, "plugin-directory/skills/brief/SKILL.md");
    const original = readFileSync(skill, "utf8");
    writeFileSync(skill, original.replace("---\n", "---\nallowed-tools: Read, Bash\n"));
    r = run(root, "--check");
    assert.match(r.stderr, /brief\/SKILL\.md: allowed-tools beyond/);
    assert.match(r.stderr, /brief\/SKILL\.md is out of date/);

    assert.equal(run(root).status, 0);
    assert.equal(run(root, "--check").status, 0);
    assert.equal(readFileSync(skill, "utf8"), original);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a Claude skill edit flows through to the directory edition", () => {
  const root = tempTree();
  try {
    const src = join(root, "plugin/skills/recall/SKILL.md");
    writeFileSync(src, `${readFileSync(src, "utf8")}\nA new closing line for the test.\n`);
    assert.equal(run(root, "--check").status, 1);
    assert.equal(run(root).status, 0);
    assert.ok(readFileSync(join(root, "plugin-directory/skills/recall/SKILL.md"), "utf8").includes("A new closing line for the test."));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
