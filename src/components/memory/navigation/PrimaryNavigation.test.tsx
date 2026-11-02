import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PrimaryNavigation } from "./PrimaryNavigation";
import { PageIcon } from "../assets/PageIcon";
import { NAVIGATION_DESTINATION_ORDER, NAVIGATION_PREFERENCE_KEY, writeNavigationPreferences } from "./navigationPreferences";
import type { GlobalNavigation } from "./viewState";

const labels = {
  entities: "Topics",
  graph: "Graph",
  home: "Home",
  more: "More",
  navigation: "Primary navigation",
  pages: "Wiki",
  pinToSidebar: (name: string) => `Pin ${name} to sidebar`,
  sources: "Sources",
  spaces: "Spaces",
  unpinFromSidebar: (name: string) => `Unpin ${name} from sidebar`,
} as const;

function renderNavigation(active: GlobalNavigation | null = null) {
  const callbacks = {
    entities: vi.fn(), graph: vi.fn(), home: vi.fn(),
    pages: vi.fn(), sources: vi.fn(), spaces: vi.fn(),
  };
  const props = {
    active, labels, compact: true,
    onNavigateEntities: callbacks.entities,
    onNavigateGraph: callbacks.graph,
    onNavigateHome: callbacks.home,
    onNavigatePages: callbacks.pages,
    onNavigateSources: callbacks.sources,
    onNavigateSpaces: callbacks.spaces,
  };
  return { callbacks, props, ...render(<PrimaryNavigation {...props} />) };
}

function railLabels() {
  return screen.getAllByRole("button").filter((button) => button.classList.contains("notes-rail-button")).map((button) => button.getAttribute("aria-label"));
}

async function openMore(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "More" }));
  return screen.getByRole("group", { name: "More" });
}

function pinButton(more: HTMLElement, label: string) {
  return within(more).getByRole("button", { name: label });
}

beforeEach(() => {
  localStorage.clear();
  writeNavigationPreferences([]);
  vi.restoreAllMocks();
});

