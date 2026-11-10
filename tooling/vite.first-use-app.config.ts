// SPDX-License-Identifier: AGPL-3.0-only
import { fileURLToPath } from "node:url";
import { mergeConfig } from "vite";
import review from "./vite.review.config";
const local = (path: string) => fileURLToPath(new URL(path, import.meta.url));
export default mergeConfig(review, {
  define: { __WENLAN_REVIEW__: "false" },
  resolve: { alias: {
    "@tauri-apps/api/core": local("../preview/mocks/first-use-core.ts"),
    "tauri-plugin-clipboard-x-api": local("../preview/mocks/first-use-clipboard.ts"),
  } },
  server: { host: "127.0.0.1", port: 1432, strictPort: true },
  build: { outDir: "dist/first-use-app", rollupOptions: { input: local("../preview/first-use-app.html") } },
});
