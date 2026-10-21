// SPDX-License-Identifier: AGPL-3.0-only
import { act, render, screen } from "@testing-library/react";
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
