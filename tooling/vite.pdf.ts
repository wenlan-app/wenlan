// SPDX-License-Identifier: AGPL-3.0-only
import { fileURLToPath } from "node:url";
import { normalizePath } from "vite";
import { viteStaticCopy } from "vite-plugin-static-copy";

/** PDF.js loads these by filename. Bundle them so local PDFs work offline. */
export function pdfAssets() {
  return viteStaticCopy({
    targets: ["cmaps", "standard_fonts", "wasm"].map((name) => ({
      src: normalizePath(fileURLToPath(new URL(`../node_modules/pdfjs-dist/${name}`, import.meta.url))),
      dest: "pdfjs",
    })),
  });
}
