// SPDX-License-Identifier: AGPL-3.0-only
import { StrictMode, useEffect, useState } from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import PageInfoDrawer from "./PageInfoDrawer";
import { WorkspacePaneHostContext } from "../navigation/WorkspacePaneHost";
import { NoteGroupFrame } from "../pages/NoteGroupFrame";

function Harness() {
  const [open, setOpen] = useState(false);
  return <>
    <button onClick={() => setOpen(true)} type="button">Page information</button>
    <PageInfoDrawer closeLabel="Close information" onClose={() => setOpen(false)} open={open} title="Page information">
      <button disabled tabIndex={0} type="button">Unavailable source</button>
      <button hidden type="button">Hidden source</button>
      <a href="#source">Open source</a>
    </PageInfoDrawer>
  </>;
}

function InspectorHarness() {
  const [open, setOpen] = useState(false);
  return <>
    <button onClick={() => setOpen(true)} type="button">Open note inspector</button>
    <PageInfoDrawer
      closeLabel="Close inspector"
      headerContent={<div aria-label="Note views" role="tablist"><button aria-selected="true" role="tab" type="button">Info</button></div>}
      inspector
      onClose={() => setOpen(false)}
      open={open}
      title="Note inspector"
    >
      <a href="#source">Open source</a>
    </PageInfoDrawer>
  </>;
}

function CanvasProbe({ onMount, onUnmount }: { onMount: () => void; onUnmount: () => void }) {
  const [draft, setDraft] = useState("saved map label");
  useEffect(() => {
    onMount();
    return onUnmount;
  }, [onMount, onUnmount]);
  return <input aria-label="Map label" onChange={(event) => setDraft(event.target.value)} value={draft} />;
}

function ExpandableInspectorHarness({ onMount, onUnmount }: { onMount: () => void; onUnmount: () => void }) {
  const [expanded, setExpanded] = useState(false);
  const [variant, setVariant] = useState<"info" | "canvas">("canvas");
  const [sidebarHost, setSidebarHost] = useState<HTMLDivElement | null>(null);
  const [workspaceHost, setWorkspaceHost] = useState<HTMLDivElement | null>(null);
  return <WorkspacePaneHostContext.Provider value={sidebarHost}>
    <div data-testid="sidebar-host" ref={setSidebarHost} />
    <div data-testid="article-workspace-host" ref={setWorkspaceHost} />
    <PageInfoDrawer
      closeLabel="Close map"
      docked
      expanded={expanded}
      expandedHost={workspaceHost}
      expandLabel="Expand workspace"
      headerContent={<div aria-label="Note views" role="tablist"><button onClick={() => setVariant(v => v === "canvas" ? "info" : "canvas")} aria-selected="true" role="tab" type="button">Switch view</button></div>}
      inspector
      onClose={() => {}}
      onExpandedChange={setExpanded}
      open
      restoreLabel="Restore sidebar"
      title="Note inspector"
      variant={variant}
    >
      <CanvasProbe onMount={onMount} onUnmount={onUnmount} />
    </PageInfoDrawer>
  </WorkspacePaneHostContext.Provider>;
}

function GroupExpandableInspectorHarness() {
  const [expanded, setExpanded] = useState(false);
  const [externalHost, setExternalHost] = useState<HTMLDivElement | null>(null);
  return <>
    <div data-testid="external-expanded-host" ref={setExternalHost} />
    <NoteGroupFrame id="expandable" label="Expandable note" tabs={<button>Note tab</button>}>
      <PageInfoDrawer docked inspector open expanded={expanded} expandedHost={externalHost}
        onExpandedChange={setExpanded} expandLabel="Expand group" restoreLabel="Restore group"
        onClose={() => {}} title="Group inspector" closeLabel="Close group">
        <input aria-label="Group map label" defaultValue="unfinished map draft" />
      </PageInfoDrawer>
    </NoteGroupFrame>
  </>;
}

