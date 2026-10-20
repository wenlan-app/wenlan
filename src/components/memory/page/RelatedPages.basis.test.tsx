// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { BasisRef, PageLinkOutbound } from "../../../lib/tauri";
import RelatedPages from "./RelatedPages";

const pricing: PageLinkOutbound = { label: "Pricing", target_page_id: "page-2", target_title: "Pricing" };
const roadmap: PageLinkOutbound = { label: "Roadmap", target_page_id: "page-3", target_title: "Roadmap" };
const basisOnPricing: BasisRef = { page_id: "page-2", title: "Pricing", state: "ok", from_version: 1, to_version: 1 };

function setup(props: Partial<React.ComponentProps<typeof RelatedPages>> = {}) {
  const onMarkBasis = vi.fn().mockResolvedValue(true);
  const onUnmarkBasis = vi.fn();
  const view = render(<RelatedPages
    outbound={[pricing, roadmap]}
    onPageClick={vi.fn()}
    currentPageId="page-1"
    onMarkBasis={onMarkBasis}
    onUnmarkBasis={onUnmarkBasis}
    {...props}
  />);
  return { ...view, onMarkBasis, onUnmarkBasis };
}

beforeEach(() => window.localStorage.clear());
afterEach(cleanup);

describe("RelatedPages as sources", () => {
  it("lets a related note become a source from its row menu", async () => {
    const { onMarkBasis } = setup();
    const row = screen.getByRole("button", { name: "Roadmap" }).closest(".note-info-link-item") as HTMLElement;

    fireEvent.click(within(row).getByRole("button", { name: "More actions" }));
    const item = screen.getByRole("menuitem", { name: /Use as a source/ });
    expect(item).toHaveTextContent("You'll be told when it changes");
    fireEvent.click(item);

    await waitFor(() => expect(onMarkBasis).toHaveBeenCalledWith("page-3"));
    expect(onMarkBasis).toHaveBeenCalledTimes(1);
  });

  it("says so when the note could not be used as a source", async () => {
    const { onMarkBasis } = setup();
    onMarkBasis.mockResolvedValue(false);

    fireEvent.click(screen.getAllByRole("button", { name: "More actions" })[0]);
    fireEvent.click(screen.getByRole("menuitem", { name: /Use as a source/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not use it as a source. Try again.");
  });

  it("offers no menu without a handler and never for the note itself", () => {
    const { unmount } = setup({ onMarkBasis: undefined, onUnmarkBasis: undefined });
    expect(screen.queryByRole("button", { name: "More actions" })).toBeNull();
    unmount();

    setup({ outbound: [{ label: "Self", target_page_id: "page-1", target_title: "Self" }] });
    expect(screen.queryByRole("button", { name: "More actions" })).toBeNull();
  });

  it("marks a source row with a badge and offers to stop instead", () => {
    const { onUnmarkBasis } = setup({ basis: [basisOnPricing] });

    expect(screen.getAllByText("Source")).toHaveLength(1);
    const row = screen.getByRole("button", { name: "Pricing" }).closest(".note-info-link-item") as HTMLElement;
    expect(within(row).getByText("Source")).toBeInTheDocument();

    fireEvent.click(within(row).getByRole("button", { name: "More actions" }));
    expect(screen.queryByRole("menuitem", { name: /Use as a source/ })).toBeNull();
    fireEvent.click(screen.getByRole("menuitem", { name: "Stop using as a source" }));
    expect(onUnmarkBasis).toHaveBeenCalledWith("page-2");
  });

  it("explains the badge once, on its first hover", () => {
    setup({ basis: [basisOnPricing] });
    const badge = screen.getByText("Source");

    fireEvent.mouseEnter(badge);
    expect(screen.getByRole("tooltip")).toHaveTextContent("This note is a source for this one.");
    fireEvent.mouseLeave(badge);
    expect(screen.queryByRole("tooltip")).toBeNull();

    fireEvent.mouseEnter(badge);
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("still shows the tip when browser storage is unavailable", () => {
    const getItem = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("blocked"); });
    try {
      setup({ basis: [basisOnPricing] });
      fireEvent.mouseEnter(screen.getByText("Source"));
      expect(screen.getByRole("tooltip")).toBeInTheDocument();
    } finally {
      getItem.mockRestore();
      setItem.mockRestore();
    }
  });
});
