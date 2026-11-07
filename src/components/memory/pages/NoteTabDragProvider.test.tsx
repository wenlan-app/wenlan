// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NoteTabDragProvider } from "./NoteTabDragProvider";

function rect(left: number, top: number, width: number, height: number) {
  return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) } as DOMRect;
}

function giveRect(element: Element, bounds: DOMRect) {
  vi.spyOn(element, "getBoundingClientRect").mockReturnValue(bounds);
  vi.spyOn(element, "getClientRects").mockReturnValue({ 0: bounds, length: 1, item: (index: number) => index === 0 ? bounds : null, [Symbol.iterator]: function* () { yield bounds; } } as DOMRectList);
}

function renderSharedDrop(width: number, surface: "tools" | "docked" | "expanded" | "menu" | "dialog", secondary: boolean | "hidden" = false) {
  const onDrop = vi.fn();
  const targetGroupLeft = secondary ? width / 2 : 0;
  const targetGroupWidth = secondary ? width / 2 : width;
  const wrapperClass = surface === "docked" || surface === "dialog" ? "page-info-drawer-group-overlay"
    : surface === "expanded" ? "page-info-drawer-expanded-workspace page-info-drawer-expanded-group" : "";
  const { container } = render(<>
    <div className="memory-shell">
      <div className="wiki-workspace-reading">
        <section data-note-group-id="primary">
          <div className="note-tabs" data-note-tab-group="primary"><div className="note-tabs-list"><div className="note-tab" data-note-tab-key="note:one"><button role="tab">First note</button></div></div></div>
        </section>
        {secondary === "hidden" ? <div hidden><section data-note-group-id="secondary"><div className="note-tabs" data-note-tab-group="secondary"><div className="note-tabs-list"><div className="note-tab" data-note-tab-key="note:two"><button role="tab">Second note</button></div></div></div></section></div>
          : secondary && <section data-note-group-id="secondary"><div className="note-tabs" data-note-tab-group="secondary"><div className="note-tabs-list"><div className="note-tab" data-note-tab-key="note:two"><button role="tab">Second note</button></div></div></div></section>}
        {surface === "tools" || surface === "menu" ? <div className="wiki-shared-tools-rail">{surface === "menu"
          ? <div role="menu"><button type="button" data-testid="drop-target">Menu action</button></div>
          : <button type="button" data-testid="drop-target">Inspector tools</button>}</div>
          : <div className={wrapperClass}><aside className={surface === "dialog" ? "unrelated-dialog" : "page-info-drawer"} role={surface === "dialog" ? "dialog" : "complementary"}><button type="button" data-testid="drop-target">PDF panel</button></aside></div>}
      </div>
    </div>
    <NoteTabDragProvider onDrop={onDrop} />
  </>);
  const root = container.querySelector<HTMLElement>(".wiki-workspace-reading")!;
  const primary = container.querySelector<HTMLElement>('[data-note-group-id="primary"]')!;
  const sourceStrip = primary.querySelector<HTMLElement>(".note-tabs")!;
  const sourceList = sourceStrip.querySelector<HTMLElement>(".note-tabs-list")!;
  const sourceButton = sourceStrip.querySelector<HTMLButtonElement>("[role=tab]")!;
  const target = container.querySelector<HTMLElement>('[data-testid="drop-target"]')!;
  const hitSurface = surface === "tools" || surface === "menu" ? target.closest<HTMLElement>(".wiki-shared-tools-rail")!
    : surface === "dialog" ? target.closest<HTMLElement>("[role=dialog]")!
      : target.closest<HTMLElement>(`.${wrapperClass.split(" ")[0]}`)!;
  const targetX = targetGroupLeft + targetGroupWidth - Math.min(80, targetGroupWidth / 4);
  const targetY = surface === "tools" ? 20 : 140;
  giveRect(root, rect(0, 0, width, 600));
  giveRect(primary, rect(0, 0, secondary ? width / 2 : width, 600));
  giveRect(sourceStrip, rect(0, 0, 200, 52));
  giveRect(sourceList, rect(0, 0, 180, 48));
  if (secondary) giveRect(container.querySelector<HTMLElement>('[data-note-group-id="secondary"]')!, rect(width / 2, 0, width / 2, 600));
  giveRect(hitSurface, rect(targetGroupLeft, surface === "tools" ? 0 : 52, targetGroupWidth, surface === "tools" ? 52 : 548));
  giveRect(target, rect(targetX, targetY, 40, 32));
  Object.defineProperty(document, "elementFromPoint", { configurable: true, value: vi.fn(() => target) });
  fireEvent.pointerDown(sourceButton, { pointerId: 40, pointerType: "mouse", button: 0, clientX: 20, clientY: 20 });
  fireEvent.pointerMove(document, { pointerId: 40, pointerType: "mouse", clientX: targetX + 8, clientY: targetY + 8 });
  const previewElement = container.ownerDocument.querySelector<HTMLElement>(".note-tab-split-preview");
  const preview = previewElement ? { left: previewElement.style.left, top: previewElement.style.top, width: previewElement.style.width, height: previewElement.style.height } : null;
  fireEvent.pointerUp(document, { pointerId: 40, pointerType: "mouse", clientX: targetX + 8, clientY: targetY + 8 });
  return { onDrop, root, primary, target, preview };
}

