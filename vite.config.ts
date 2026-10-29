import { resolve } from "node:path";
import { defineConfig, normalizePath } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;
const devPort = Number.parseInt(process.env.WENLAN_DEV_UI_PORT ?? "1420", 10);
const cacheDir = process.env.WENLAN_VITE_CACHE_DIR ?? ".vite";
// The worktree-owned cache is outside node_modules. React's default exclusion
// therefore misses its prebundled dependencies and Babel processes them again.
const cachePrefix = normalizePath(resolve(cacheDir)).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

export default defineConfig(async () => ({
  // Linked node_modules can be shared by worktrees; optimizer state must not be.
  cacheDir,
  plugins: [react({ exclude: [/\/node_modules\//, new RegExp(`^${cachePrefix}/`)] }), tailwindcss()],
  define: {
    __WENLAN_REVIEW__: "false",
  },
  clearScreen: false,
  server: {
    port: devPort,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: devPort + 1 } : undefined,
    watch: {
      ignored: ["**/app/**"],
    },
  },
}));
