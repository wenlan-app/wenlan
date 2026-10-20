// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import type { GraphEdge, GraphNode } from "../../lib/graph/model";
import AtlasInspector from "./AtlasInspector";

const node = (id: string, kind: GraphNode["kind"], name: string, entityType = "person"): GraphNode => ({
  id, kind, name, entityType, confirmed: true, degree: 1, space: "test", createdAt: 1, updatedAt: 1,
});

const selected = node("entity-selected", "entity", "Selected", "person");
const neighbors = [
  node("page-1", "page", "Research page"),
  node("entity-1", "entity", "Ada Lovelace", "person"),
  node("memory-1", "memory", "Launch page"),
];
const edges: GraphEdge[] = [
  { id: "edge-page", source: selected.id, target: "page-1", type: "documents", confidence: null, createdAt: 1 },
  { id: "edge-entity", source: "entity-1", target: selected.id, type: "inspired", confidence: null, createdAt: 1 },
  { id: "edge-memory", source: selected.id, target: "memory-1", type: "mentions", confidence: null, createdAt: 1 },
];

function renderInspector(overrides: Partial<React.ComponentProps<typeof AtlasInspector>> = {}) {
  const onSelect = vi.fn();
  const onClose = vi.fn();
  return {
    user: userEvent.setup(),
    onSelect,
    onClose,
    ...render(<AtlasInspector node={selected} neighbors={neighbors} edges={edges} onSelect={onSelect} onClose={onClose} {...overrides} />),
  };
}

beforeEach(async () => { await i18n.changeLanguage("en"); });

describe("AtlasInspector", () => {
  it("groups connections in page, entity, memory order and shows directional relation context", () => {
    renderInspector();
    const sections = screen.getAllByRole("region");
    expect(sections.map((section) => within(section).getByRole("heading").textContent?.replace(/\s+\d+$/, ""))).toEqual([
      "Pages", "Topics", "Memories",
    ]);
    expect(within(sections[0]).getByText("documents")).toBeInTheDocument();
    expect(within(sections[1]).getByText("inspired")).toBeInTheDocument();
    expect(screen.getByText("Person")).toBeInTheDocument();
    expect(within(sections[2]).getByText("mentions")).toBeInTheDocument();
  });

  it("keeps the single-group heading accessible without a redundant jump summary", () => {
    const many = Array.from({ length: 13 }, (_, index) => node(`entity-${index}`, "entity", `Person ${index}`, "person"));
    renderInspector({ neighbors: many, edges: [] });
    expect(screen.getByRole("heading", { name: "Connections 13", level: 3 })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Connections" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Topics 13", level: 4 })).toBeInTheDocument();
    expect(screen.getByText("Person")).toBeInTheDocument();
  });

  it("shows group jump links for a large mixed result and keeps the group order", () => {
    const many = [
      ...Array.from({ length: 5 }, (_, index) => node(`page-${index}`, "page", `Page ${index}`)),
      ...Array.from({ length: 5 }, (_, index) => node(`entity-${index}`, "entity", `Person ${index}`, "person")),
      ...Array.from({ length: 5 }, (_, index) => node(`memory-${index}`, "memory", `Memory ${index}`)),
    ];
    renderInspector({ neighbors: many, edges: [] });
    const summary = screen.getByRole("navigation", { name: "Connections" });
    expect(within(summary).getByRole("button", { name: "Pages 5" })).toBeInTheDocument();
    expect(within(summary).getByRole("button", { name: "Topics 5" })).toBeInTheDocument();
    expect(within(summary).getByRole("button", { name: "Memories 5" })).toBeInTheDocument();
    const sections = screen.getAllByRole("region");
    expect(sections.map((section) => within(section).getByRole("heading").textContent?.replace(/\s+\d+$/, ""))).toEqual([
      "Pages", "Topics", "Memories",
    ]);
  });

  it("filters by relation type and keeps neighbor button names stable", async () => {
    const { user } = renderInspector({
      neighbors: Array.from({ length: 13 }, (_, index) => node(`entity-${index}`, "entity", `Person ${index}`, "person")),
      edges: [{ id: "edge-11", source: selected.id, target: "entity-11", type: "references", confidence: null, createdAt: 1 }],
    });
    const input = screen.getByRole("textbox", { name: "Filter connections" });
    await user.type(input, "references");
    expect(screen.getByRole("button", { name: "Person 11" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Person 0" })).not.toBeInTheDocument();
  });

  it("selects a connection through the existing callback", async () => {
    const { user, onSelect } = renderInspector();
    await user.click(screen.getByRole("button", { name: "Ada Lovelace" }));
    expect(onSelect).toHaveBeenCalledWith("entity-1");
  });

  it("reveals twelve more rows per group", async () => {
    const many = Array.from({ length: 25 }, (_, index) => node(`entity-${index}`, "entity", `Person ${index}`, "person"));
    const { user } = renderInspector({ neighbors: many });
    const entitySection = screen.getByRole("region", { name: /Topics/ });
    expect(within(entitySection).getAllByRole("button")).toHaveLength(13);
    await user.click(within(entitySection).getByRole("button", { name: "Show more" }));
    expect(within(entitySection).getAllByRole("button")).toHaveLength(25);
  });

  it("uses the shared drawer title and a single return-to-map close control", async () => {
    const onOpen = vi.fn();
    const { user, onClose } = renderInspector({ onOpen });

    expect(screen.getByRole("dialog", { name: "Selected" })).toBeInTheDocument();
    const close = screen.getByRole("button", { name: "Return to full map" });
    expect(screen.getAllByRole("button", { name: "Return to full map" })).toHaveLength(1);
    const details = screen.getByRole("button", { name: "Open details" });
    expect(details.parentElement?.className).toContain("atlas-inspector-metadata");

    await user.click(details);
    expect(onOpen).toHaveBeenCalledOnce();
    await user.click(close);
    expect(onClose).toHaveBeenCalledOnce();
  });
});