describe("PageInfoDrawer", () => {
  it.each([true, false])("preserves reader focus only for automatic docked context (wide=%s)", (wide) => {
    const original = window.matchMedia;
    window.matchMedia = vi.fn().mockReturnValue({ matches: wide, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    const trigger = document.createElement("button");
    document.body.append(trigger);
    trigger.focus();
    try {
      const { unmount } = render(<PageInfoDrawer docked open focusOnOpen={false} onClose={() => {}} title="Info" closeLabel="Close info"><button>Source</button></PageInfoDrawer>);
      if (wide) {
        expect(screen.getByRole("complementary", { name: "Info" })).toBeInTheDocument();
        expect(trigger).toHaveFocus();
      } else {
        expect(screen.getByRole("dialog", { name: "Info" })).toHaveAttribute("aria-modal", "true");
        expect(screen.getByRole("button", { name: "Close info" })).toHaveFocus();
      }
      unmount();
    } finally {
      trigger.remove();
      window.matchMedia = original;
    }
  });

  it("returns Escape focus to the named opener when docked context opened on its own", async () => {
    const original = window.matchMedia;
    window.matchMedia = vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() });
    // The real Info toggle is portaled into the document tools host, so its
    // ref stays attached while StrictMode re-runs the drawer's focus effect.
    const toggle = document.createElement("button");
    toggle.textContent = "Info";
    document.body.append(toggle);
    const returnFocusRef = { current: toggle };
    function DefaultOpen() {
      const [open, setOpen] = useState(true);
      return <PageInfoDrawer docked inspector open={open} focusOnOpen={false} returnFocusRef={returnFocusRef} onClose={() => setOpen(false)} title="Info" closeLabel="Close info"><button>Source</button></PageInfoDrawer>;
    }
    try {
      const user = userEvent.setup();
      render(<StrictMode><DefaultOpen /></StrictMode>);
      // Re-running the effect while the pane stays open leaves reader focus alone.
      expect(document.body).toHaveFocus();
      screen.getByRole("button", { name: "Source" }).focus();
      await user.keyboard("{Escape}");
      expect(screen.queryByRole("complementary", { name: "Info" })).not.toBeInTheDocument();
      expect(toggle).toHaveFocus();
    } finally {
      toggle.remove();
      window.matchMedia = original;
    }
  });

  it("does not mount closed content and portals the labelled modal to body when opened", async () => {
    const user = userEvent.setup();
    const { container } = render(<Harness />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.queryByText("Open source")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Page information" }));
    const drawer = screen.getByRole("dialog", { name: "Page information" });
    expect(drawer).toHaveAttribute("aria-modal", "true");
    expect(container.contains(drawer)).toBe(false);
    expect(screen.getByRole("button", { name: "Close information" })).toHaveFocus();
  });

  it("traps Tab around enabled visible controls and restores the trigger on Escape", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Page information" });
    await user.click(trigger);
    const close = screen.getByRole("button", { name: "Close information" });
    await user.tab({ shift: true });
    expect(screen.getByRole("link", { name: "Open source" })).toHaveFocus();
    await user.tab();
    expect(close).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("closes on the backdrop while clicks inside stay open", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("button", { name: "Page information" }));
    const dialog = screen.getByRole("dialog");
    await user.click(screen.getByRole("heading", { name: "Page information" }));
    expect(dialog).toBeInTheDocument();
    fireEvent.click(dialog.parentElement!);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("contains programmatic background focus and leaves composing Escape alone", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Page information" });
    await user.click(trigger);
    trigger.focus();
    const close = screen.getByRole("button", { name: "Close information" });
    expect(close).toHaveFocus();
    fireEvent.keyDown(close, { key: "Escape", isComposing: true });
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("keeps focus and scroll position stable when callback identity changes", async () => {
    const trigger = document.createElement("button");
    document.body.append(trigger);
    trigger.focus();
    const firstClose = vi.fn();
    const nextClose = vi.fn();
    const props = { open: true, title: "Information", closeLabel: "Close", children: <a href="#source">Source</a> };
    const { rerender, unmount } = render(<PageInfoDrawer {...props} onClose={firstClose} />);
    const source = screen.getByRole("link", { name: "Source" });
    source.focus();
    document.documentElement.scrollTop = 123;
    rerender(<PageInfoDrawer {...props} onClose={nextClose} />);
    expect(source).toHaveFocus();
    expect(document.documentElement.scrollTop).toBe(123);
    fireEvent.keyDown(source, { key: "Escape" });
    expect(nextClose).toHaveBeenCalledOnce();
    expect(firstClose).not.toHaveBeenCalled();
    unmount();
    expect(trigger).toHaveFocus();
    trigger.remove();
    document.documentElement.scrollTop = 0;
  });
});

it("moves the same map subtree between the sidebar and article workspace", async () => {
  const original = window.matchMedia;
  window.matchMedia = vi.fn().mockReturnValue({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() });
  const onMount = vi.fn();
  const onUnmount = vi.fn();
  const user = userEvent.setup();
  try {
    render(<ExpandableInspectorHarness onMount={onMount} onUnmount={onUnmount} />);
    const sidebarHost = screen.getByTestId("sidebar-host");
    const workspaceHost = screen.getByTestId("article-workspace-host");
    const originalInput = within(sidebarHost).getByRole("textbox", { name: "Map label" });
    await user.clear(originalInput);
    await user.type(originalInput, "unfinished draft");
    expect(onMount).toHaveBeenCalledOnce();

    await user.click(within(sidebarHost).getByRole("button", { name: "Expand workspace" }));
    const expandedPanel = within(workspaceHost).getByRole("complementary", { name: "Note inspector" });
    const restore = within(expandedPanel).getByRole("button", { name: "Restore sidebar" });
    expect(within(workspaceHost).getByRole("textbox", { name: "Map label" })).toBe(originalInput);
    expect(originalInput).toHaveValue("unfinished draft");
    expect(restore).toHaveFocus();
    expect(onMount).toHaveBeenCalledOnce();
    expect(onUnmount).not.toHaveBeenCalled();
    await user.tab();
    expect(within(expandedPanel).getByRole("button", { name: "Close map" })).toHaveFocus();

    await user.click(within(expandedPanel).getByRole("tab", { name: "Switch view" }));
    expect(within(workspaceHost).getByRole("complementary", { name: "Note inspector" })).toBe(expandedPanel);
    expect(originalInput).toHaveValue("unfinished draft");
    expect(onUnmount).not.toHaveBeenCalled();

    await user.click(restore);
    expect(within(sidebarHost).getByRole("complementary", { name: "Note inspector" })).toBeInTheDocument();
    expect(within(sidebarHost).getByRole("textbox", { name: "Map label" })).toBe(originalInput);
    expect(originalInput).toHaveValue("unfinished draft");
    expect(onMount).toHaveBeenCalledOnce();
    expect(onUnmount).not.toHaveBeenCalled();
  } finally {
    window.matchMedia = original;
  }
});


it("keeps wide docked context nonmodal without capturing editor focus or Escape", async () => {
  const original = window.matchMedia;
  window.matchMedia = vi.fn().mockReturnValue({matches:true,addEventListener:vi.fn(),removeEventListener:vi.fn()});
  try {
    const close = vi.fn();
    render(<><button>Editor target</button><PageInfoDrawer docked open onClose={close} title="Context" closeLabel="Close"><button>Source</button></PageInfoDrawer></>);
    const panel = screen.getByRole("complementary",{name:"Context"});
    expect(panel).not.toHaveAttribute("aria-modal");
    const editor = screen.getByRole("button",{name:"Editor target"});
    editor.focus(); expect(editor).toHaveFocus();
    fireEvent.keyDown(editor,{key:"Escape"});expect(close).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("button",{name:"Source"}),{key:"Escape"});expect(close).toHaveBeenCalledOnce();
  } finally { window.matchMedia = original; }
});


it("tabs through open disclosure actions and skips collapsed descendants", async () => {
  const user = userEvent.setup();
  // jsdom does not expose the native summary tab stop; browser coverage uses real summaries.
  render(<PageInfoDrawer open title="Context" closeLabel="Close" onClose={vi.fn()}>
    <details open><summary tabIndex={0}>Local graph</summary><button>Open linked note</button></details>
    <details><summary tabIndex={0}>Source</summary><button>Hidden source action</button></details>
  </PageInfoDrawer>);
  await user.tab();
  expect(screen.getByText("Local graph")).toHaveFocus();
  await user.tab();
  expect(screen.getByRole("button", { name: "Open linked note" })).toHaveFocus();
  await user.tab();
  expect(screen.getByText("Source", { exact: true })).toHaveFocus();
  await user.tab();
  expect(screen.getByRole("button", { name: "Close" })).toHaveFocus();
  await user.tab({ shift: true });
  expect(screen.getByText("Source", { exact: true })).toHaveFocus();
});

it("lets nested map editors and menus consume Escape before closing the panel", () => {
  const close = vi.fn();
  render(<PageInfoDrawer open variant="canvas" title="Mind map" closeLabel="Close" onClose={close}>
    <input aria-label="Map label" onKeyDown={(event) => {
      if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); }
    }} />
    <div role="menu" onKeyDown={(event) => { if (event.key === "Escape") event.preventDefault(); }}>
      <button role="menuitem">Map action</button>
    </div>
  </PageInfoDrawer>);
  fireEvent.keyDown(screen.getByRole("textbox", { name: "Map label" }), { key: "Escape" });
  expect(close).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("menuitem", { name: "Map action" }), { key: "Escape" });
  expect(close).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("button", { name: "Close" }), { key: "Escape" });
  expect(close).toHaveBeenCalledOnce();
  expect(screen.getByRole("dialog", { name: "Mind map" })).toHaveClass("page-info-drawer--canvas");
});
it("keeps an accessible drawer title with custom inspector header content and restores trigger focus", async () => {
  const user = userEvent.setup();
  render(<InspectorHarness />);
  const trigger = screen.getByRole("button", { name: "Open note inspector" });
  await user.click(trigger);
  const panel = screen.getByRole("dialog", { name: "Note inspector" });
  const heading = within(panel).getByRole("heading", { name: "Note inspector" });
  const tablist = within(panel).getByRole("tablist", { name: "Note views" });
  const close = within(panel).getByRole("button", { name: "Close inspector" });

  expect(panel).toHaveClass("page-info-drawer--inspector");
  expect(heading).toHaveClass("sr-only");
  expect(tablist.compareDocumentPosition(close) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(close).toHaveFocus();
  await user.keyboard("{Escape}");
  expect(trigger).toHaveFocus();
});

it("keeps two group inspectors local, nonmodal, and isolated for Escape and focus", () => {
  const closeLeft = vi.fn();
  const closeRight = vi.fn();
  const originalRect = HTMLElement.prototype.getBoundingClientRect;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains("note-group-frame")) {
      return { x: 0, y: 0, top: 0, left: 0, right: 800, bottom: 700, width: 800, height: 700, toJSON: () => ({}) } as DOMRect;
    }
    return originalRect.call(this);
  });
  try {
    render(<>
      <NoteGroupFrame id="left" label="Left note" tabs={<button>Left tab</button>}>
        <button>Left editor</button>
        <PageInfoDrawer docked inspector open onClose={closeLeft} title="Left inspector" closeLabel="Close left">
          <button>Left source</button>
        </PageInfoDrawer>
      </NoteGroupFrame>
      <NoteGroupFrame id="right" label="Right note" tabs={<button>Right tab</button>}>
        <button>Right editor</button>
        <PageInfoDrawer docked inspector open onClose={closeRight} title="Right inspector" closeLabel="Close right">
          <button>Right source</button>
        </PageInfoDrawer>
      </NoteGroupFrame>
    </>);
    const leftGroup = document.querySelector<HTMLElement>('[data-note-group-id="left"]')!;
    const rightGroup = document.querySelector<HTMLElement>('[data-note-group-id="right"]')!;
    const leftInspector = within(leftGroup).getByRole("complementary", { name: "Left inspector" });
    const rightInspector = within(rightGroup).getByRole("complementary", { name: "Right inspector" });
    expect(leftGroup.querySelector(".page-info-drawer-group-overlay-panel")).toBe(leftInspector);
    expect(rightGroup.querySelector(".page-info-drawer-group-overlay-panel")).toBe(rightInspector);
    expect(leftGroup).not.toContainElement(rightInspector);
    expect(rightGroup).not.toContainElement(leftInspector);
    expect(leftInspector).not.toHaveAttribute("aria-modal");
    expect(rightInspector).not.toHaveAttribute("aria-modal");
    expect(document.activeElement).toBe(document.body);

    const rightEditor = within(rightGroup).getByRole("button", { name: "Right editor" });
    rightEditor.focus();
    fireEvent.keyDown(rightEditor, { key: "Escape" });
    expect(closeRight).toHaveBeenCalledOnce();
    expect(closeLeft).not.toHaveBeenCalled();
    expect(rightEditor).toHaveFocus();

    fireEvent.keyDown(within(leftInspector).getByRole("button", { name: "Left source" }), { key: "Escape" });
    expect(closeLeft).toHaveBeenCalledOnce();
    expect(closeRight).toHaveBeenCalledOnce();
  } finally {
    vi.restoreAllMocks();
  }
});