describe("PrimaryNavigation", () => {
  it("defaults to Home, Wiki, Spaces and More, with Graph and Sources one step away in More", async () => {
    const user = userEvent.setup();
    localStorage.clear();
    const { callbacks } = renderNavigation("pages");
    expect(screen.getByRole("navigation", { name: "Primary navigation" })).toBeInTheDocument();
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    const wiki = screen.getByRole("button", { name: "Wiki", current: "page" });
    expect(wiki).toHaveAttribute("title", "Wiki");
    expect(wiki.querySelector('[data-navigation-icon="wiki-stack"]')).toHaveAttribute("aria-hidden", "true");
    const pageIcon = render(<PageIcon />).container.querySelector('[data-resource-icon="page"]');
    expect(pageIcon).toBeInTheDocument();
    expect(pageIcon).not.toHaveAttribute("data-navigation-icon");
    await user.click(wiki);
    await user.click(screen.getByRole("button", { name: "Spaces" }));
    expect(callbacks.pages).toHaveBeenCalledOnce();
    expect(callbacks.spaces).toHaveBeenCalledExactlyOnceWith(false);
    await user.click(within(await openMore(user)).getByRole("button", { name: "Graph" }));
    expect(callbacks.graph).toHaveBeenCalledOnce();
    const more = await openMore(user);
    const sources = within(more).getByRole("button", { name: "Sources" });
    expect(sources.querySelector('[data-navigation-icon="sources-library"]')).toHaveAttribute("aria-hidden", "true");
    await user.click(sources);
    expect(callbacks.sources).toHaveBeenCalledOnce();
  });

  it("lists only optional destinations in More and navigates from each stable row", async () => {
    const user = userEvent.setup();
    const { callbacks } = renderNavigation("entities");
    const moreButton = screen.getByRole("button", { name: "More" });
    expect(moreButton).toHaveAttribute("aria-current", "page");
    const more = await openMore(user);
    expect(within(more).getAllByRole("button").filter((button) => button.classList.contains("notes-more-destination-link")).map((button) => button.getAttribute("aria-label")))
      .toEqual(["Sources", "Graph", "Topics"]);
    for (const label of ["Wiki", "Spaces", "Memories"]) {
      expect(within(more).queryByRole("button", { name: label })).not.toBeInTheDocument();
      expect(within(more).queryByRole("button", { name: `Unpin ${label} from sidebar` })).not.toBeInTheDocument();
    }
    expect(moreButton).not.toHaveAttribute("aria-current");
    expect(within(more).getByRole("button", { name: "Topics", current: "page" })).toBeInTheDocument();
    for (const [label, callback] of [
      ["Sources", callbacks.sources], ["Graph", callbacks.graph], ["Topics", callbacks.entities],
    ] as const) {
      await user.click(within(screen.getByRole("group", { name: "More" })).getByRole("button", { name: label }));
      expect(callback).toHaveBeenCalledOnce();
      expect(moreButton).toHaveAttribute("aria-expanded", "false");
      await openMore(user);
    }
  });

  it("restores required destinations from legacy-hidden data and keeps More optional", async () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible: ["home", "sources"] }));
    const user = userEvent.setup();
    renderNavigation("sources");
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    const more = await openMore(user);
    expect(within(more).queryByRole("button", { name: "Home" })).not.toBeInTheDocument();
    expect(within(more).getAllByRole("button").filter((button) => button.classList.contains("notes-more-destination-link")).map((button) => button.getAttribute("aria-label")))
      .toEqual(["Sources", "Graph", "Topics"]);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).visible).toEqual(["home", "sources"]);
    writeNavigationPreferences(["home", "sources"]);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).visible).toEqual(["home", "pages", "spaces", "sources"]);
  });

  it("pins immediately without navigating, preserves focus, persists and keeps unpinned navigation reachable", async () => {
    const user = userEvent.setup();
    const view = renderNavigation("graph");
    const more = await openMore(user);
    const pinGraph = pinButton(more, "Pin Graph to sidebar");
    await user.click(pinGraph);
    const unpinGraph = pinButton(more, "Unpin Graph from sidebar");
    expect(unpinGraph).toBe(pinGraph);
    expect(unpinGraph).toHaveFocus();
    expect(unpinGraph).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("group", { name: "More" })).toBeInTheDocument();
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "Graph", "More"]);
    expect(screen.getByRole("button", { name: "Graph", current: "page" })).toBeInTheDocument();
    expect(view.callbacks.graph).not.toHaveBeenCalled();

    await user.click(unpinGraph);
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    expect(within(more).getByRole("button", { name: "Graph", current: "page" })).toBeInTheDocument();
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).visible).toEqual(["home", "pages", "spaces"]);
    view.unmount();

    const remounted = renderNavigation("graph");
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    const reopened = await openMore(user);
    await user.click(within(reopened).getByRole("button", { name: "Graph" }));
    expect(remounted.callbacks.graph).toHaveBeenCalledOnce();
    expect(screen.queryByRole("group", { name: "More" })).not.toBeInTheDocument();
  });

  it("keeps required destinations in the rail while every optional destination is unpinned", async () => {
    const user = userEvent.setup();
    writeNavigationPreferences(NAVIGATION_DESTINATION_ORDER);
    renderNavigation("pages");
    const more = await openMore(user);
    for (const label of ["Sources", "Graph", "Topics"]) {
      await user.click(pinButton(more, `Unpin ${label} from sidebar`));
    }
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).visible).toEqual(["home", "pages", "spaces"]);
    expect(within(more).queryByRole("button", { name: "Wiki" })).not.toBeInTheDocument();
    await user.click(pinButton(more, "Pin Graph to sidebar"));
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "Graph", "More"]);
    expect(screen.getByRole("group", { name: "More" })).toBeInTheDocument();
  });

  it("recovers malformed persistence and retains More when a saved empty preference remounts", () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, '{"version":1,"visible":["old-route"]}');
    const view = renderNavigation();
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    view.unmount();
    writeNavigationPreferences([]);
    renderNavigation("pages");
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "More"]);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).visible).toEqual(["home", "pages", "spaces"]);
  });

  it("keeps exactly one accessible current destination as pins move without a decorative stripe", async () => {
    const user = userEvent.setup();
    writeNavigationPreferences(["graph"]);
    const { container } = renderNavigation("graph");
    const marker = () => container.querySelectorAll('[aria-current="page"]');
    expect(container.querySelector(".notes-nav-active-marker")).toBeNull();
    expect(marker()).toHaveLength(1);
    const more = await openMore(user);
    expect(marker()).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Graph", current: "page" })).toBeInTheDocument();
    await user.click(pinButton(more, "Unpin Graph from sidebar"));
    expect(pinButton(more, "Pin Graph to sidebar")).toHaveFocus();
    expect(within(more).getByRole("button", { name: "Graph", current: "page" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "More" })).not.toHaveAttribute("aria-current");
    expect(marker()).toHaveLength(1);
    await user.keyboard("{Escape}");
    expect(screen.getByRole("button", { name: "More", current: "page" })).toBeInTheDocument();
    expect(marker()).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "More" }));
    const reopened = screen.getByRole("group", { name: "More" });
    await user.click(pinButton(reopened, "Pin Graph to sidebar"));
    expect(screen.getByRole("button", { name: "Graph", current: "page" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "More" })).not.toHaveAttribute("aria-current");
    expect(marker()).toHaveLength(1);
  });

  it("supports keyboard pin toggles and Escape returns focus to More without navigation", async () => {
    const user = userEvent.setup();
    const { callbacks } = renderNavigation();
    await user.tab();
    expect(screen.getByRole("button", { name: "Home" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Wiki" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Spaces" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(callbacks.spaces).toHaveBeenCalledExactlyOnceWith(false);
    const more = await openMore(user);
    const graph = pinButton(more, "Pin Graph to sidebar");
    graph.focus();
    await user.keyboard(" ");
    expect(pinButton(more, "Unpin Graph from sidebar")).toBe(graph);
    expect(graph).toHaveAttribute("aria-pressed", "true");
    expect(graph).toHaveFocus();
    await user.keyboard("{Escape}");
    const moreButton = screen.getByRole("button", { name: "More" });
    expect(moreButton).toHaveFocus();
    expect(moreButton).toHaveAttribute("aria-expanded", "false");
    expect(callbacks.graph).not.toHaveBeenCalled();
    await user.keyboard("{Enter}");
    expect(screen.getByRole("group", { name: "More" })).toBeInTheDocument();
  });

  it("omits unwired routes from both lists while preserving saved choices", async () => {
    const user = userEvent.setup();
    writeNavigationPreferences(NAVIGATION_DESTINATION_ORDER);
    const { rerender } = render(<PrimaryNavigation active={null} compact labels={labels} onNavigateSpaces={() => {}} onNavigateSources={() => {}} />);
    expect(railLabels()).toEqual(["Spaces", "Sources", "More"]);
    const more = await openMore(user);
    expect(within(more).queryAllByRole("button").filter((button) => button.classList.contains("notes-more-destination-link")).map((button) => button.getAttribute("aria-label"))).toEqual(["Sources"]);
    expect(within(more).queryByRole("button", { name: "Pin Wiki to sidebar" })).not.toBeInTheDocument();
    expect(within(more).queryByRole("button", { name: "Unpin Spaces from sidebar" })).not.toBeInTheDocument();
    expect(within(more).getByRole("button", { name: "Unpin Sources from sidebar" })).toBeInTheDocument();
    rerender(<PrimaryNavigation active={null} compact labels={labels} onNavigatePages={() => {}} onNavigateSpaces={() => {}} onNavigateSources={() => {}} />);
    expect(railLabels()).toEqual(["Wiki", "Spaces", "Sources", "More"]);
    expect(within(screen.getByRole("group", { name: "More" })).queryAllByRole("button").filter((button) => button.classList.contains("notes-more-destination-link")).map((button) => button.getAttribute("aria-label"))).toEqual(["Sources"]);
  });

  it("keeps pinning usable when preference storage is unavailable", async () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("disabled"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("disabled"); });
    const user = userEvent.setup();
    renderNavigation();
    const more = await openMore(user);
    await user.click(pinButton(more, "Pin Graph to sidebar"));
    expect(railLabels()).toEqual(["Home", "Wiki", "Spaces", "Graph", "More"]);
    expect(screen.getByRole("group", { name: "More" })).toBeInTheDocument();
    expect(pinButton(more, "Unpin Graph from sidebar")).toHaveFocus();
  });

  it("clamps a late More anchor inside a narrow viewport and updates after resize", async () => {
    const user = userEvent.setup();
    writeNavigationPreferences(NAVIGATION_DESTINATION_ORDER);
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      return this.classList.contains("notes-more-anchor")
        ? { left: 37, right: 73, top: 338, bottom: 374, width: 36, height: 36, x: 37, y: 338, toJSON: () => ({}) }
        : { left: 80, right: 344, top: 338, bottom: 638, width: 264, height: 300, x: 80, y: 338, toJSON: () => ({}) };
    });
    vi.stubGlobal("innerWidth", 375);
    vi.stubGlobal("innerHeight", 400);
    renderNavigation();
    await user.click(screen.getByRole("button", { name: "More" }));
    const panel = screen.getByRole("group", { name: "More" });
    expect(panel).toHaveStyle({ left: "80px", top: "92px", maxHeight: "384px" });
    vi.stubGlobal("innerWidth", 280);
    vi.stubGlobal("innerHeight", 280);
    fireEvent(window, new Event("resize"));
    expect(panel).toHaveStyle({ left: "8px", top: "8px", maxHeight: "264px" });
    vi.unstubAllGlobals();
  });

  it("keeps More at its screen position while pinning and repositions only after resize", async () => {
    const user = userEvent.setup();
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("notes-more-anchor")) {
        const railRows = Array.from(this.parentElement?.children ?? []).filter((element) => element !== this).length;
        const top = 100 + railRows * 50;
        return { left: 37, right: 73, top, bottom: top + 36, width: 36, height: 36, x: 37, y: top, toJSON: () => ({}) };
      }
      return { left: 80, right: 344, top: 100, bottom: 400, width: 264, height: 300, x: 80, y: 100, toJSON: () => ({}) };
    });
    vi.stubGlobal("innerWidth", 1280);
    vi.stubGlobal("innerHeight", 1000);
    renderNavigation();
    const more = await openMore(user);
    const panel = screen.getByRole("group", { name: "More" });
    expect(panel).toHaveStyle({ top: "250px" });
    await user.click(pinButton(more, "Pin Graph to sidebar"));
    expect(panel).toHaveStyle({ top: "250px" });
    fireEvent(window, new Event("resize"));
    expect(panel).toHaveStyle({ top: "300px" });
    await user.click(pinButton(more, "Unpin Graph from sidebar"));
    expect(panel).toHaveStyle({ top: "300px" });
    fireEvent(window, new Event("resize"));
    expect(panel).toHaveStyle({ top: "250px" });
    vi.unstubAllGlobals();
  });
});
