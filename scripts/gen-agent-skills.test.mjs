// SPDX-License-Identifier: AGPL-3.0-only
// Run with: node --test scripts/gen-agent-skills.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { bundleVersion, generateReference, generateSkill, renderBundle } from "./gen-agent-skills.mjs";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const script = join(repo, "scripts/gen-agent-skills.mjs");
const SKILLS = "plugin-agents/skills";
const BUNDLE = "app/src/agent_skills_bundle.rs";

function tempTree() {
  const root = mkdtempSync(join(tmpdir(), "gen-agent-skills-"));
  for (const f of ["plugin", "plugin-agents", "plugin-contract.json", BUNDLE]) {
    mkdirSync(dirname(join(root, f)), { recursive: true });
    cpSync(join(repo, f), join(root, f), { recursive: true });
  }
  return root;
}

const run = (root, ...flags) =>
  spawnSync(process.execPath, [script, "--root", root, ...flags], { encoding: "utf8" });

const withTree = (fn) => {
  const root = tempTree();
  try {
    fn(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
};

const contract = JSON.parse(readFileSync(join(repo, "plugin-contract.json"), "utf8"));
const read = (rel) => readFileSync(join(repo, rel), "utf8");
const claudeSkill = (name) => read(`plugin/skills/${name}/SKILL.md`);

function walk(dir) {
  const out = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) out.push(...walk(p));
    else out.push(p);
  }
  return out;
}
const treeFiles = () => walk(join(repo, SKILLS)).map((p) => relative(join(repo, SKILLS), p).split("\\").join("/")).sort();

// A minimal Claude skill for unit-level transform checks.
const SAMPLE = `---
name: sample
description: >
  Sample skill. Invoked as \`/sample\` or \`/brief\`.
argument-hint: "<x>"
allowed-tools: ["mcp__plugin_wenlan_wenlan__recall", "AskUserQuestion", "Skill", "Bash"]
---

# /sample

Run \`/wenlan:brief\` or \`/capture\` again if it fails. See \`/wenlan lint\`.

\`\`\`bash
raw_args="<args>"
resolved="$("\${CLAUDE_PLUGIN_ROOT}/scripts/resolve-space.sh" --cwd "$PWD" --args "$raw_args")"
\`\`\`

## Keep

Call \`mcp__plugin_wenlan_wenlan__recall\` first, then
\`mcp__plugin_wenlan_wenlan__capture(content="x")\`.

\`\`\`text
mcp__plugin_wenlan_wenlan__brief(
  space="s"
)
\`\`\`

## Drop me

\`\`\`text
## not a heading, inside a fence
\`\`\`

Gone for other tools.

## Replace me

Old line one.
Old line two.
`;

test("real tree: every generated file and the Rust bundle are in sync", () => {
  const res = run(repo, "--check");
  assert.equal(res.status, 0, res.stderr);
});

test("generation is idempotent and --check agrees", () => {
  withTree((root) => {
    const first = run(root);
    assert.equal(first.status, 0, first.stderr);
    assert.equal(first.stdout, "", "a clean copy of the repo must need no rewrite");
    assert.equal(run(root, "--check").status, 0);
  });
});

test("--check fails on a hand edit to a generated skill and names the file", () => {
  withTree((root) => {
    const file = join(root, SKILLS, "wenlan-brief/SKILL.md");
    writeFileSync(file, readFileSync(file, "utf8").replace("# /wenlan-brief", "# /wenlan-brief (edited)"));
    const res = run(root, "--check");
    assert.equal(res.status, 1);
    assert.match(res.stderr, /plugin-agents\/skills\/wenlan-brief\/SKILL\.md is out of date/);
    assert.match(res.stderr, /run: node scripts\/gen-agent-skills\.mjs/);
  });
});

