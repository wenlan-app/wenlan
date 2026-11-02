// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ComponentProps } from "react";
import { useState } from "react";
import { i18n } from "../../i18n";
import Sidebar, { SidebarToggleButton } from "./Sidebar";
vi.mock("./IdentityCard", () => ({
  default: ({ onOpenAbout, onOpenDetail, onOpenSettings }: {
    readonly onOpenAbout?: () => void;
    readonly onOpenDetail: (entityId: string) => void;
    readonly onOpenSettings?: () => void;
  }) => {
    const [open, setOpen] = useState(false);
    return <div data-testid="identity-card">
      <button aria-label="Account menu" onClick={() => setOpen((value) => !value)} type="button">Account</button>
      {open && <div role="menu">
        <button role="menuitem" onClick={onOpenSettings} type="button">Settings</button>
        <button role="menuitem" onClick={() => onOpenDetail("person-1")} type="button">Open identity detail</button>
        <button role="menuitem" onClick={onOpenAbout} type="button">Open identity about</button>
      </div>}
    </div>;
  },
}));

function renderSidebar(extraProps: Partial<ComponentProps<typeof Sidebar>> = {}) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <Sidebar
        hidden={false}
        mode="labels"
        onEntityClick={() => {}}
        onNavigateGraph={() => {}}
        onNavigatePages={() => {}}
        onNavigateSpaces={() => {}}
        {...extraProps}
      />
    </QueryClientProvider>,
  );
}

describe("Sidebar workspace", () => {
  beforeEach(async () => {
    localStorage.clear();
    await i18n.changeLanguage("en");
  });

  it("shows the 240px labels navigation without an inventory panel", () => {
    const { container } = renderSidebar();
    const aside = screen.getByRole("complementary", { name: "Primary navigation" });
    expect(aside).toHaveStyle({ width: "var(--workspace-sidebar-width, 240px)" });
    expect(container.querySelector(".notes-icon-rail")).toBeInTheDocument();
    expect(container.querySelector(".notes-workspace-panel")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Wiki" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Spaces" })).toBeInTheDocument();
    // The graph waits under More; memories and sources have no page.
    expect(screen.queryByRole("button", { name: "Graph" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Sources" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Memories" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Home" })).not.toBeInTheDocument();
  });

  it("keeps the icon-only navigation operable", async () => {
    const user = userEvent.setup();
    const onNavigatePages = vi.fn();
    renderSidebar({ mode: "icons", onNavigatePages });

    const aside = screen.getByRole("complementary", { name: "Primary navigation" });
    expect(aside).toHaveStyle({ width: "64px" });
    expect(aside).not.toHaveAttribute("aria-hidden", "true");
    expect(screen.queryByRole("region", { name: "Notes" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Wiki" }));
    expect(onNavigatePages).toHaveBeenCalledTimes(1);
  });

  it("UX-FIND-4: puts Graph and Topics under More with pins, and no Memories or Sources", async () => {
    const user = userEvent.setup();
    renderSidebar({ onNavigateEntities: vi.fn() });
    await user.click(screen.getByRole("button", { name: "More" }));
    expect(screen.queryByRole("button", { name: "Home" })).not.toBeInTheDocument();
    const more = screen.getByRole("group", { name: "More" });
    expect(more.querySelectorAll(".notes-more-destination")).toHaveLength(2);
    expect(more.querySelectorAll(".notes-more-pin")).toHaveLength(2);
    for (const name of ["Graph", "Topics"]) expect(within(more).getByRole("button", { name })).toBeInTheDocument();
    for (const name of ["Wiki", "Spaces", "Memories", "Sources"]) expect(within(more).queryByRole("button", { name })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Customize navigation/ })).not.toBeInTheDocument();
  });

  it("keeps page and memory inventory out of global navigation", () => {
    const { container } = renderSidebar();
    expect(container.querySelector(".notes-inventory-panel")).not.toBeInTheDocument();
    expect(container.querySelector(".notes-memory-panel")).not.toBeInTheDocument();
  });

  it("opens the graph from More, marks More while there, and keeps Settings in the account menu", async () => {
    const user = userEvent.setup();
    const onNavigateGraph = vi.fn();
    const onNavigateSettings = vi.fn();
    renderSidebar({ onNavigateGraph, onNavigateSettings, activeNavigation: "graph" });

    const more = screen.getByRole("button", { name: "More" });
    expect(more).toHaveAttribute("aria-current", "page");
    await user.click(more);
    await user.click(within(screen.getByRole("group", { name: "More" })).getByRole("button", { name: "Graph" }));
    expect(onNavigateGraph).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Account menu" }));
    await user.click(screen.getByRole("menuitem", { name: "Settings" }));
    expect(onNavigateSettings).toHaveBeenCalledTimes(1);
  });

  it("closes the narrow drawer after navigation from More", async () => {
    const user = userEvent.setup();
    const onRequestClose = vi.fn();
    const onNavigateGraph = vi.fn();
    renderSidebar({
      open: true,
      presentation: "overlay",
      onRequestClose,
      onNavigateGraph,
    });

    await user.click(screen.getByRole("button", { name: "More" }));
    await user.click(within(screen.getByRole("group", { name: "More" })).getByRole("button", { name: "Graph" }));
    expect(onNavigateGraph).toHaveBeenCalledTimes(1);
    expect(onRequestClose).toHaveBeenCalledTimes(1);
  });

  it("keeps the closed narrow drawer out of the accessibility tree", () => {
    renderSidebar({ hidden: true, open: false, presentation: "overlay" });
    expect(screen.queryByRole("button", { name: "Wiki" })).not.toBeInTheDocument();
  });

  it("removes hidden desktop navigation from the focusable accessibility surface", () => {
    const { container } = renderSidebar({ hidden: true, mode: "icons" });
    const aside = container.querySelector(".notes-workspace-sidebar");
    expect(aside).toHaveAttribute("aria-hidden", "true");
    expect(aside).toHaveAttribute("inert");
    expect(screen.queryByRole("button", { name: "Wiki" })).not.toBeInTheDocument();
  });

  it("keeps account controls in the bottom utilities", async () => {
    const user = userEvent.setup();
    const onOpenAbout = vi.fn();
    renderSidebar({ onOpenAbout, mode: "icons" });
    await user.click(screen.getByRole("button", { name: "Account menu" }));
    await user.click(screen.getByRole("menuitem", { name: "Open identity about" }));
    expect(onOpenAbout).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("identity-card").closest(".notes-rail-utilities")).not.toBeNull();
  });

  it("keeps the shared header toggle named and expanded in both states", () => {
    const { rerender } = render(<SidebarToggleButton collapsed={false} onToggle={() => {}} />);
    expect(screen.getByRole("button", { name: "Hide sidebar" })).toHaveClass("mem-icon-action", "workspace-panel-toggle");
    expect(screen.getByRole("button", { name: "Hide sidebar" })).toHaveAttribute("aria-expanded", "true");
    rerender(<SidebarToggleButton collapsed onToggle={() => {}} />);
    expect(screen.getByRole("button", { name: "Show sidebar" })).toHaveAttribute("data-sidebar-toggle", "true");
    expect(screen.getByRole("button", { name: "Show sidebar" })).toHaveAttribute("aria-expanded", "false");
  });
});