it("overlays a narrow inspector against only its own group edge", () => {
  const originalRect = HTMLElement.prototype.getBoundingClientRect;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains("note-group-frame")) {
      return { x: 0, y: 0, top: 0, left: 0, right: 600, bottom: 700, width: 600, height: 700, toJSON: () => ({}) } as DOMRect;
    }
    return originalRect.call(this);
  });
  try {
    render(<NoteGroupFrame id="narrow" label="Narrow note" tabs={<button>Note tab</button>}>
      <PageInfoDrawer docked inspector open onClose={vi.fn()} title="Narrow inspector" closeLabel="Close">
        <button>Source</button>
      </PageInfoDrawer>
    </NoteGroupFrame>);
    const group = document.querySelector<HTMLElement>(".note-group-frame")!;
    const overlay = within(group).getByRole("complementary", { name: "Narrow inspector" });
    expect(overlay).toHaveClass("page-info-drawer-group-overlay-panel");
    expect(overlay).not.toHaveAttribute("aria-modal");
    expect(group).toContainElement(overlay.closest(".page-info-drawer-group-overlay"));
    expect(overlay).toHaveStyle({ width: "min(320px, max(0px, calc(100% - max(204px, min(320px, 50%)))))" });
  } finally {
    vi.restoreAllMocks();
  }
});

it("expands an inspector inside its note group and preserves the map subtree", async () => {
  const originalRect = HTMLElement.prototype.getBoundingClientRect;
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains("note-group-frame")) {
      return { x: 0, y: 0, top: 0, left: 0, right: 800, bottom: 700, width: 800, height: 700, toJSON: () => ({}) } as DOMRect;
    }
    return originalRect.call(this);
  });
  try {
    const user = userEvent.setup();
    render(<GroupExpandableInspectorHarness />);
    const group = document.querySelector<HTMLElement>(".note-group-frame")!;
    const input = within(group).getByRole("textbox", { name: "Group map label" });
    await user.clear(input);
    await user.type(input, "preserved draft");
    await user.click(within(group).getByRole("button", { name: "Expand group" }));
    const expanded = within(group).getByRole("complementary", { name: "Group inspector" });
    expect(group).toContainElement(expanded.closest(".page-info-drawer-expanded-workspace"));
    expect(screen.getByTestId("external-expanded-host")).not.toContainElement(expanded);
    expect(within(group).getByRole("textbox", { name: "Group map label" })).toBe(input);
    expect(input).toHaveValue("preserved draft");
  } finally {
    vi.restoreAllMocks();
  }
});
