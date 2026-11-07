// SPDX-License-Identifier: AGPL-3.0-only
// Writes the CSS token names a custom theme may override, one per line, from
// src/index.css into crates/wenlan-core/src/theme/known_tokens.txt. The daemon
// checks a theme's `advanced` overrides against this list; src/themeTokens.test.ts
// fails when the two drift. Run: node scripts/gen-theme-tokens.mjs
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export function themeTokenNames(css) {
  const names = new Set();
  for (const match of css.matchAll(/(--(?:mem|kg|canvas)-[a-z0-9-]+)\s*:/g)) names.add(match[1]);
  return [...names].sort();
}
export const KNOWN_TOKENS_PATH = resolve(root, "crates/wenlan-core/src/theme/known_tokens.txt");

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const css = readFileSync(resolve(root, "src/index.css"), "utf8");
  writeFileSync(KNOWN_TOKENS_PATH, `${themeTokenNames(css).join("\n")}\n`);
}
