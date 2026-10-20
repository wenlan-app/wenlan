import { QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SpacesOverview } from "./SpacesOverview";
import { createQueryClient, labels, makeSpace, renderOverview } from "./SpacesOverview.testUtils";

const api = vi.hoisted(() => ({
  listSpaces: vi.fn(),
  listPages: vi.fn(),
  createSpace: vi.fn(),
  updateSpace: vi.fn(),
  deleteSpace: vi.fn(),
  confirmSpace: vi.fn(),
  reorderSpace: vi.fn(),
  toggleSpaceStarred: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => api);

const work = makeSpace({ id: "work", name: "Work", sort_order: 0 });
const personal = makeSpace({ id: "personal", name: "Personal", sort_order: 1 });
const starred = makeSpace({ id: "starred", name: "Starred", starred: true, sort_order: 0 });

describe("SpacesOverview cards lens", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.localStorage.removeItem("wenlan-spaces-view-mode");
    api.listSpaces.mockResolvedValue([work, personal, starred]);
    api.listPages.mockResolvedValue([
      { id: "page-1", title: "One", space: "Work", domain: null },
      { id: "page-2", title: "Two", domain: "Work" },
    ]);
    api.updateSpace.mockResolvedValue(work);
    api.deleteSpace.mockResolvedValue(undefined);
    api.reorderSpace.mockResolvedValue(undefined);
    api.toggleSpaceStarred.mockResolvedValue(true);
  });

  it("renders cards by default with the mark, star, description, page count, and no drag handle", async () => {
    // Given no stored lens preference
    window.localStorage.removeItem("wenlan-spaces-view-mode");
    const queryClient = createQueryClient();
    render(
      <QueryClientProvider client={queryClient}>
        <SpacesOverview labels={labels} onSelectSpace={() => undefined} />
      </QueryClientProvider>,
    );

    // When the confirmed inventory resolves
    const cards = await screen.findByTestId("spaces-cards");
    expect(screen.queryByRole("columnheader")).not.toBeInTheDocument();

    // Then cards follow the same sorted order as rows, starting with the starred space
    const rendered = within(cards).getAllByRole("article").map((card) => card.getAttribute("data-testid"));
    expect(rendered).toEqual(["space-card-starred", "space-card-work", "space-card-personal"]);

    const workCard = screen.getByTestId("space-card-work");
    expect(workCard.querySelector("[data-space-mark]")).not.toBeNull();

    const starredCard = screen.getByTestId("space-card-starred");
    expect(within(starredCard).getByText("★")).toBeInTheDocument();
    expect(within(workCard).queryByText("★")).not.toBeInTheDocument();

    expect(within(workCard).getByText("Projects and planning")).toBeInTheDocument();
    expect(within(workCard).getByTestId("space-card-pages")).toHaveTextContent("2 pages");
    expect(within(workCard).queryByTestId("space-card-memories")).not.toBeInTheDocument();
    expect(within(workCard).queryByTestId("space-card-entities")).not.toBeInTheDocument();
    expect(workCard.querySelector("time")).toBeNull();
    expect(screen.queryByRole("button", { name: labels.dragSpace("Work") })).not.toBeInTheDocument();
    expect(cards.querySelector(".spaces-drag-handle")).toBeNull();
  });

  it("opens the space detail from a card", async () => {
    const onSelectSpace = vi.fn();
    renderOverview({ onSelectSpace }, undefined, { lens: "cards" });

    fireEvent.click(await screen.findByRole("button", { name: "Open Work" }));

    expect(onSelectSpace).toHaveBeenCalledWith("Work");
  });

  it("stars from a card menu", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("space-card-work");

    fireEvent.click(screen.getByRole("button", { name: labels.actionsFor("Work") }));
    fireEvent.click(screen.getByRole("menuitem", { name: labels.star }));

    await waitFor(() => expect(api.toggleSpaceStarred).toHaveBeenCalledWith("Work"));
  });

  it("renames from a card menu through the inline editor", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("space-card-work");

    fireEvent.click(screen.getByRole("button", { name: labels.actionsFor("Work") }));
    fireEvent.click(screen.getByRole("menuitem", { name: labels.rename }));

    const card = screen.getByTestId("space-card-work");
    expect(card).toHaveClass("space-card-edit");
    const nameInput = within(card).getByLabelText(labels.nameLabel);
    fireEvent.change(nameInput, { target: { value: "Studio" } });
    fireEvent.keyDown(nameInput, { key: "Enter" });

    await waitFor(() => expect(api.updateSpace).toHaveBeenCalledWith("Work", "Studio", "Projects and planning"));
  });

  it("deletes from a card menu after confirmation", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("space-card-work");

    fireEvent.click(screen.getByRole("button", { name: labels.actionsFor("Work") }));
    fireEvent.click(screen.getByRole("menuitem", { name: labels.delete }));
    expect(api.deleteSpace).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: labels.confirmDelete }));

    await waitFor(() => expect(api.deleteSpace).toHaveBeenCalledWith("Work"));
  });

  it("moves a card down through the menu reorder command", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("space-card-work");

    fireEvent.click(screen.getByRole("button", { name: labels.actionsFor("Work") }));
    fireEvent.click(screen.getByRole("menuitem", { name: labels.moveDown }));

    await waitFor(() => expect(api.reorderSpace).toHaveBeenCalledWith("Work", 1));
  });

  it("toggling to Rows persists the preference and restores the labelled list", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("spaces-cards");

    fireEvent.click(screen.getByRole("button", { name: "Rows" }));

    expect(await screen.findByTestId("space-row-work")).toBeInTheDocument();
    expect(screen.queryByRole("columnheader")).not.toBeInTheDocument();
    expect(screen.queryByTestId("spaces-cards")).not.toBeInTheDocument();
    expect(window.localStorage.getItem("wenlan-spaces-view-mode")).toBe("rows");
    expect(screen.getByRole("button", { name: "Rows" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Cards" })).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(screen.getByRole("button", { name: "Cards" }));

    expect(await screen.findByTestId("spaces-cards")).toBeInTheDocument();
    expect(screen.queryByRole("columnheader")).not.toBeInTheDocument();
    expect(window.localStorage.getItem("wenlan-spaces-view-mode")).toBe("cards");
    expect(screen.getByRole("button", { name: "Cards" })).toHaveAttribute("aria-pressed", "true");
  });

  it("keeps the lens toggle visible when the filter matches nothing", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("space-card-work");

    fireEvent.change(screen.getByLabelText(labels.filterLabel), { target: { value: "zzz" } });

    expect(screen.getByText(labels.noResults)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cards" })).toBeInTheDocument();
  });

  it("omits absent descriptions and extra metadata for a space that was never updated", async () => {
    api.listSpaces.mockResolvedValue([
      makeSpace({ id: "bare", name: "Bare", description: null, updated_at: 0, memory_count: 1, entity_count: 1 }),
    ]);
    renderOverview({}, undefined, { lens: "cards" });

    const card = await screen.findByTestId("space-card-bare");
    expect(card.querySelector(".asset-card-context")).toBeNull();
    expect(card.querySelector("time")).toBeNull();
    expect(within(card).queryByTestId("space-card-memories")).not.toBeInTheDocument();
    expect(within(card).queryByTestId("space-card-entities")).not.toBeInTheDocument();
    expect(within(card).getByTestId("space-card-pages")).toHaveTextContent("0 pages");
  });

  it("filters cards by name", async () => {
    renderOverview({}, undefined, { lens: "cards" });
    await screen.findByTestId("space-card-work");

    fireEvent.change(screen.getByLabelText(labels.filterLabel), { target: { value: "personal" } });

    expect(screen.queryByTestId("space-card-work")).not.toBeInTheDocument();
    expect(screen.getByTestId("space-card-personal")).toBeInTheDocument();
  });
});
