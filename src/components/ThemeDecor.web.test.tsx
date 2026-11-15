// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, expect, it, vi } from "vitest";
import { render } from "@testing-library/react";

const mocks = vi.hoisted(() => ({
  getActiveThemeImage: vi.fn(() => Promise.resolve("data:image/png;base64,AA==")),
  startCustomThemeSync: vi.fn(() => () => {}),
}));

vi.mock("../lib/customTheme", () => ({
  startCustomThemeSync: mocks.startCustomThemeSync,
  useCustomTheme: () => ({
    id: "custom",
    base: "dark",
    revision: "one",
    tokens: {},
    background: { placement: "sidebar", opacity: 0.35 },
    sticker: { corner: "bottom-right", size: 88 },
  }),
}));
vi.mock("../lib/tauri", () => ({
  getActiveThemeImage: mocks.getActiveThemeImage,
}));

import ThemeDecor from "./ThemeDecor";

afterEach(() => {
  vi.unstubAllGlobals();
  mocks.getActiveThemeImage.mockClear();
  mocks.startCustomThemeSync.mockClear();
});

it("does not request native theme image files during web startup", () => {
  vi.stubGlobal("__WENLAN_WEB__", true);
  render(<ThemeDecor />);
  expect(mocks.startCustomThemeSync).toHaveBeenCalledOnce();
  expect(mocks.getActiveThemeImage).not.toHaveBeenCalled();
});