test("--check fails on a hand edit to the Rust bundle; a write run restores it", () => {
  withTree((root) => {
    const file = join(root, BUNDLE);
    const expected = readFileSync(file, "utf8");
    writeFileSync(file, expected.replace(/BUNDLE_VERSION: &str = "[0-9a-f]+"/, 'BUNDLE_VERSION: &str = "0000000000000000"'));
    const res = run(root, "--check");
    assert.equal(res.status, 1);
    assert.match(res.stderr, /app\/src\/agent_skills_bundle\.rs is out of date/);
    const write = run(root);
    assert.equal(write.status, 0, write.stderr);
    assert.match(write.stdout, /updated app\/src\/agent_skills_bundle\.rs/);
    assert.equal(readFileSync(file, "utf8"), expected);
  });
});

test("a change to a Claude skill changes the bundle version and fails --check until regenerated", () => {
  withTree((root) => {
    const before = readFileSync(join(root, BUNDLE), "utf8").match(/BUNDLE_VERSION: &str = "([0-9a-f]{16})"/)[1];
    const file = join(root, "plugin/skills/recall/SKILL.md");
    writeFileSync(file, readFileSync(file, "utf8").replace("Hint: write specific queries", "Hint: write very specific queries"));
    assert.equal(run(root, "--check").status, 1);
    assert.equal(run(root).status, 0);
    const after = readFileSync(join(root, BUNDLE), "utf8").match(/BUNDLE_VERSION: &str = "([0-9a-f]{16})"/)[1];
    assert.notEqual(after, before);
    assert.equal(run(root, "--check").status, 0);
  });
});

test("an overlay `from` run that no longer matches the Claude skill is a hard error", () => {
  withTree((root) => {
    const file = join(root, "plugin/skills/setup/SKILL.md");
    writeFileSync(file, readFileSync(file, "utf8").replace("- Right after `/plugin install wenlan@7xuanlu-wenlan`.", "- Right after installing."));
    const res = run(root, "--check");
    assert.equal(res.status, 2);
    assert.match(res.stderr, /setup: replace\[\d+\] matched 0 runs/);
  });
});

test("a Claude-only token the overlay did not remove fails generation", () => {
  withTree((root) => {
    const file = join(root, "plugin/skills/forget/SKILL.md");
    writeFileSync(file, readFileSync(file, "utf8").replace("## Call", "## Call (never via AskUserQuestion)"));
    const res = run(root, "--check");
    assert.equal(res.status, 2);
    assert.match(res.stderr, /forget: generated file still contains "AskUserQuestion"/);
  });
});

test("--check reports a file nothing generates, and a write run deletes it", () => {
  withTree((root) => {
    const stale = join(root, SKILLS, "wenlan-lint/SKILL.md");
    mkdirSync(dirname(stale), { recursive: true });
    writeFileSync(stale, "old skill");
    const check = run(root, "--check");
    assert.equal(check.status, 1);
    assert.match(check.stderr, /plugin-agents\/skills\/wenlan-lint\/SKILL\.md is not generated by any skill; delete it/);
    const write = run(root);
    assert.equal(write.status, 0, write.stderr);
    assert.match(write.stdout, /deleted plugin-agents\/skills\/wenlan-lint\/SKILL\.md/);
    assert.ok(!existsSync(join(root, SKILLS, "wenlan-lint")), "the emptied directory is pruned");
    assert.equal(run(root, "--check").status, 0);
  });
});

test("an overlay entry for a skill the contract does not list fails generation", () => {
  withTree((root) => {
    const file = join(root, "plugin-agents/overlay.json");
    const o = JSON.parse(readFileSync(file, "utf8"));
    o.skills.nonesuch = { replace: [] };
    writeFileSync(file, JSON.stringify(o));
    const res = run(root, "--check");
    assert.equal(res.status, 2);
    assert.match(res.stderr, /names skills not generated from plugin-contract\.json: nonesuch/);
  });
});

test("a bare --root is a usage error, not a crash", () => {
  const res = spawnSync(process.execPath, [script, "--root"], { encoding: "utf8" });
  assert.equal(res.status, 64);
  assert.match(res.stderr, /usage:/);
});

