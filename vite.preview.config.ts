import { pdfAssets } from "./vite.pdf";
// SPDX-License-Identifier: AGPL-3.0-only
// Browser rendering of the app with Tauri APIs shimmed.
// Run: pnpm exec vite --config vite.preview.config.ts
//   http://localhost:1421/          — live app (invoke → daemon on :7878)
//   http://localhost:1421/preview/  — fixture harness (citation states)
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

const mock = (p: string) => fileURLToPath(new URL(p, import.meta.url));
const stubs = mock("./preview/mocks/tauri-stubs.ts");

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

export default defineConfig({
  ...(process.env.WENLAN_VITE_CACHE_DIR ? { cacheDir: process.env.WENLAN_VITE_CACHE_DIR } : {}),
  plugins: [react(), tailwindcss(), pdfAssets()],
  clearScreen: false,
  // `RuntimeOverlays` reads this at module scope, so without it the whole app
  // throws on first render and the preview shows a blank page. Matches
  // `vite.config.ts`; the review harness is the only config that sets it true.
  define: {
    __WENLAN_REVIEW__: "false",
    __WENLAN_SOURCE_PREVIEW__: JSON.stringify(sourcePreview),
  },
  resolve: {
    alias: {
      "@tauri-apps/api/core": mock("./preview/mocks/core.ts"),
      "@tauri-apps/plugin-shell": mock("./preview/mocks/shell.ts"),
      "@tauri-apps/api/event": stubs,
      "@tauri-apps/api/app": stubs,
      "@tauri-apps/api/window": stubs,
      "@tauri-apps/api/webviewWindow": stubs,
      "@tauri-apps/plugin-dialog": stubs,
      "@tauri-apps/plugin-fs": stubs,
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
      },
    },
    // Polling: edits from the sandboxed agent don't emit fsevents.
    watch: { ignored: ["**/app/**", "**/target/**", "**/crates/**", "**/scripts/**", "**/*.test.ts", "**/*.test.tsx"], usePolling: true, interval: 400 },
  },
});
