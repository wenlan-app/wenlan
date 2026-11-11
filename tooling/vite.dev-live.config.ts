// SPDX-License-Identifier: AGPL-3.0-only
// Native dev shell + current app UI + live daemon reads. No fixture graph.
import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig } from "vite";
import preview from "./vite.preview.config";
import { allowsDevLiveRequest } from "../preview/devLivePolicy";

export default mergeConfig(preview, defineConfig({
  plugins: [{
    name: "wenlan-dev-live-read-only",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (!req.url?.startsWith("/daemon")) return next();
        const pathname = new URL(req.url, "http://localhost").pathname;
        const path = pathname.startsWith("/daemon/") ? pathname.slice(7) : "";
        if (!allowsDevLiveRequest(req.method ?? "", path)) {
          res.statusCode = 403;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "Wenlan Dev is read-only. Changes are blocked." }));
          return;
        }
        next();
      });
    },
  }],
  resolve: { alias: {
    "@tauri-apps/api/core": fileURLToPath(new URL("../preview/mocks/dev-live-core.ts", import.meta.url)),
  } },
  server: { host: "127.0.0.1", port: 1424, strictPort: true },
}));