test("skill: frontmatter is exactly a prefixed name and one quoted description line", () => {
  const text = generateSkill("sample", SAMPLE, {}, { ...contract });
  const [open, name, description, close] = text.split("\n");
  assert.equal(open, "---");
  assert.equal(name, "name: wenlan-sample");
  assert.equal(description, 'description: "Sample skill. Invoked as `/sample` or `/wenlan-brief`."');
  assert.equal(close, "---");
  assert.ok(!text.includes("allowed-tools") && !text.includes("argument-hint"));
});

test("skill: the front door keeps the name `wenlan`", () => {
  const src = SAMPLE.replace("name: sample", "name: wenlan");
  assert.match(generateSkill("wenlan", src, {}, contract), /^---\nname: wenlan\n/);
});

test("skill: description override is folded onto one line", () => {
  const text = generateSkill("sample", SAMPLE, { description: ["Line one.", "Line two."] }, contract);
  assert.match(text, /\ndescription: "Line one\. Line two\."\n---/);
});

test("skill: a description over 1024 characters fails generation", () => {
  assert.throws(
    () => generateSkill("sample", SAMPLE, { description: ["x".repeat(1025)] }, contract),
    /description is 1025 characters; the limit is 1024/,
  );
});

test("body: resolver path, MCP tool names and skill commands become host-neutral", () => {
  const text = generateSkill("sample", SAMPLE, {}, contract);
  assert.ok(text.includes('resolved="$("{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh" --cwd'));
  assert.ok(text.includes("Call the Wenlan `recall` tool first, then\n`capture(content=\"x\")`."));
  assert.ok(text.includes("```text\nbrief(\n  space=\"s\"\n)\n```"));
  assert.ok(text.includes("Run `/wenlan-brief` or `/wenlan-capture` again if it fails. See `/wenlan lint`."));
  for (const token of ["CLAUDE_PLUGIN_ROOT", "mcp__", "AskUserQuestion", "/wenlan:"]) {
    assert.ok(!text.includes(token), `leftover ${token}`);
  }
});

test("body: command renaming leaves /wenlan, paths and longer words alone", () => {
  const src = SAMPLE.replace(
    "Gone for other tools.",
    "Gone.\n\n## Entry\n\nType `/wenlan`. Notes live in `~/.wenlan/pages/`; read docs/capture.md, /capture/x and /recall-all; then /handoff.",
  );
  const text = generateSkill("sample", src, {}, contract);
  assert.ok(text.includes("Type `/wenlan`."));
  assert.ok(text.includes("`~/.wenlan/pages/`; read docs/capture.md, /capture/x and /recall-all; then /wenlan-handoff."));
});

test("guard: a Claude or Codex word, an unprefixed resolver or a stray MCP prefix fail generation", () => {
  const cases = [
    ["ask Claude Code", /still contains "Claude"/],
    ["ask Codex", /still contains "Codex"/],
    ["use `$wenlan`", /still contains "\$wenlan"/],
    ["agent name claude-code", /still contains "claude-code"/],
    ["install with `npx -y wenlan@0.1.0 setup`", /still contains "npx -y wenlan@"/],
    ["call mcp__wenlan__recall", /still contains "mcp__"/],
    ["start an `Agent` for it", /still contains "`Agent`"/],
    ["run ./scripts/resolve-space.sh now", /names a resolve-space\.sh path without/],
  ];
  for (const [needle, re] of cases) {
    const src = SAMPLE.replace("Gone for other tools.", `Gone.\n\n## Stay\n\n${needle}`);
    assert.throws(() => generateSkill("sample", src, {}, contract), re, needle);
  }
});

