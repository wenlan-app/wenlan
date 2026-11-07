// SPDX-License-Identifier: AGPL-3.0-only
// The daemon checks a custom theme's `advanced` token overrides against
// crates/wenlan-core/src/theme/known_tokens.txt. That list must name exactly
// the tokens index.css defines, or a theme could not set a real token (or
// could set one that no longer exists). Regenerate with
// `node scripts/gen-theme-tokens.mjs`.
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

describe("custom theme token list", () => {
  it("matches the tokens index.css defines", () => {
    const css = readFileSync(resolve("src/index.css"), "utf8");
    const fromCss = [
      ...new Set([...css.matchAll(/(--(?:mem|kg|canvas)-[a-z0-9-]+)\s*:/g)].map((m) => m[1])),
    ].sort();
    const listed = readFileSync(resolve("crates/wenlan-core/src/theme/known_tokens.txt"), "utf8")
      .split("\n")
      .filter(Boolean);
    expect(listed).toEqual(fromCss);
  });
});
