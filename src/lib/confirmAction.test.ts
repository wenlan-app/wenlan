// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { ask } from "@tauri-apps/plugin-dialog";
import { confirmAction } from "./confirmAction";

const labels = { confirm: "Delete page", cancel: "Cancel" };
const MAC_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
const WINDOWS_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko)";

describe("confirmAction", () => {
  beforeEach(() => {
    vi.mocked(ask).mockReset();
    vi.spyOn(window, "confirm").mockReturnValue(false);
  });
  afterEach(() => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("uses the browser confirm outside the app", async () => {
    vi.mocked(window.confirm).mockReturnValue(true);
    await expect(confirmAction("Delete this page?", labels)).resolves.toBe(true);
    expect(window.confirm).toHaveBeenCalledWith("Delete this page?");
    expect(ask).not.toHaveBeenCalled();
  });

  describe("inside the app", () => {
    beforeEach(() => { Reflect.set(window, "__TAURI_INTERNALS__", {}); });

    it("asks through the native dialog, never window.confirm", async () => {
      vi.stubGlobal("navigator", { ...navigator, userAgent: WINDOWS_UA });
      vi.mocked(ask).mockResolvedValue(true);
      await expect(confirmAction("Delete this page?", labels)).resolves.toBe(true);
      expect(ask).toHaveBeenCalledWith("Delete this page?", { okLabel: "Delete page", cancelLabel: "Cancel" });
      expect(window.confirm).not.toHaveBeenCalled();
    });

    it("puts the question in the bold line on macOS", async () => {
      vi.stubGlobal("navigator", { ...navigator, userAgent: MAC_UA });
      vi.mocked(ask).mockResolvedValue(false);
      await expect(confirmAction("Delete this page?", labels)).resolves.toBe(false);
      expect(ask).toHaveBeenCalledWith("", { okLabel: "Delete page", cancelLabel: "Cancel", title: "Delete this page?" });
    });

    it("treats a dialog that fails to open as no", async () => {
      vi.spyOn(console, "warn").mockImplementation(() => {});
      vi.mocked(ask).mockRejectedValue(new Error("not allowed"));
      await expect(confirmAction("Delete this page?", labels)).resolves.toBe(false);
    });
  });
});

function listSourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "test" ? [] : listSourceFiles(fullPath);
    if (!/\.tsx?$/.test(entry.name) || /\.test\.tsx?$/.test(entry.name)) return [];
    return [fullPath];
  });
}

describe("app code", () => {
  it("never opens a browser dialog, which the macOS app answers with no", () => {
    const offenders = listSourceFiles(path.resolve("src")).flatMap((file) => {
      if (file.endsWith(path.join("lib", "confirmAction.ts"))) return [];
      const text = fs.readFileSync(file, "utf8");
      const declaresConfirm = /\b(const|let|function)\s+confirm\b/.test(text);
      const calls = text.match(/\bwindow\.(confirm|alert|prompt)\s*\(|(?<![.\w])(alert|prompt)\s*\(/g) ?? [];
      if (!declaresConfirm && /(?<![.\w])confirm\s*\(/.test(text)) calls.push("confirm(");
      return calls.map(call => `${path.relative(process.cwd(), file)}: ${call}`);
    });
    expect(offenders).toEqual([]);
  });
});
