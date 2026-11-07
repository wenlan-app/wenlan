// SPDX-License-Identifier: AGPL-3.0-only
import { StrictMode, useCallback, useEffect, useState } from "react";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import PageInfoDrawer from "./PageInfoDrawer";
import { SharedWikiInspectorContext, WorkspacePaneHostContext } from "../navigation/WorkspacePaneHost";
import { RIGHT_SIDEBAR_PREFERENCE_KEY, rightSidebarPreferenceKey } from "../navigation/rightSidebarPreferences";
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

function RetainedInspectorHarness() {
  const [open, setOpen] = useState(false);
  const close = () => setOpen(false);
  return <>
    <button onClick={() => setOpen(true)} type="button">Open retained inspector</button>
    <PageInfoDrawer keepMounted open={open} onClose={close} title="Retained inspector" closeLabel="Close retained inspector">
      <input aria-label="Reader position" defaultValue="page 4" />
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

function SharedWikiInspectorHarness() {
  const [activeGroupId, setActiveGroupId] = useState("primary");
  const [readingElement, setReadingElement] = useState<HTMLDivElement | null>(null);
  const [toolsElement, setToolsElement] = useState<HTMLDivElement | null>(null);
  const [mounts, setMounts] = useState({ primary: 0, secondary: 0 });
  const recordMount = useCallback((group: "primary" | "secondary") => setMounts(current => ({ ...current, [group]: current[group] + 1 })), []);
  return <SharedWikiInspectorContext.Provider value={{ activeGroupId, readingElement, toolsElement }}>
    <div className="wiki-workspace-reading" ref={setReadingElement}>
      <div className="note-group-document-tools" ref={setToolsElement}><button type="button">Shared tools</button></div>
      <NoteGroupFrame id="primary" label="Primary note" tabs={<button type="button">Primary tab</button>} onFocus={() => setActiveGroupId("primary")}>
        <input aria-label="Primary editor" />
        <PageInfoDrawer docked inspector keepMounted open onClose={sharedCloseCalls.primary} title="Primary inspector" closeLabel="Close primary">
          <MountCounter group="primary" onMount={recordMount} />
        </PageInfoDrawer>
      </NoteGroupFrame>
      <NoteGroupFrame id="secondary" label="Secondary note" tabs={<button type="button">Secondary tab</button>} onFocus={() => setActiveGroupId("secondary")}>
        <input aria-label="Secondary editor" />
        <PageInfoDrawer docked inspector keepMounted open onClose={sharedCloseCalls.secondary} title="Secondary inspector" closeLabel="Close secondary">
          <MountCounter group="secondary" onMount={recordMount} />
        </PageInfoDrawer>
      </NoteGroupFrame>
    </div>
    <output data-testid="mount-counts">{mounts.primary}:{mounts.secondary}</output>
  </SharedWikiInspectorContext.Provider>;
}

const sharedCloseCalls = { primary: vi.fn(), secondary: vi.fn() };

function MountCounter({ group, onMount }: { group: "primary" | "secondary"; onMount: (group: "primary" | "secondary") => void }) {
  useEffect(() => onMount(group), [group, onMount]);
  return <button type="button">{group} source</button>;
}

function SharedSplitWidthHarness({ isSplit }: { isSplit: boolean }) {
  const [readingElement, setReadingElement] = useState<HTMLDivElement | null>(null);
  return <SharedWikiInspectorContext.Provider value={{ activeGroupId: "primary", readingElement, toolsElement: null, isSplit }}>
    <div className="wiki-workspace-reading" ref={setReadingElement}>
      <NoteGroupFrame id="primary" label="Primary note" tabs={<button>Primary tab</button>}>
        <PageInfoDrawer defaultWidth={560} docked inspector open onClose={() => {}} title="PDF inspector" closeLabel="Close PDF">
          <button>PDF page</button>
        </PageInfoDrawer>
      </NoteGroupFrame>
    </div>
  </SharedWikiInspectorContext.Provider>;
}

function SharedNarrowSplitHarness() {
  const [activeGroupId, setActiveGroupId] = useState("primary");
  const [readingElement, setReadingElement] = useState<HTMLDivElement | null>(null);
  return <SharedWikiInspectorContext.Provider value={{ activeGroupId, readingElement, toolsElement: null, isSplit: true }}>
    <div className="wiki-workspace-reading" ref={setReadingElement}>
      <NoteGroupFrame id="primary" label="Primary note" tabs={<button>Primary tab</button>} onFocus={() => setActiveGroupId("primary")}>
        <input aria-label="Primary editor" />
        <PageInfoDrawer docked inspector keepMounted open onClose={() => {}} title="Primary inspector" closeLabel="Close primary">
          <button>Primary PDF controls</button>
        </PageInfoDrawer>
      </NoteGroupFrame>
      <NoteGroupFrame id="secondary" label="Secondary note" tabs={<button>Secondary tab</button>} onFocus={() => setActiveGroupId("secondary")}>
        <input aria-label="Secondary editor" />
        <PageInfoDrawer docked inspector keepMounted open onClose={() => {}} title="Secondary inspector" closeLabel="Close secondary">
          <button>Secondary PDF controls</button>
        </PageInfoDrawer>
      </NoteGroupFrame>
    </div>
  </SharedWikiInspectorContext.Provider>;
}

describe("PageInfoDrawer", () => {
  it("shares one inspector across Wiki groups without remounting the inactive reader", () => {
    sharedCloseCalls.primary.mockClear();
    sharedCloseCalls.secondary.mockClear();
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("wiki-workspace-reading")) {
        return { x: 0, y: 0, top: 0, left: 0, right: 800, bottom: 700, width: 800, height: 700, toJSON: () => ({}) } as DOMRect;
      }
      return originalRect.call(this);
    });
    try {
    render(<SharedWikiInspectorHarness />);
    const reading = document.querySelector<HTMLElement>(".wiki-workspace-reading")!;
    const primaryEditor = screen.getByRole("textbox", { name: "Primary editor" });
    const secondaryEditor = screen.getByRole("textbox", { name: "Secondary editor" });
    expect(within(reading).getByRole("complementary", { name: "Primary inspector" })).toBeInTheDocument();
    expect(within(reading).queryByRole("complementary", { name: "Secondary inspector" })).not.toBeInTheDocument();
    expect(reading).toHaveClass("wiki-shared-inspector-open");
    expect(screen.getByTestId("mount-counts")).toHaveTextContent("1:1");

    act(() => secondaryEditor.focus());
    expect(within(reading).getByRole("complementary", { name: "Secondary inspector" })).toBeInTheDocument();
    expect(within(reading).queryByRole("complementary", { name: "Primary inspector" })).not.toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Primary editor" })).toBe(primaryEditor);
    expect(screen.getByRole("textbox", { name: "Secondary editor" })).toBe(secondaryEditor);
    expect(screen.getByTestId("mount-counts")).toHaveTextContent("1:1");
    expect(secondaryEditor).toHaveFocus();

    fireEvent.keyDown(primaryEditor, { key: "Escape" });
    expect(sharedCloseCalls.primary).not.toHaveBeenCalled();
    expect(sharedCloseCalls.secondary).not.toHaveBeenCalled();
    fireEvent.keyDown(within(reading).getByRole("button", { name: "secondary source" }), { key: "Escape" });
    expect(sharedCloseCalls.secondary).toHaveBeenCalledOnce();
    fireEvent.keyDown(screen.getByRole("button", { name: "Shared tools" }), { key: "Escape" });
    expect(sharedCloseCalls.secondary).toHaveBeenCalledTimes(2);
    expect(sharedCloseCalls.primary).not.toHaveBeenCalled();
    } finally {
      vi.restoreAllMocks();
    }
  });

  it("omits the inspector header when its controls live in the shared toolbar", () => {
    render(<PageInfoDrawer headerless headerContent={<button>Hidden toolbar</button>} open onClose={() => {}} title="Unified inspector" closeLabel="Close">
      <button>Reader action</button>
    </PageInfoDrawer>);
    const panel = screen.getByRole("dialog", { name: "Unified inspector" });
    expect(within(panel).getByRole("heading", { name: "Unified inspector" })).toHaveClass("sr-only");
    expect(within(panel).queryByRole("button", { name: "Hidden toolbar" })).not.toBeInTheDocument();
    expect(panel.querySelector(".page-info-drawer-header")).toBeNull();
    expect(within(panel).getByRole("button", { name: "Reader action" })).toBeInTheDocument();
  });

  it.each([
    { width: 800, isSplit: false, panelWidth: "320px", reservedWidth: "320px" },
    { width: 600, isSplit: true, panelWidth: "560px", reservedWidth: null },
    { width: 799, isSplit: true, panelWidth: "320px", reservedWidth: "320px" },
    { width: 800, isSplit: true, panelWidth: "320px", reservedWidth: "320px" },
    { width: 801, isSplit: true, panelWidth: "320px", reservedWidth: "320px" },
    { width: 1032, isSplit: true, panelWidth: "392px", reservedWidth: "392px" },
  ])("keeps split reading width continuous ($width px, split=$isSplit)", async ({ width, isSplit, panelWidth, reservedWidth }) => {
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("wiki-workspace-reading")) {
        return { x: 0, y: 0, top: 0, left: 0, right: width, bottom: 700, width, height: 700, toJSON: () => ({}) } as DOMRect;
      }
      return originalRect.call(this);
    });
    try {
      render(<SharedSplitWidthHarness isSplit={isSplit} />);
      const panel = await screen.findByRole("complementary", { name: "PDF inspector" });
      expect(panel).toHaveStyle({ width: panelWidth });
      const reading = document.querySelector<HTMLElement>(".wiki-workspace-reading")!;
      if (reservedWidth) expect(reading).toHaveStyle({ "--wiki-shared-inspector-width": reservedWidth });
      else expect(reading).not.toHaveClass("wiki-shared-inspector-open");
    } finally {
      vi.restoreAllMocks();
    }
  });

  it("overlays a usable narrow inspector without narrowing either Wiki or remounting them", async () => {
    const originalRect = HTMLElement.prototype.getBoundingClientRect;
    const preferenceKeys = [RIGHT_SIDEBAR_PREFERENCE_KEY, rightSidebarPreferenceKey("primary"), rightSidebarPreferenceKey("secondary")];
    const previousPreferences = preferenceKeys.map(key => localStorage.getItem(key));
    preferenceKeys.forEach(key => localStorage.removeItem(key));
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      if (this.classList.contains("wiki-workspace-reading")) {
        return { x: 0, y: 0, top: 0, left: 0, right: 375, bottom: 700, width: 375, height: 700, toJSON: () => ({}) } as DOMRect;
      }
      return originalRect.call(this);
    });
    try {
      render(<SharedNarrowSplitHarness />);
      const reading = document.querySelector<HTMLElement>(".wiki-workspace-reading")!;
      const primaryEditor = screen.getByRole("textbox", { name: "Primary editor" });
      const secondaryEditor = screen.getByRole("textbox", { name: "Secondary editor" });
      const primaryPanel = await screen.findByRole("complementary", { name: "Primary inspector" });
      expect(primaryPanel).toHaveStyle({ width: "320px" });
      expect(primaryPanel.closest(".page-info-drawer-group-overlay")).toHaveClass("page-info-drawer-shared-narrow-overlay");
      expect(reading).not.toHaveClass("wiki-shared-inspector-open");
      expect(reading.style.getPropertyValue("--wiki-shared-inspector-width")).toBe("");
      const resize = screen.getByRole("separator", { name: "Resize right sidebar" });
      expect(resize).toHaveAttribute("aria-valuemax", "375");
      expect(resize).toHaveAttribute("aria-valuenow", "320");
      fireEvent.keyDown(resize, { key: "ArrowLeft" });
      expect(primaryPanel).toHaveStyle({ width: "340px" });
      expect(resize).toHaveAttribute("aria-valuenow", "340");

      act(() => secondaryEditor.focus());
      expect(screen.getByRole("complementary", { name: "Secondary inspector" })).toHaveStyle({ width: "320px" });
      expect(screen.queryByRole("complementary", { name: "Primary inspector" })).not.toBeInTheDocument();
      expect(screen.getByRole("textbox", { name: "Primary editor" })).toBe(primaryEditor);
      expect(screen.getByRole("textbox", { name: "Secondary editor" })).toBe(secondaryEditor);
      expect(reading).not.toHaveClass("wiki-shared-inspector-open");
    } finally {
      vi.restoreAllMocks();
      preferenceKeys.forEach((key, index) => {
        const value = previousPreferences[index];
        if (value === null) localStorage.removeItem(key); else localStorage.setItem(key, value);
      });
    }
  });

  it("keeps a requested reader mounted while the closed pane is fully hidden", async () => {
    const user = userEvent.setup();
    render(<RetainedInspectorHarness />);
    const opener = screen.getByRole("button", { name: "Open retained inspector" });
    expect(screen.queryByRole("complementary", { name: "Retained inspector" })).toBeNull();
    const hiddenOverlay = document.querySelector<HTMLElement>(".page-info-drawer-overlay[hidden]");
    expect(hiddenOverlay).toBeTruthy();
    expect(hiddenOverlay).toHaveAttribute("inert");

    await user.click(opener);
    const position = screen.getByRole("textbox", { name: "Reader position" });
    const drawer = screen.getByRole("dialog", { name: "Retained inspector" });
    expect(position).toHaveValue("page 4");
    await user.click(within(drawer).getByRole("button", { name: "Close retained inspector" }));
    expect(screen.queryByRole("dialog", { name: "Retained inspector" })).toBeNull();
    expect(opener).toHaveFocus();

    await user.click(opener);
    expect(screen.getByRole("textbox", { name: "Reader position" })).toBe(position);
    expect(position).toHaveValue("page 4");
  });

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
    expect(overlay).toHaveStyle({ width: "min(320px, max(0px, calc(100% - max(230px, min(320px, 50%)))))" });
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
