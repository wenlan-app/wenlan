// SPDX-License-Identifier: AGPL-3.0-only
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { pageLayersEditing, setPageLayers, type PageLayersSettings } from "./pageLayersEditing";

const DOC = "The point.\n\n## Why\n\nReasoning.\n\n## Details for AI\n\nRaw notes.";
const labels = { aiDetails: "Details for AI", showFull: "Show the full page" };
let view: EditorView | null = null;

function mount(settings: PageLayersSettings | null) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  view = new EditorView({ parent, state: EditorState.create({ doc: DOC, extensions: [pageLayersEditing(settings)] }) });
  return view;
}

const visibleText = (v: EditorView) => v.contentDOM.textContent ?? "";

afterEach(() => {
  view?.destroy();
  view = null;
  document.body.innerHTML = "";
});

describe("pageLayersEditing", () => {
  it("UX-VIEW-3 folds the part for AI into one line until it is opened, keeping the text whole", () => {
    const v = mount({ length: "full", labels });
    expect(visibleText(v)).toContain("Reasoning.");
    expect(visibleText(v)).not.toContain("Raw notes.");
    const fold = v.dom.querySelector<HTMLButtonElement>("[data-testid=page-ai-details-fold]");
    expect(fold?.textContent).toBe("Details for AI");
    fold?.click();
    expect(visibleText(v)).toContain("Raw notes.");
    expect(v.state.doc.toString()).toBe(DOC);
  });

  it("UX-VIEW-2 folds the sections for the short length and opens them from the text", () => {
    const onShowFull = vi.fn();
    const v = mount({ length: "short", labels, onShowFull });
    expect(visibleText(v)).toContain("The point.");
    expect(visibleText(v)).not.toContain("Reasoning.");
    v.dom.querySelector<HTMLButtonElement>("[data-testid=page-short-fold]")?.click();
    expect(onShowFull).toHaveBeenCalledOnce();
    v.dispatch({ effects: setPageLayers.of({ length: "full", labels }) });
    expect(visibleText(v)).toContain("Reasoning.");
  });

  it("shows everything without settings", () => {
    expect(visibleText(mount(null))).toContain("Raw notes.");
  });
});
