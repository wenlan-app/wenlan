// SPDX-License-Identifier: AGPL-3.0-only
import { act, render, screen, waitFor } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, describe, expect, it } from "vitest";
import { AppToaster } from "./AppToaster";

describe("AppToaster", () => {
  afterEach(() => {
    act(() => toast.dismiss());
  });

  it("shows a toast raised from the main window", async () => {
    render(<AppToaster />);
    act(() => {
      toast("Source connected", { description: "/Users/me/Vault" });
    });
    expect(await screen.findByText("Source connected")).toBeTruthy();
    expect(screen.getByText("/Users/me/Vault")).toBeTruthy();
    expect(screen.getByText("Source connected").closest("[data-sonner-toast]")?.className).toContain("mem-toast");
  });
});

describe("AppToaster placement", () => {
  afterEach(() => {
    act(() => toast.dismiss());
    document.querySelectorAll("main, .page-info-drawer-docked").forEach((node) => node.remove());
  });

  function box(node: Element, left: number, right: number) {
    node.getBoundingClientRect = () => ({ left, right, width: right - left, top: 0, bottom: 800, height: 800, x: left, y: 0, toJSON: () => ({}) });
  }

  it("sits at the bottom right of the page area, left of an open inspector", async () => {
    const main = document.createElement("main");
    box(main, 488, 1280);
    const drawer = document.createElement("div");
    drawer.className = "page-info-drawer-docked";
    const panel = document.createElement("div");
    panel.className = "page-info-drawer";
    box(panel, 960, 1280);
    drawer.appendChild(panel);
    document.body.append(main, drawer);
    Object.defineProperty(window, "innerWidth", { configurable: true, value: 1280 });

    render(<AppToaster />);
    act(() => { toast("Copied"); });
    const item = await screen.findByText("Copied");
    const list = item.closest("[data-sonner-toaster]") as HTMLElement;
    await waitFor(() => expect(list.style.getPropertyValue("--offset-right")).toBe("336px"));
    expect(list.dataset.xPosition).toBe("right");
  });
});
