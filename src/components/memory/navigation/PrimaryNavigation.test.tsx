// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { PrimaryNavigation } from "./PrimaryNavigation";
import type { GlobalNavigation } from "./viewState";

const labels = {
  entities: "Topics",
  graph: "Graph",
  home: "Home",
  navigation: "Primary navigation",
  pages: "Wiki",
  spaces: "Spaces",
} as const;

function renderNavigation(active: GlobalNavigation | null = null, compact = true) {
  const callbacks = {
    entities: vi.fn(), graph: vi.fn(), home: vi.fn(),
    pages: vi.fn(), spaces: vi.fn(),
  };
  render(<PrimaryNavigation
    active={active}
    compact={compact}
    labels={labels}
    onNavigateEntities={callbacks.entities}
    onNavigateGraph={callbacks.graph}
    onNavigateHome={callbacks.home}
    onNavigatePages={callbacks.pages}
    onNavigateSpaces={callbacks.spaces}
  />);
  return callbacks;
}

function railLabels() {
  return screen.getAllByRole("button").map((button) => button.getAttribute("aria-label"));
}

describe("PrimaryNavigation", () => {
  it("UX-FIND-4: puts every destination on the left, with no More menu, Memories or Sources", () => {
    renderNavigation("pages");
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "Graph", "Topics"]);
    expect(screen.queryByRole("button", { name: "More" })).not.toBeInTheDocument();
    for (const name of ["Memories", "Sources"]) expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
  });

  it("navigates from each destination", async () => {
    const user = userEvent.setup();
    const callbacks = renderNavigation(null, false);
    await user.click(screen.getByRole("button", { name: "Home" }));
    await user.click(screen.getByRole("button", { name: "Wiki" }));
    await user.click(screen.getByRole("button", { name: "Spaces" }));
    await user.click(screen.getByRole("button", { name: "Graph" }));
    await user.click(screen.getByRole("button", { name: "Topics" }));
    expect(callbacks.home).toHaveBeenCalledOnce();
    expect(callbacks.pages).toHaveBeenCalledOnce();
    expect(callbacks.spaces).toHaveBeenCalledWith(false);
    expect(callbacks.graph).toHaveBeenCalledOnce();
    expect(callbacks.entities).toHaveBeenCalledOnce();
  });

  it("marks exactly one current destination", () => {
    renderNavigation("graph");
    const current = screen.getAllByRole("button").filter((button) => button.getAttribute("aria-current") === "page");
    expect(current.map((button) => button.getAttribute("aria-label"))).toEqual(["Graph"]);
  });

  it("leaves out a destination with no route", () => {
    render(<PrimaryNavigation active={null} labels={labels} onNavigateHome={vi.fn()} onNavigatePages={vi.fn()} onNavigateSpaces={vi.fn()} />);
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces"]);
  });

  it("UX-FIND-4: with labels, the Wiki section closes the one column instead of a second column", () => {
    const hosts: (HTMLDivElement | null)[] = [];
    const navigation = (compact: boolean) => <PrimaryNavigation
      active={null}
      compact={compact}
      labels={labels}
      onNavigateHome={vi.fn()}
      onNavigatePages={vi.fn()}
      onNavigateSpaces={vi.fn()}
      onWikiSectionHost={(host) => hosts.push(host)}
    />;
    const { container, rerender } = render(navigation(false));
    // The shell renders the Wiki heading and every page into the section.
    expect(railLabels()).toEqual(["Home", "Spaces"]);
    const section = container.querySelector(".notes-rail-nav > .notes-wiki-section");
    expect(section).not.toBeNull();
    expect(section?.previousElementSibling?.getAttribute("aria-label")).toBe("Spaces");
    expect(hosts.at(-1)).toBe(section);
    // Folded to icons there is no room: Wiki is a destination again.
    rerender(navigation(true));
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces"]);
    expect(container.querySelector(".notes-wiki-section")).toBeNull();
  });
});