test("sections and replace: null drops, arrays swap, misses fail", () => {
  const text = generateSkill(
    "sample",
    SAMPLE,
    {
      sections: { "## Drop me": null },
      replace: [{ from: ["Old line one.", "Old line two."], to: ["New line."] }],
    },
    contract,
  );
  assert.ok(!text.includes("## Drop me") && !text.includes("## not a heading"));
  assert.match(text, /## Replace me\n\nNew line\.\n$/);
  assert.throws(
    () => generateSkill("sample", SAMPLE, { replace: [{ from: ["Nope."], to: null }] }, contract),
    /replace\[0\] matched 0 runs/,
  );
});

test("reference: no frontmatter, same body transforms, hidden flag required", () => {
  const hidden = SAMPLE.replace('argument-hint: "<x>"', 'argument-hint: "<x>"\nuser-invocable: false');
  const ref = generateReference("sample", hidden, {}, contract);
  assert.ok(ref.startsWith("# /sample\n"), "starts at the body");
  assert.ok(ref.includes("Call the Wenlan `recall` tool first"));
  assert.ok(ref.includes("{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh"));
  assert.throws(() => generateReference("sample", hidden, { description: ["x"] }, contract), /drop its overlay description/);
  assert.throws(() => generateReference("sample", SAMPLE, {}, contract), /must set user-invocable: false/);
});

test("real tree: the output holds exactly the front door, four skills, eight references and the resolver", () => {
  const hidden = contract.skills.filter((s) => s.status === "hidden_routed").map((s) => s.name);
  const expected = [
    "wenlan-brief/SKILL.md",
    "wenlan-capture/SKILL.md",
    "wenlan-handoff/SKILL.md",
    "wenlan-recall/SKILL.md",
    "wenlan/SKILL.md",
    "wenlan/scripts/resolve-space.sh",
    ...hidden.map((h) => `wenlan/references/${h}.md`),
  ].sort();
  assert.deepEqual(treeFiles(), expected);
  assert.equal(hidden.length, 8);
});

test("real tree: every SKILL.md is valid Agent Skills with name equal to its folder", () => {
  for (const file of treeFiles().filter((f) => f.endsWith("/SKILL.md"))) {
    const folder = file.split("/")[0];
    const text = read(`${SKILLS}/${file}`);
    const m = text.match(/^---\nname: (.+)\ndescription: (".*")\n---\n/);
    assert.ok(m, `${file}: frontmatter is exactly name and description`);
    assert.equal(m[1], folder, `${file}: name equals folder`);
    assert.match(m[1], /^[a-z0-9]+(-[a-z0-9]+)*$/);
    assert.ok(m[1].length <= 64);
    const description = JSON.parse(m[2]);
    assert.ok(description.length > 0 && description.length <= 1024, `${file}: description length ${description.length}`);
  }
  for (const file of treeFiles().filter((f) => f.includes("/references/"))) {
    assert.ok(!read(`${SKILLS}/${file}`).startsWith("---"), `${file}: a reference has no frontmatter`);
  }
});

test("real tree: no Claude or Codex only wording survives anywhere", () => {
  const banned = [
    /CLAUDE_PLUGIN_ROOT/,
    /\.claude-plugin/,
    /AskUserQuestion/,
    /mcp__/,
    /\$wenlan/,
    /\bClaude\b/,
    /\bCodex\b/i,
    /claude-code/,
    /\/wenlan:/,
    /Skill tool/,
    /allowed-tools/,
    /user-invocable/,
    /\bBash\b/,
    /plugin/i,
    /version-drift/,
  ];
  for (const file of treeFiles().filter((f) => f.endsWith(".md"))) {
    const text = read(`${SKILLS}/${file}`);
    for (const re of banned) assert.ok(!re.test(text), `${file} matches ${re}`);
  }
});

test("real tree: every resolver call uses the placeholder path, which the script ships", () => {
  let calls = 0;
  for (const file of treeFiles().filter((f) => f.endsWith(".md"))) {
    for (const line of read(`${SKILLS}/${file}`).split("\n")) {
      if (!line.includes("/resolve-space.sh")) continue;
      calls++;
      assert.ok(line.includes('"{{WENLAN_SKILLS_DIR}}/wenlan/scripts/resolve-space.sh"'), `${file}: ${line}`);
    }
  }
  assert.ok(calls >= 8, `expected the workflows to call the resolver, found ${calls}`);
  assert.equal(read(`${SKILLS}/wenlan/scripts/resolve-space.sh`), read("plugin/scripts/resolve-space.sh"));
  assert.ok(statSync(join(repo, "plugin/scripts/resolve-space.sh")).mode & 0o100, "the source script is executable");
});

test("real tree: the front door routes every hidden workflow to its reference", () => {
  const door = read(`${SKILLS}/wenlan/SKILL.md`);
  assert.ok(door.includes("`references/<workflow>.md` beside this SKILL.md"));
  assert.ok(door.includes("use the `wenlan-<workflow>` skill"));
  for (const word of ["setup", "distill", "steep", "pages", "lint", "curate", "forget", "wiki"]) {
    assert.ok(existsSync(join(repo, SKILLS, "wenlan/references", `${word}.md`)), word);
  }
});

test("real tree: no release version is baked in, so a release bump needs no regeneration", () => {
  const version = read("version.txt").trim();
  for (const file of treeFiles().filter((f) => f.endsWith(".md"))) {
    const text = read(`${SKILLS}/${file}`);
    assert.ok(!text.includes(version), `${file} names release ${version}`);
    assert.ok(!/install\.sh|npx -y wenlan/.test(text), `${file} runs an installer`);
  }
});

test("a release bump of the Claude setup pin and plugin manifest needs no overlay edit", () => {
  withTree((root) => {
    const bump = (rel, from, to) => {
      const file = join(root, rel);
      const text = readFileSync(file, "utf8");
      assert.ok(text.includes(from), `${rel} has ${from}`);
      writeFileSync(file, text.replaceAll(from, to));
    };
    const version = JSON.parse(readFileSync(join(root, "plugin/.claude-plugin/plugin.json"), "utf8")).version;
    bump("plugin/skills/setup/SKILL.md", `wenlan@${version} setup`, "wenlan@99.1.2 setup");
    bump("plugin/.claude-plugin/plugin.json", `"version": "${version}"`, '"version": "99.1.2"');
    assert.equal(run(root, "--check").status, 0);
  });
});

test("real tree: curate and pages ask for typed replies, not native pickers", () => {
  const curate = read(`${SKILLS}/wenlan/references/curate.md`);
  for (const needle of ["Perform no mutation until the user replies", "Ambiguous replies do not mutate", "revision_source_id"]) {
    assert.ok(curate.includes(needle), needle);
  }
  assert.ok(read(`${SKILLS}/wenlan/references/pages.md`).includes("Do not use a picker"));
});

test("bundle: FILES lists every generated file sorted by path, with the content hash as version", () => {
  const rs = read(BUNDLE);
  const paths = [...rs.matchAll(/^ {8}"([^"]+)",\n {8}include_str!\("\.\.\/\.\.\/plugin-agents\/skills\/([^"]+)"\),$/gm)].map((m) => {
    assert.equal(m[1], m[2], "the path and the include_str! target agree");
    return m[1];
  });
  assert.deepEqual(paths, treeFiles());
  assert.deepEqual(paths, [...paths].sort());
  const version = rs.match(/pub const BUNDLE_VERSION: &str = "([0-9a-f]{16})";/)[1];
  assert.equal(version, bundleVersion(paths.map((path) => ({ path, text: read(`${SKILLS}/${path}`) }))));
  assert.ok(rs.startsWith("// SPDX-License-Identifier: AGPL-3.0-only\n"));
  assert.match(rs, /Generated by `scripts\/gen-agent-skills\.mjs`[^\n]*\n[^\n]*do not edit/);
  assert.match(rs, /pub const FILES: &\[\(&str, &str\)\] = &\[\n/);
});

test("bundle: the version hashes paths and contents, and renders the same for the same files", () => {
  const a = [{ path: "b/SKILL.md", text: "two" }, { path: "a/SKILL.md", text: "one" }];
  const reordered = [a[1], a[0]];
  assert.equal(bundleVersion(a), bundleVersion(reordered));
  assert.match(bundleVersion(a), /^[0-9a-f]{16}$/);
  assert.notEqual(bundleVersion(a), bundleVersion([{ ...a[0], text: "other" }, a[1]]));
  assert.notEqual(bundleVersion(a), bundleVersion([{ ...a[0], path: "c/SKILL.md" }, a[1]]));
  assert.equal(renderBundle(a), renderBundle(reordered));
  assert.ok(renderBundle(a).indexOf('"a/SKILL.md"') < renderBundle(a).indexOf('"b/SKILL.md"'));
});
