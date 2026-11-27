import { describe, expect, it } from "vitest";
import { activeNavigationForView } from "./viewState";

describe("activeNavigationForView", () => {
  it("keeps Home, the first-use guide and things under Home, apart from the Wiki", () => {
    expect(activeNavigationForView({ kind: "home" })).toBe("home");
    expect(activeNavigationForView({ kind: "first-use" })).toBe("home");
    expect(activeNavigationForView({
      kind: "first-use",
      showKnowledge: true,
      batchId: "batch-1",
    })).toBe("home");
    expect(activeNavigationForView({ kind: "thing", thingId: "thing-1" })).toBe("home");
  });
  it("keeps the Page library and Page detail first-class instead of nesting them under Spaces", () => {
    expect(activeNavigationForView({ kind: "pages" })).toBe("pages");
    expect(activeNavigationForView({ kind: "page", pageId: "page-1" })).toBe("pages");
    expect(activeNavigationForView({
      kind: "page-draft",
      draftId: "draft-1",
      space: "Launch",
    })).toBe("pages");
    expect(activeNavigationForView({ kind: "distill-review" })).toBe("pages");
    expect(activeNavigationForView({ kind: "space", spaceId: "space-1", spaceName: "Launch" })).toBe("spaces");
  });
  it("lights each section's own row on its list page, and Skills on Find skills", () => {
    expect(activeNavigationForView({ kind: "pages", section: "docs" })).toBe("pages");
    expect(activeNavigationForView({ kind: "pages", inventoryScope: "folder:Work" })).toBe("pages");
    expect(activeNavigationForView({ kind: "pages", section: "skills" })).toBe("skills");
    expect(activeNavigationForView({ kind: "pages", section: "rules" })).toBe("rules");
    expect(activeNavigationForView({ kind: "skills" })).toBe("skills");
  });
  it("lights Spaces on a joined team's page, which opens from the Spaces list", () => {
    expect(activeNavigationForView({ kind: "team-page", teamId: "team-1", pageId: "page-1" })).toBe("spaces");
  });
  it("lights the graph on the topic list, which opens from it", () => {
    expect(activeNavigationForView({ kind: "entities" })).toBe("graph");
    expect(activeNavigationForView({ kind: "entity", entityId: "entity-1" })).toBe("entities");
  });
});
