import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const rule = (css: string, selector: string) => {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = css.match(new RegExp(`(?:^|\\n)${escaped}\\s*\\{([^}]*)\\}`));
  expect(match, selector).not.toBeNull();
  return match![1];
};

describe("the map's own layers stay under menus opened from outside it", () => {
  const atlasCss = readFileSync(resolve("src/components/memory/atlasControls.css"), "utf8");
  const identityCss = readFileSync(resolve("src/components/memory/IdentityCard.css"), "utf8");

  it("keeps the map's bottom lines, notes and controls inside the map's own layer", () => {
    // The count line, the night line, the corner note and the controls all set
    // a z-index. Without a layer of its own the map would lift them above the
    // account menu that opens from the sidebar, which sets a smaller one.
    expect(rule(atlasCss, ".atlas-canvas-shell")).toMatch(/isolation:\s*isolate/);
    for (const selector of [".atlas-count-line", ".atlas-night-line"]) {
      expect(rule(atlasCss, selector)).toMatch(/z-index:\s*\d+/);
    }
  });

  it("gives the account menu a layer above the map", () => {
    const zIndex = Number(rule(identityCss, ".identity-rail-menu").match(/z-index:\s*(\d+)/)?.[1]);
    expect(zIndex).toBeGreaterThan(0);
  });
});
