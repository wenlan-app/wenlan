import { pdfAssets } from "./vite.pdf";
// SPDX-License-Identifier: AGPL-3.0-only
// Browser rendering of the app with Tauri APIs shimmed.
// Run: pnpm exec vite --config tooling/vite.preview.config.ts
//   http://localhost:1421/          — live app (invoke → daemon on :7878)
//   http://localhost:1421/preview/  — fixture harness (citation states)
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";

const mock = (p: string) => fileURLToPath(new URL(p, import.meta.url));
const stubs = mock("../src/web/tauri-stubs.ts");

// Verification must never run against the shared prod daemon on :7878 (dev and
// prod share the platform data dir). Point this at an isolated instance with
// `WENLAN_PREVIEW_DAEMON=http://127.0.0.1:17878`.
const daemonTarget =
  process.env.WENLAN_PREVIEW_DAEMON ?? "http://127.0.0.1:7878";

// Real source mutations are only allowed against an explicitly isolated daemon.
const sourcePreview = process.env.WENLAN_PREVIEW_SOURCE_WRITES === "1";
if (sourcePreview) {
  const target = new URL(daemonTarget);
  if (target.protocol !== "http:" || !["127.0.0.1", "localhost", "[::1]"].includes(target.hostname) || !target.port || target.port === "7878") {
    throw new Error("Source preview writes require an isolated loopback daemon port");
  }
}

// The isolated daemon believes the person marker only next to its person
// key. With `WENLAN_PREVIEW_DATA_DIR` set to that daemon's data root, the
// proxy adds the key to the preview's person gestures, as the app does, but
// only for a scratch daemon: an isolated loopback port (not 7878) and a data
// root that is not the installed app's. Otherwise this proxy would hand any
// local program the real person's key.
const PRODUCTION_DATA_ROOTS = (() => {
  const home = homedir();
  const bases = [
    join(home, ".local", "share"),
    join(home, "Library", "Application Support"),
    process.env.LOCALAPPDATA ?? "",
    process.env.XDG_DATA_HOME ?? "",
  ].filter(Boolean);
  return bases.flatMap((base) => [join(base, "wenlan"), join(base, "origin")]).map((p) => resolve(p));
})();

export function isScratchPreview(target: string, dataRoot: string | undefined): dataRoot is string {
  if (!dataRoot || !isAbsolute(dataRoot)) return false;
  const url = new URL(target);
  const loopback = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (url.protocol !== "http:" || !loopback || !url.port || url.port === "7878") return false;
  const root = resolve(dataRoot);
  return !PRODUCTION_DATA_ROOTS.includes(root) && !root.startsWith(join(homedir(), ".wenlan"));
}

function previewPersonKey(): string | null {
  const root = process.env.WENLAN_PREVIEW_DATA_DIR;
  if (!isScratchPreview(daemonTarget, root)) return null;
  try {
    const key = readFileSync(join(root, "person.key"), "utf8").trim();
    return /^[0-9a-f]{64}$/.test(key) ? key : null;
  } catch {
    return null;
  }
}

export default defineConfig({
  ...(process.env.WENLAN_VITE_CACHE_DIR ? { cacheDir: process.env.WENLAN_VITE_CACHE_DIR } : {}),
  plugins: [react(), tailwindcss(), pdfAssets()],
  clearScreen: false,
  // `RuntimeOverlays` reads this at module scope, so without it the whole app
  // throws on first render and the preview shows a blank page. Matches
  // `vite.config.ts`; the review harness is the only config that sets it true.
  define: {
    __WENLAN_REVIEW__: "false",
    __WENLAN_WEB__: "false",
    __WENLAN_SOURCE_PREVIEW__: JSON.stringify(sourcePreview),
  },
  resolve: {
    alias: {
      "@tauri-apps/api/core": mock("../preview/mocks/core.ts"),
      "@tauri-apps/plugin-shell": mock("../src/web/shell.ts"),
      "@tauri-apps/api/event": stubs,
      "@tauri-apps/api/app": stubs,
      "@tauri-apps/api/window": stubs,
      "@tauri-apps/api/webviewWindow": stubs,
      "@tauri-apps/plugin-dialog": stubs,
      "@tauri-apps/plugin-fs": stubs,
      "@tauri-apps/plugin-notification": stubs,
      "tauri-plugin-clipboard-x-api": stubs,
    },
  },
  server: {
    port: 1421,
    strictPort: true,
    open: false,
    proxy: {
      "/daemon": {
        target: daemonTarget,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/daemon/, ""),
        configure: (proxy) => {
          proxy.on("proxyReq", (outgoing, incoming) => {
            const key = incoming.headers["x-wenlan-caller"] === "person" ? previewPersonKey() : null;
            if (key) outgoing.setHeader("x-wenlan-person-key", key);
          });
        },
      },
    },
    // Polling: edits from the sandboxed agent don't emit fsevents.
    watch: { ignored: ["**/app/**", "**/target/**", "**/crates/**", "**/scripts/**", "**/*.test.ts", "**/*.test.tsx"], usePolling: true, interval: 400 },
  },
});
