// SPDX-License-Identifier: AGPL-3.0-only
// Production browser bundle embedded in wenlan-server.
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";
import { pdfAssets } from "./vite.pdf";

const local = (path: string) => fileURLToPath(new URL(path, import.meta.url));
const stubs = local("../src/web/tauri-stubs.ts");

/**
 * The desktop shell needs the first-paint bootstrap inline, before the module
 * graph starts. The daemon's browser build has a strict script-src 'self'
 * policy, so only that build moves the same classic script into a same-origin
 * file while keeping its position ahead of the application module.
 */
function externalizeWebBootstrap(): Plugin {
  const fileName = "web-bootstrap.js";
  let source: string | null = null;
  return {
    name: "wenlan-web-bootstrap",
    apply: "build",
    transformIndexHtml: {
      order: "pre",
      handler(html) {
        const classicScript = /<script>([\s\S]*?)<\/script>/u;
        const match = classicScript.exec(html);
        if (!match) throw new Error("Wenlan web build could not find the first-paint bootstrap");
        source = match[1];
        // Vite only bundles module scripts. `vite-ignore` suppresses its
        // classic-script warning and is removed from the emitted HTML.
        return html.replace(classicScript, `<script vite-ignore src="/${fileName}"></script>`);
      },
    },
    generateBundle() {
      if (source === null) throw new Error("Wenlan web bootstrap was not transformed");
      this.emitFile({ type: "asset", fileName, source });
    },
  };
}

export default defineConfig({
  plugins: [externalizeWebBootstrap(), react(), tailwindcss(), pdfAssets()],
  define: {
    __WENLAN_REVIEW__: "false",
    __WENLAN_WEB__: "true",
    __WENLAN_SOURCE_PREVIEW__: "true",
  },
  resolve: { alias: {
    "@tauri-apps/api/core": local("../src/web/core.ts"),
    "@tauri-apps/plugin-shell": local("../src/web/shell.ts"),
    "@tauri-apps/api/event": stubs,
    "@tauri-apps/api/app": stubs,
    "@tauri-apps/api/window": stubs,
    "@tauri-apps/api/dpi": stubs,
    "@tauri-apps/api/webviewWindow": stubs,
    "@tauri-apps/plugin-dialog": stubs,
    "@tauri-apps/plugin-fs": stubs,
    "@tauri-apps/plugin-notification": stubs,
    "tauri-plugin-clipboard-x-api": stubs,
  } },
  build: { outDir: local("../dist/web"), emptyOutDir: true },
});
