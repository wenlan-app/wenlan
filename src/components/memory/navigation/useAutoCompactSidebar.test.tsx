// SPDX-License-Identifier: AGPL-3.0-only
import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { useAutoCompactSidebar } from "./useAutoCompactSidebar";
import type { SidebarPreferences } from "./navigationPreferences";

const labels: SidebarPreferences = { visible: true, mode: "labels", width: 240 };
const initialWidth = window.innerWidth;

function setWindowWidth(width: number) {
  Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  window.dispatchEvent(new Event("resize"));
}

afterEach(() => setWindowWidth(initialWidth));

describe("useAutoCompactSidebar", () => {
  it("folds labels only while the list, Info and a readable note do not fit", () => {
    setWindowWidth(1280);
    const { result } = renderHook(() => useAutoCompactSidebar(labels, true));
    act(() => result.current.demand.report("list", "list", 248));
    expect(result.current.compact).toBe(false);
    act(() => result.current.demand.report("info", "info", 320));
    expect(result.current.compact).toBe(true);
    act(() => setWindowWidth(1440));
    expect(result.current.compact).toBe(false);
    act(() => setWindowWidth(1280));
    act(() => result.current.demand.report("info", "info", 0));
    expect(result.current.compact).toBe(false);
  });

  it("does not fold when that would dock a stacked note list beside the note", () => {
    setWindowWidth(1024);
    const { result } = renderHook(() => useAutoCompactSidebar(labels, true));
    act(() => {
      result.current.demand.report("list", "list", 248);
      result.current.demand.report("info", "info", 320);
    });
    expect(result.current.compact).toBe(false);
    act(() => setWindowWidth(960));
    expect(result.current.compact).toBe(true);
  });

  it("leaves icons, a hidden rail and disabled views alone", () => {
    setWindowWidth(1000);
    const icons = renderHook(() => useAutoCompactSidebar({ ...labels, mode: "icons" }, true));
    const hidden = renderHook(() => useAutoCompactSidebar({ ...labels, visible: false }, true));
    const disabled = renderHook(() => useAutoCompactSidebar(labels, false));
    for (const { result } of [icons, hidden, disabled]) {
      act(() => result.current.demand.report("info", "info", 320));
      expect(result.current.compact).toBe(false);
    }
  });

  it("keeps a manual expand until Info or the window changes", () => {
    setWindowWidth(1280);
    const { result } = renderHook(() => useAutoCompactSidebar(labels, true));
    act(() => {
      result.current.demand.report("list", "list", 248);
      result.current.demand.report("info", "info", 320);
    });
    expect(result.current.compact).toBe(true);
    act(() => result.current.keepExpanded());
    expect(result.current.compact).toBe(false);
    expect(result.current.transitioning).toBe(false);
    act(() => result.current.demand.report("info", "info", 0));
    act(() => result.current.demand.report("info", "info", 320));
    expect(result.current.compact).toBe(true);
    act(() => result.current.keepExpanded());
    act(() => setWindowWidth(1281));
    expect(result.current.compact).toBe(true);
  });
});
