// SPDX-License-Identifier: AGPL-3.0-only
// Isolated-live first-use preview: the actual first-use-app.html/Main against
// a scratch daemon. Port 1433. Fixture-only :1432 (vite.first-use-app.config.ts)
// is untouched. Fails closed at startup unless the scratch env is explicit.
import { fileURLToPath } from "node:url";
import { defineConfig, mergeConfig } from "vite";
import review from "./vite.review.config";
import {
  allowsFirstUseLiveRequest,
  FIRST_USE_LIVE_MUTATION_POSTS,
  assertFirstUseLiveDaemon,
  assertFirstUseLiveScratchEnv,
  verifyUpstreamScratchBinding,
} from "../preview/mocks/first-use-live-core";

const local = (path: string) => fileURLToPath(new URL(path, import.meta.url));

// All three throw when absent/wrong: the server never starts half-guarded.
const daemonTarget = assertFirstUseLiveDaemon(process.env.WENLAN_PREVIEW_DAEMON);
const scratch = assertFirstUseLiveScratchEnv(process.env);

export default mergeConfig(review, defineConfig({
  define: {
    __WENLAN_REVIEW__: "false",
    __WENLAN_PREVIEW_KNOWLEDGE_PATH__: JSON.stringify(scratch.knowledgePath),
  },
  plugins: [{
    name: "wenlan-first-use-live-guard",
    configureServer(server) {
      // Operator-visible scratch binding (the daemon exposes no data-dir
      // signal over HTTP, so this declaration plus the daemon launch is the
      // binding; the knowledge path is re-verified per mutation in-browser).
      console.log(
        `[first-use-live] daemon=${daemonTarget} data-dir=${scratch.dataDir} knowledge-path=${scratch.knowledgePath}`,
      );
      server.middlewares.use(async (req, res, next) => {
        if (!req.url?.startsWith("/daemon")) return next();
        const pathname = new URL(req.url, "http://localhost").pathname;
        const path = pathname.startsWith("/daemon/") ? pathname.slice(7) : "";
        if (!allowsFirstUseLiveRequest(req.method ?? "", path)) {
          res.statusCode = 403;
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ error: "First-use live preview blocks this daemon call." }));
          return;
        }
        // Second gate for the one mutation: prove the upstream daemon is the
        // scratch one BEFORE proxying. A failure here never reaches the
        // browser gate — the import is refused with 409, not proxied.
        if ((req.method ?? "") === "POST" && FIRST_USE_LIVE_MUTATION_POSTS.has(path)) {
          try {
            await verifyUpstreamScratchBinding(daemonTarget, scratch.knowledgePath);
          } catch (err) {
            res.statusCode = 409;
            res.setHeader("Content-Type", "application/json");
            res.end(JSON.stringify({
              error: err instanceof Error ? err.message : "Scratch binding check failed.",
            }));
            return;
          }
        }
        next();
      });
    },
  }],
  resolve: { alias: {
    "@tauri-apps/api/core": local("../preview/mocks/first-use-live-core.ts"),
    "tauri-plugin-clipboard-x-api": local("../preview/mocks/first-use-clipboard.ts"),
  } },
  server: {
    host: "127.0.0.1",
    port: 1433,
    strictPort: true,
    proxy: {
      "/daemon": {
        target: daemonTarget,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/daemon/, ""),
      },
    },
  },
  build: { outDir: "dist/first-use-live", rollupOptions: { input: local("../preview/first-use-app.html") } },
}));
