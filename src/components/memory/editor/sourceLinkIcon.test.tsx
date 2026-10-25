// SPDX-License-Identifier: AGPL-3.0-only
import { act } from "react";
import { describe, expect, it, vi } from "vitest";
import { sourceLinkIconMount } from "./sourceLinkIcon";

const URL = "https://x.com/simonw/status/1849";

describe("a saved link's icon", () => {
  it("says it is saved and opens its source", async () => {
    const open = vi.fn();
    const container = document.createElement("span");
    document.body.append(container);
    const mount = sourceLinkIconMount(() => ({ label: "Saved in Sources", open }));
    await act(async () => { mount(container, URL); });
    const button = container.querySelector("button")!;
    expect(button.title).toBe("Saved in Sources");
    expect(button.getAttribute("aria-label")).toBe("Saved in Sources");
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    button.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    await act(async () => { button.click(); });
    expect(open).toHaveBeenCalledWith(URL);
  });

  it("is only a picture without anything to open", async () => {
    const container = document.createElement("span");
    await act(async () => { sourceLinkIconMount()(container, URL); });
    expect(container.querySelector("button")).toBeNull();
    expect(container.firstElementChild?.getAttribute("aria-hidden")).toBe("true");
  });
});
