// SPDX-License-Identifier: AGPL-3.0-only
// @vitest-environment node
import { resolve } from "node:path";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { normalizePath, resolveConfig, type ResolvedConfig } from "vite";

const fixture = 'import { jsxDEV } from "react/jsx-dev-runtime"; export function Fixture() { return jsxDEV("div", {}); }';

async function transform(config: ResolvedConfig, id: string, code = fixture) {
  const hook = config.plugins.find((plugin) => plugin.name === "vite:react-babel")?.transform;
  if (!hook) throw new Error("React Babel transform must remain active for application sources");
  const handler = typeof hook === "function" ? hook : hook.handler;
  return handler.call({} as never, code, id, { ssr: false });
}

afterEach(() => vi.unstubAllEnvs());

beforeAll(async () => {
  // Load Babel once during setup, rather than charging its cold import to an
  // individual path-filter case's five-second assertion budget.
  const config = await resolveConfig({ configFile: resolve("vite.config.ts") }, "serve");
  await transform(config, normalizePath(resolve("src/Fixture.tsx")), "export function Fixture() { return <div />; }");
});

describe("worktree Vite cache", () => {
  for (const configured of [undefined, "", "target/vite cache [checks]+", resolve("target/absolute cache [checks]+")] as const) {
    it(`skips optimized dependencies in ${configured === undefined ? "the default cache" : configured === "" ? "an empty override" : configured} while preserving app transforms`, async () => {
      vi.stubEnv("WENLAN_VITE_CACHE_DIR", configured);
      const config = await resolveConfig({ configFile: resolve("vite.config.ts") }, "serve");
      const cache = normalizePath(resolve(configured || ".vite"));
      expect(normalizePath(config.cacheDir)).toBe(cache);
      expect(await transform(config, `${cache}/deps/prebuilt.js?v=fixture`), "optimized dependencies must bypass Babel").toBeUndefined();
      expect(await transform(config, `${cache}/deps/nested/prebuilt.js`)).toBeUndefined();
      expect(await transform(config, normalizePath(resolve("node_modules/prebuilt.js")))).toBeUndefined();
      expect(await transform(config, normalizePath(resolve("src/Fixture.tsx")), "export function Fixture() { return <div />; }")).toMatchObject({
        code: expect.stringContaining("$RefreshReg$"),
      });
      expect(await transform(config, `${cache}-other/Fixture.tsx`, "export function Fixture() { return <div />; }")).toMatchObject({
        code: expect.stringContaining("$RefreshReg$"),
      });
    });
  }
});
