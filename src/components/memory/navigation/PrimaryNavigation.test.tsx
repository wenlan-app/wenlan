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
  pages: "Docs",
  skills: "Skills",
  rules: "Rules",
  spaces: "Spaces",
} as const;

const SECTIONS = { onNavigatePages: vi.fn(), onNavigateSkills: vi.fn(), onNavigateRules: vi.fn() };

function renderNavigation(active: GlobalNavigation | null = null, compact = true) {
  const callbacks = {
    entities: vi.fn(), graph: vi.fn(), home: vi.fn(),
    pages: vi.fn(), skills: vi.fn(), rules: vi.fn(), spaces: vi.fn(),
  };
  render(<PrimaryNavigation
    active={active}
    compact={compact}
    labels={labels}
    onNavigateEntities={callbacks.entities}
    onNavigateGraph={callbacks.graph}
    onNavigateHome={callbacks.home}
    onNavigatePages={callbacks.pages}
    onNavigateSkills={callbacks.skills}
    onNavigateRules={callbacks.rules}
    onNavigateSpaces={callbacks.spaces}
  />);
  return callbacks;
}

function railLabels() {
  return screen.getAllByRole("button").map((button) => button.getAttribute("aria-label"));
}

describe("PrimaryNavigation", () => {
  it("UX-FIND-4: shows the same rows in the same order, with no More menu, Topics, Memories or Sources", () => {
    renderNavigation("pages");
    expect(railLabels()).toEqual(["Home", "Spaces", "Docs", "Skills", "Rules", "Graph"]);
    expect(screen.queryByRole("button", { name: "More" })).not.toBeInTheDocument();
    // Topics open from the graph (Qi-Xuan 2026-11-09 19:37 PT).
    for (const name of ["Topics", "Memories", "Sources", "Library", "Wiki"]) expect(screen.queryByRole("button", { name })).not.toBeInTheDocument();
  });

  it("navigates from each destination", async () => {
    const user = userEvent.setup();
    const callbacks = renderNavigation(null, false);
    for (const name of ["Home", "Docs", "Skills", "Rules", "Spaces", "Graph"]) await user.click(screen.getByRole("button", { name }));
    expect(callbacks.home).toHaveBeenCalledOnce();
    expect(callbacks.pages).toHaveBeenCalledOnce();
    expect(callbacks.skills).toHaveBeenCalledOnce();
    expect(callbacks.rules).toHaveBeenCalledOnce();
    expect(callbacks.spaces).toHaveBeenCalledWith(false);
    expect(callbacks.graph).toHaveBeenCalledOnce();
  });

  it("marks exactly one current destination", () => {
    renderNavigation("rules");
    const current = screen.getAllByRole("button").filter((button) => button.getAttribute("aria-current") === "page");
    expect(current.map((button) => button.getAttribute("aria-label"))).toEqual(["Rules"]);
  });

  it("leaves out a destination with no route", () => {
    render(<PrimaryNavigation active={null} labels={labels} onNavigateHome={vi.fn()} {...SECTIONS} onNavigateSpaces={vi.fn()} />);
    expect(railLabels()).toEqual(["Home", "Spaces", "Docs", "Skills", "Rules"]);
  });

  it("UX-FIND-4: the rows never change with labels or icons, nothing nests under them, and sections show their counts", () => {
    const navigation = (compact: boolean, counts: Record<string, number>) => <PrimaryNavigation
      active={null}
      compact={compact}
      labels={{ ...labels, counted: (label, count) => `${label}, ${count} pages` }}
      onNavigateHome={vi.fn()}
      {...SECTIONS}
      onNavigateSpaces={vi.fn()}
      onNavigateGraph={vi.fn()}
      counts={counts}
    />;
    const { container, rerender } = render(navigation(false, { docs: 24, skills: 6, rules: 0 }));
    const rows = ["Home", "Spaces", "Docs", "Skills", "Rules", "Graph"];
    expect(railLabels()).toEqual(rows);
    // An empty section keeps its row and says 0.
    expect([...container.querySelectorAll(".notes-primary-link")].map((row) => row.textContent)).toEqual(["Home", "Spaces", "Docs24", "Skills6", "Rules0", "Graph"]);
    expect(screen.getByRole("button", { name: "Docs" })).toHaveAccessibleDescription("Docs, 24 pages");
    // No page list, folder tree or fold control lives in the navigation.
    expect(container.querySelector(".notes-wiki-section, .wiki-workspace-directory, [aria-expanded]")).toBeNull();
    expect(container.querySelector("nav")?.children).toHaveLength(rows.length);
    // The counts change; the rows and their order do not.
    rerender(navigation(false, { docs: 0, skills: 0, rules: 0 }));
    expect(railLabels()).toEqual(rows);
    // Folded to icons the same rows stay, without the numbers.
    rerender(navigation(true, { docs: 24, skills: 6, rules: 2 }));
    expect(railLabels()).toEqual(rows);
    expect(container.querySelector(".notes-primary-count")).toBeNull();
  });

  it("UX-FIND-6: shows pinned pages under the destinations with a heading, and opens, unpins and reorders them", async () => {
    const user = userEvent.setup();
    const open = vi.fn();
    const unpin = vi.fn();
    const { container } = render(<PrimaryNavigation
      active={null}
      labels={{ ...labels, pinned: "Pinned", unpin: (title) => `Unpin ${title}` }}
      onNavigateHome={vi.fn()} {...SECTIONS} onNavigateSpaces={vi.fn()} onNavigateGraph={vi.fn()}
      pinnedPages={[{ id: "p1", title: "Weekly review" }, { id: "p2", title: "Agents" }]}
      activePinnedPageId="p2"
      onOpenPinnedPage={open}
      onUnpinPage={unpin}
    />);
    expect(railLabels()).toEqual(["Home", "Spaces", "Docs", "Skills", "Rules", "Graph", "Weekly review", "Unpin Weekly review", "Agents", "Unpin Agents"]);
    const group = screen.getByRole("group", { name: "Pinned" });
    expect(group.textContent).toBe("PinnedWeekly reviewAgents");
    expect(container.querySelector(".notes-pinned-pages")?.previousElementSibling?.getAttribute("aria-label")).toBe("Graph");
    expect(screen.getByRole("button", { name: "Agents" })).toHaveAttribute("aria-current", "page");
    await user.click(screen.getByRole("button", { name: "Weekly review" }));
    expect(open).toHaveBeenCalledWith("p1");
    await user.click(screen.getByRole("button", { name: "Unpin Agents" }));
    expect(unpin).toHaveBeenCalledWith("p2");
  });

  it("UX-FIND-6: with nothing pinned the left side has no pinned group", () => {
    render(<PrimaryNavigation active={null} labels={labels} onNavigateHome={vi.fn()} {...SECTIONS} onNavigateSpaces={vi.fn()} onOpenPinnedPage={vi.fn()} pinnedPages={[]} />);
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
    expect(railLabels()).toEqual(["Home", "Spaces", "Docs", "Skills", "Rules"]);
  });

  it("UX-FIND-4: lists the pages opened lately under Recent, each with its section, four at most", async () => {
    const user = userEvent.setup();
    const open = vi.fn();
    const recent = [
      { id: "r1", title: "Pricing", section: "docs" }, { id: "r2", title: "Write release notes", section: "skills" },
      { id: "r3", title: "How we work", section: "rules" }, { id: "r4", title: "Onboarding", section: "docs" },
      { id: "r5", title: "One too many", section: "docs" },
    ] as const;
    const navigation = (compact: boolean) => <PrimaryNavigation
      active={null}
      compact={compact}
      labels={{ ...labels, recent: "Recent" }}
      onNavigateHome={vi.fn()} {...SECTIONS} onNavigateSpaces={vi.fn()} onNavigateGraph={vi.fn()}
      recentPages={recent}
      onOpenPinnedPage={open}
    />;
    const { rerender } = render(navigation(false));
    const group = screen.getByRole("group", { name: "Recent" });
    expect([...group.querySelectorAll("button")].map((row) => row.textContent)).toEqual(["PricingDocs", "Write release notesSkills", "How we workRules", "OnboardingDocs"]);
    expect(screen.getByRole("button", { name: "How we work" })).toHaveAccessibleDescription("Rules");
    await user.click(screen.getByRole("button", { name: "Write release notes" }));
    expect(open).toHaveBeenCalledWith("r2");
    // Icons leave no room for a list of titles.
    rerender(navigation(true));
    expect(screen.queryByRole("group", { name: "Recent" })).not.toBeInTheDocument();
  });
});
