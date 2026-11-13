#!/usr/bin/env node
// SPDX-License-Identifier: AGPL-3.0-only
// `pnpm dev:team`: a team server for development, the way Docker runs one
// (`wenlan-server team <address>`), on this computer and apart from
// everything real. Its data lives in a scratch folder, its ports are not the
// installed Wenlan's, and it never reads your AI tools' own folders. The
// owner's invite link it prints on first start joins it from `pnpm dev:all`
// (Settings › Team › Join with an invite link).
//
//   pnpm dev:team            start (Ctrl+C stops it; the team stays)
//   pnpm dev:team -- --fresh start over with a new, empty team
import { spawn } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const DAEMON_PORT = "17978";
const TEAM_PORT = "17879";
// Always this one folder: `--fresh` deletes it, so it is never a path from outside.
const root = join(tmpdir(), "wenlan-dev-team");

if (process.argv.includes("--fresh")) rmSync(root, { recursive: true, force: true });
const data = join(root, "data");
const claude = join(root, "claude");
const codex = join(root, "codex");
for (const dir of [data, claude, codex]) mkdirSync(dir, { recursive: true });

const url = `http://127.0.0.1:${TEAM_PORT}`;
console.log(`Team server for development: ${url}  (data in ${data})`);
const child = spawn(
  "cargo",
  ["run", "-p", "wenlan-server", "--", "team", url, "--listen", `127.0.0.1:${TEAM_PORT}`],
  {
    stdio: "inherit",
    env: {
      ...process.env,
      WENLAN_DATA_DIR: data,
      WENLAN_PORT: DAEMON_PORT,
      WENLAN_NO_AUTOSTART: "1",
      // An empty Claude Code and Codex home: the server must not follow the
      // developer's own AI memory into a test team.
      CLAUDE_CONFIG_DIR: claude,
      CODEX_HOME: codex,
    },
  },
);
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => child.kill(signal));
child.on("exit", (code, signal) => process.exit(signal ? 1 : code ?? 1));