describe("NoteTabDragProvider", () => {
  afterEach(() => { vi.restoreAllMocks(); Reflect.deleteProperty(document, "elementFromPoint"); });

  it("reports a cross-group drop at the append position after the pointer threshold", () => {
    const onDrop = vi.fn();
    const { container } = render(<>
      <div className="memory-shell">
        <section data-note-group-id="primary">
          <div className="note-tabs" data-note-tab-group="primary"><div className="note-tabs-list"><div className="note-tab" data-note-tab-key="note:one"><button role="tab">First note</button></div></div></div>
        </section>
        <section data-note-group-id="secondary">
          <div className="note-tabs" data-note-tab-group="secondary"><div className="note-tabs-list"><div className="note-tab" data-note-tab-key="note:two"><button role="tab">Second note</button></div></div></div>
        </section>
      </div>
      <NoteTabDragProvider onDrop={onDrop} />
    </>);
    const sourceStrip = container.querySelector<HTMLElement>('[data-note-tab-group="primary"]')!;
    const sourceFrame = container.querySelector<HTMLElement>('[data-note-group-id="primary"]')!;
    const sourceList = sourceStrip.querySelector<HTMLElement>(".note-tabs-list")!;
    const targetStrip = container.querySelector<HTMLElement>('[data-note-tab-group="secondary"]')!;
    const targetFrame = container.querySelector<HTMLElement>('[data-note-group-id="secondary"]')!;
    const targetList = targetStrip.querySelector<HTMLElement>(".note-tabs-list")!;
    const targetWrapper = targetStrip.querySelector<HTMLElement>("[data-note-tab-key]")!;
    const sourceButton = sourceStrip.querySelector<HTMLButtonElement>("[role=tab]")!;
    giveRect(sourceStrip, rect(0, 0, 190, 48));
    giveRect(sourceFrame, rect(0, 0, 190, 200));
    giveRect(sourceList, rect(0, 0, 160, 40));
    giveRect(targetStrip, rect(200, 0, 190, 48));
    giveRect(targetFrame, rect(200, 0, 190, 200));
    giveRect(targetList, rect(200, 0, 180, 40));
    giveRect(targetWrapper, rect(220, 4, 90, 36));
    Object.defineProperty(document, "elementFromPoint", { configurable: true, value: vi.fn((x: number, y: number) => x >= 200 && y < 48 ? targetWrapper : sourceButton) });

    fireEvent.pointerDown(sourceButton, { pointerId: 9, pointerType: "mouse", button: 0, clientX: 20, clientY: 20 });
    fireEvent.pointerMove(document, { pointerId: 9, pointerType: "mouse", clientX: 330, clientY: 20 });
    expect(document.querySelector(".note-tab-drag-ghost")).toBeInTheDocument();
    fireEvent.pointerUp(document, { pointerId: 9, pointerType: "mouse", clientX: 330, clientY: 20 });

    expect(onDrop).toHaveBeenCalledOnce();
    expect(onDrop).toHaveBeenCalledWith({ sourceGroup: "primary", targetGroup: "secondary", tabKey: "note:one", beforeKey: null });
    expect(document.querySelector(".note-tab-drag-ghost")).not.toBeInTheDocument();
  });

  it("does not call onDrop for a pointer movement below the drag threshold", () => {
    const onDrop = vi.fn();
    const { container } = render(<>
      <div className="memory-shell"><section data-note-group-id="primary"><div className="note-tabs" data-note-tab-group="primary"><div className="note-tabs-list"><div className="note-tab" data-note-tab-key="note:one"><button role="tab">First note</button></div></div></div></section></div>
      <NoteTabDragProvider onDrop={onDrop} />
    </>);
    const strip = container.querySelector<HTMLElement>(".note-tabs")!;
    const frame = container.querySelector<HTMLElement>("[data-note-group-id=primary]")!;
    const button = strip.querySelector<HTMLButtonElement>("[role=tab]")!;
    giveRect(strip, rect(0, 0, 190, 48));
    giveRect(frame, rect(0, 0, 190, 120));
    fireEvent.pointerDown(button, { pointerId: 2, pointerType: "mouse", button: 0, clientX: 10, clientY: 10 });
    fireEvent.pointerMove(document, { pointerId: 2, pointerType: "mouse", clientX: 14, clientY: 12 });
    fireEvent.pointerUp(document, { pointerId: 2, pointerType: "mouse", clientX: 14, clientY: 12 });
    expect(onDrop).not.toHaveBeenCalled();
  });

  it.each(["tools", "docked", "expanded"] as const)("creates a secondary split when dropping on shared %s with only the primary Wiki open", surface => {
    const { onDrop, preview } = renderSharedDrop(800, surface);
    expect(onDrop).toHaveBeenCalledWith({ sourceGroup: "primary", targetGroup: "secondary", tabKey: "note:one", beforeKey: null });
    expect(preview).toEqual({ left: "408px", top: "8px", width: "384px", height: "584px" });
  });

  it("transfers a primary tab to an existing secondary Wiki from the shared inspector", () => {
    const { onDrop } = renderSharedDrop(800, "docked", true);
    expect(onDrop).toHaveBeenCalledWith({ sourceGroup: "primary", targetGroup: "secondary", tabKey: "note:one", beforeKey: null });
  });

  it("does not treat a hidden secondary host as an open destination", () => {
    const { onDrop } = renderSharedDrop(800, "docked", "hidden");
    expect(onDrop).toHaveBeenCalledWith({ sourceGroup: "primary", targetGroup: "secondary", tabKey: "note:one", beforeKey: null });
  });

  it("uses narrow shared-pane geometry to preview a stacked secondary group", () => {
    const { onDrop, preview } = renderSharedDrop(375, "docked");
    expect(onDrop).toHaveBeenCalledWith({ sourceGroup: "primary", targetGroup: "secondary", tabKey: "note:one", beforeKey: null });
    expect(preview).toEqual({ left: "8px", top: "308px", width: "359px", height: "284px" });
  });

  it.each(["menu", "dialog"] as const)("keeps %s drops in the shared pane excluded", surface => {
    const { onDrop } = renderSharedDrop(800, surface);
    expect(onDrop).not.toHaveBeenCalled();
  });
});
