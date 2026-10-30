// SPDX-License-Identifier: AGPL-3.0-only
import { StrictMode, createRef } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { SearchQuery, setSearchQuery, replaceAll } from "@codemirror/search";
import { requestFocusedMarkdownLink } from "../../../lib/focusedEditorCommand";
import { EditorView } from "@codemirror/view";
import { undo, undoDepth } from "@codemirror/commands";
import { CodeMirrorMarkdownEditor } from "./CodeMirrorMarkdownEditor";
import type {
  MarkdownEditorHandle,
  MarkdownEditorProps,
  MarkdownEditorStatus,
} from "./MarkdownEditor";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  pasteText,
  pressKey,
  replaceDocument,
  selectRange,
} from "./editorTestUtils";

beforeAll(() => {
  installCodeMirrorDomPolyfills();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const baseProps = (overrides: Partial<MarkdownEditorProps> = {}): MarkdownEditorProps => ({
  initialDocument: "alpha\nbeta",
  sessionId: "page-1:1",
  disabled: false,
  ariaLabel: "Page source",
  onDocumentChange: vi.fn(),
  onSave: vi.fn(),
  onCancel: vi.fn(),
  onFallback: vi.fn(),
  ...overrides,
});

function lastStatus(callback: ReturnType<typeof vi.fn>): MarkdownEditorStatus {
  const calls = callback.mock.calls as Array<[MarkdownEditorStatus]>;
  const status = calls[calls.length - 1]?.[0];
  if (!status) throw new Error("Expected an editor status update");
  return status;
}

describe("CodeMirrorMarkdownEditor", () => {
  it("restores directional selection and clamps it when a new session opens shorter text", () => {
    const props = baseProps({ initialSelection: { anchor: 8, head: 2 } });
    const { rerender } = render(<CodeMirrorMarkdownEditor {...props} />);
    const view = editorViewFromTextbox(screen.getByRole("textbox"));
    expect(view.state.selection.main).toMatchObject({ anchor: 8, head: 2 });
    expect(props.onDocumentChange).not.toHaveBeenCalled();

    act(() => selectRange(view, 4, 1));
    rerender(<CodeMirrorMarkdownEditor {...props} initialSelection={{ anchor: 0, head: 0 }} />);
    expect(view.state.selection.main).toMatchObject({ anchor: 4, head: 1 });
    rerender(<CodeMirrorMarkdownEditor {...props} sessionId="page-1:2" initialDocument="xy"
      initialSelection={{ anchor: 8, head: -2 }} />);
    expect(editorViewFromTextbox(screen.getByRole("textbox")).state.selection.main)
      .toMatchObject({ anchor: 2, head: 0 });
    expect(props.onDocumentChange).not.toHaveBeenCalled();
  });

  it("reports selection-only changes through the latest callback without writing text", () => {
    const firstSelection = vi.fn();
    const latestSelection = vi.fn();
    const props = baseProps({ onSelectionChange: firstSelection });
    const { rerender } = render(<CodeMirrorMarkdownEditor {...props} />);
    const view = editorViewFromTextbox(screen.getByRole("textbox"));
    rerender(<CodeMirrorMarkdownEditor {...props} onSelectionChange={latestSelection} />);
    act(() => selectRange(view, 7, 1));
    expect(firstSelection).not.toHaveBeenCalled();
    expect(latestSelection).toHaveBeenLastCalledWith({ anchor: 7, head: 1 });
    expect(props.onDocumentChange).not.toHaveBeenCalled();
    expect(props.onSave).not.toHaveBeenCalled();
  });

  it("reports the selection mapped into a changed document", () => {
    const onSelectionChange = vi.fn();
    const props = baseProps({ initialSelection: { anchor: 10, head: 10 }, onSelectionChange });
    render(<CodeMirrorMarkdownEditor {...props} />);
    const view = editorViewFromTextbox(screen.getByRole("textbox"));
    act(() => replaceDocument(view, "xy"));
    expect(onSelectionChange).toHaveBeenLastCalledWith({ anchor: 2, head: 2 });
    expect(props.onDocumentChange).toHaveBeenCalledExactlyOnceWith("xy");
  });

  it("keeps slash navigation and dismissal in the same writing session without canceling", () => {
    const props = baseProps({ initialDocument: "" });
    render(<CodeMirrorMarkdownEditor {...props} />);
    const textbox = screen.getByRole("textbox", { name: "Page source" });
    const view = editorViewFromTextbox(textbox);
    act(() => {
      view.focus();
      view.dispatch({ changes: { from: 0, insert: "/" }, selection: { anchor: 1 }, userEvent: "input.type" });
    });
    expect(textbox).toHaveAttribute("aria-activedescendant", expect.stringContaining("heading1"));
    act(() => pressKey(textbox, "ArrowDown"));
    expect(textbox).toHaveAttribute("aria-activedescendant", expect.stringContaining("heading2"));
    act(() => pressKey(textbox, "Escape"));
    expect(textbox).not.toHaveAttribute("aria-activedescendant");
    expect(view.state.doc.toString()).toBe("/");
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.onDocumentChange).toHaveBeenCalledTimes(1);
    expect(textbox).toHaveFocus();
  });

  it("closes slash before IME input and does not reopen on composition commit", async () => {
    const props = baseProps({ initialDocument: "" });
    render(<CodeMirrorMarkdownEditor {...props} />);
    const textbox = screen.getByRole("textbox", { name: "Page source" });
    const view = editorViewFromTextbox(textbox);
    act(() => {
      view.focus();
      view.dispatch({ changes: { from: 0, insert: "/" }, selection: { anchor: 1 }, userEvent: "input.type" });
      fireEvent.compositionStart(textbox);
    });
    expect(textbox).not.toHaveAttribute("aria-controls");
    act(() => {
      pressKey(textbox, "Enter", { isComposing: true, keyCode: 229 });
      pressKey(textbox, "Escape", { isComposing: true, keyCode: 229 });
      view.dispatch({ changes: { from: 1, insert: "中" }, selection: { anchor: 2 }, userEvent: "input.type.compose" });
    });
    await act(async () => { fireEvent.compositionEnd(textbox); await Promise.resolve(); });
    expect(view.state.doc.toString()).toBe("/中");
    expect(textbox).not.toHaveAttribute("aria-controls");
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.onSave).not.toHaveBeenCalled();
  });

  it("accepts slash as exact source once and keeps readonly changes in the same editor", () => {
    const props = baseProps({ initialDocument: "" });
    const { rerender } = render(<CodeMirrorMarkdownEditor {...props} />);
    const textbox = screen.getByRole("textbox", { name: "Page source" });
    const view = editorViewFromTextbox(textbox);
    act(() => {
      view.focus();
      view.dispatch({ changes: { from: 0, insert: "/" }, selection: { anchor: 1 }, userEvent: "input.type" });
    });
    vi.mocked(props.onDocumentChange).mockClear();
    act(() => pressKey(textbox, "Enter"));
    expect(props.onDocumentChange).toHaveBeenCalledExactlyOnceWith("# ");
    act(() => { replaceDocument(view, ""); selectRange(view, 0); view.dispatch({ changes: { from: 0, insert: "/" }, selection: { anchor: 1 }, userEvent: "input.type" }); });
    rerender(<CodeMirrorMarkdownEditor {...props} disabled />);
    expect(editorViewFromTextbox(textbox)).toBe(view);
    expect(textbox).not.toHaveAttribute("aria-controls");
    act(() => pressKey(textbox, "Enter"));
    expect(view.state.doc.toString()).toBe("/");
  });

  it("constructs and destroys exactly one live view under StrictMode replay without emitting callbacks", () => {
    const destroy = vi.spyOn(EditorView.prototype, "destroy");
    const props = baseProps();
    const { container, unmount } = render(
      <StrictMode>
        <CodeMirrorMarkdownEditor {...props} />
      </StrictMode>,
    );

    expect(container.querySelectorAll(".cm-editor")).toHaveLength(1);
    expect(container.firstElementChild).toHaveStyle({ width: "100%", minHeight: "300px" });
    expect(destroy).toHaveBeenCalledTimes(1);
    expect(props.onDocumentChange).not.toHaveBeenCalled();
    expect(props.onSave).not.toHaveBeenCalled();
    expect(props.onCancel).not.toHaveBeenCalled();
    expect(props.onFallback).not.toHaveBeenCalled();

    unmount();
    expect(destroy).toHaveBeenCalledTimes(2);
    expect(container.querySelectorAll(".cm-editor")).toHaveLength(0);
  });

  it("grows with the Page shell instead of owning a nested vertical scroller", () => {
    const { container } = render(
      <CodeMirrorMarkdownEditor
        {...baseProps({
          initialDocument: Array.from(
            { length: 120 },
            (_, index) => `Long editor line ${index + 1}`,
          ).join("\n"),
        })}
      />,
    );
    const scroller = container.querySelector<HTMLElement>(".cm-scroller");

    expect(scroller).not.toBeNull();
    expect(scroller).toHaveStyle({
      height: "auto",
      overflow: "visible",
    });
  });

  it("keeps document, selection, and undo history when same-session props change", () => {
    const first = baseProps();
    const { rerender } = render(<CodeMirrorMarkdownEditor {...first} />);
    const textbox = screen.getByRole("textbox", { name: "Page source" });
    const view = editorViewFromTextbox(textbox);

    act(() => {
      replaceDocument(view, "draft");
      selectRange(view, 2);
    });

    rerender(
      <CodeMirrorMarkdownEditor
        {...baseProps({
          initialDocument: "remote refetch must be ignored",
          sessionId: first.sessionId,
        })}
      />,
    );

    expect(view.state.doc.toString()).toBe("draft");
    expect(view.state.selection.main.anchor).toBe(2);
    act(() => {
      expect(undo(view)).toBe(true);
    });
    expect(view.state.doc.toString()).toBe("alpha\nbeta");
  });

  it("keeps contextual presentation and canonical source across same-session rerenders", () => {
    const onDocumentChange = vi.fn();
    const onStatusChange = vi.fn();
    const props = baseProps({
      initialDocument: "# Heading\n\n**bold**",
      onDocumentChange,
      onStatusChange,
    });
    const { rerender } = render(<CodeMirrorMarkdownEditor {...props} />);
    const view = editorViewFromTextbox(screen.getByRole("textbox"));

    act(() => {
      replaceDocument(view, "# Draft\n\n**bold**");
      selectRange(view, view.state.doc.length);
    });
    const selection = view.state.selection;
    const historyDepth = undoDepth(view.state);
    onDocumentChange.mockClear();

    rerender(
      <CodeMirrorMarkdownEditor
        {...props}
        initialDocument="remote refetch must be ignored"
      />,
    );

    const sameView = editorViewFromTextbox(screen.getByRole("textbox"));
    expect(sameView).toBe(view);
    expect(view.dom).toHaveAttribute("data-editor-presentation-mode", "writing");
    expect(view.state.doc.toString()).toBe("# Draft\n\n**bold**");
    expect(view.state.selection.eq(selection)).toBe(true);
    expect(undoDepth(view.state)).toBe(historyDepth);
    expect(onDocumentChange).not.toHaveBeenCalled();
    expect(lastStatus(onStatusChange)).toMatchObject({
      sessionId: props.sessionId,
      engine: "codemirror",
      ready: true,
    });
  });

  it("runs toolbar commands through the live handle and publishes history status", () => {
    const ref = createRef<MarkdownEditorHandle>();
    const onStatusChange = vi.fn();
    render(
      <CodeMirrorMarkdownEditor
        ref={ref}
        {...baseProps({
          initialDocument: "alpha",
          onStatusChange,
        })}
      />,
    );
    const view = editorViewFromTextbox(screen.getByRole("textbox"));
    act(() => selectRange(view, 0, view.state.doc.length));

    let formatted = false;
    act(() => {
      formatted = ref.current?.runCommand("bold") ?? false;
    });

    expect(formatted).toBe(true);
    expect(view.state.doc.toString()).toBe("**alpha**");
    expect(lastStatus(onStatusChange)).toMatchObject({ canUndo: true, canRedo: false });

    let undone = false;
    act(() => {
      undone = ref.current?.runCommand("undo") ?? false;
    });

    expect(undone).toBe(true);
    expect(view.state.doc.toString()).toBe("alpha");
    expect(lastStatus(onStatusChange)).toMatchObject({ canUndo: false, canRedo: true });
  });

  it("saves and cancels from the live handle with fresh callbacks", () => {
    const ref = createRef<MarkdownEditorHandle>();
    const firstSave = vi.fn();
    const firstCancel = vi.fn();
    const secondSave = vi.fn();
    const secondCancel = vi.fn();
    const { rerender } = render(
      <CodeMirrorMarkdownEditor
        ref={ref}
        {...baseProps({ onSave: firstSave, onCancel: firstCancel })}
      />,
    );
    const view = editorViewFromTextbox(screen.getByRole("textbox"));
    act(() => replaceDocument(view, "live document"));

    rerender(
      <CodeMirrorMarkdownEditor
        ref={ref}
        {...baseProps({ onSave: secondSave, onCancel: secondCancel })}
      />,
    );
    act(() => {
      expect(ref.current?.requestSave()).toBe(true);
      expect(ref.current?.requestCancel()).toBe(true);
    });

    expect(firstSave).not.toHaveBeenCalled();
    expect(firstCancel).not.toHaveBeenCalled();
    expect(secondSave).toHaveBeenCalledWith("live document");
    expect(secondCancel).toHaveBeenCalledOnce();
  });

  it("reveals source while composition blocks every imperative action", async () => {
    const ref = createRef<MarkdownEditorHandle>();
    const onSave = vi.fn();
    const onCancel = vi.fn();
    const onStatusChange = vi.fn();
    const props = baseProps({
      initialDocument: "**bold**\naway",
      onSave,
      onCancel,
      onStatusChange,
    });
    render(<CodeMirrorMarkdownEditor ref={ref} {...props} />);
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    act(() => selectRange(view, view.state.doc.length));
    expect(view.contentDOM.querySelector(".cm-line")?.textContent).toBe("bold");

    act(() => fireEvent.compositionStart(textbox));

    expect(lastStatus(onStatusChange)).toMatchObject({
      compositionActive: true,
    });
    expect(view.contentDOM.querySelector(".cm-line")?.textContent).toBe("**bold**");
    expect(ref.current?.runCommand("italic")).toBe(false);
    expect(ref.current?.requestSave()).toBe(false);
    expect(ref.current?.requestCancel()).toBe(false);

    expect(view.dom).toHaveAttribute("data-editor-presentation-mode", "writing");

    act(() => fireEvent.compositionEnd(textbox));
    await act(async () => {
      await Promise.resolve();
    });

    expect(view.dom).toHaveAttribute("data-editor-presentation-mode", "writing");
    expect(lastStatus(onStatusChange)).toMatchObject({
      compositionActive: false,
    });
    expect(view.contentDOM.querySelector(".cm-line")?.textContent).toBe("bold");
    expect(onSave).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("blocks every imperative action after read-only reconfiguration", () => {
    const ref = createRef<MarkdownEditorHandle>();
    const onSave = vi.fn();
    const onCancel = vi.fn();
    const props = baseProps({ onSave, onCancel });
    const { rerender } = render(<CodeMirrorMarkdownEditor ref={ref} {...props} />);

    rerender(<CodeMirrorMarkdownEditor ref={ref} {...props} disabled />);

    expect(ref.current?.runCommand("bold")).toBe(false);
    expect(ref.current?.requestSave()).toBe(false);
    expect(ref.current?.requestCancel()).toBe(false);
    expect(onSave).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("reconstructs only when sessionId changes", () => {
    const destroy = vi.spyOn(EditorView.prototype, "destroy");
    const { rerender } = render(<CodeMirrorMarkdownEditor {...baseProps()} />);
    const firstView = editorViewFromTextbox(screen.getByRole("textbox"));

    rerender(
      <CodeMirrorMarkdownEditor
        {...baseProps({ initialDocument: "next", sessionId: "page-1:2" })}
      />,
    );

    const nextView = editorViewFromTextbox(screen.getByRole("textbox"));
    expect(nextView).not.toBe(firstView);
    expect(nextView.state.doc.toString()).toBe("next");
    expect(destroy).toHaveBeenCalledTimes(1);
  });

  it("uses fresh callbacks without reconstructing the view", () => {
    const firstChange = vi.fn();
    const secondChange = vi.fn();
    const firstSave = vi.fn();
    const secondSave = vi.fn();
    const { rerender } = render(
      <CodeMirrorMarkdownEditor
        {...baseProps({ onDocumentChange: firstChange, onSave: firstSave })}
      />,
    );
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);

    rerender(
      <CodeMirrorMarkdownEditor
        {...baseProps({ onDocumentChange: secondChange, onSave: secondSave })}
      />,
    );
    act(() => replaceDocument(view, "fresh CJK 中文 😀 e\u0301"));
    act(() => pressKey(textbox, "s", { ctrlKey: true }));

    expect(firstChange).not.toHaveBeenCalled();
    expect(secondChange).toHaveBeenLastCalledWith("fresh CJK 中文 😀 e\u0301");
    expect(firstSave).not.toHaveBeenCalled();
    expect(secondSave).toHaveBeenCalledWith("fresh CJK 中文 😀 e\u0301");
  });

  it("reconfigures disabled and accessibility attributes without reconstructing", () => {
    const { rerender } = render(<CodeMirrorMarkdownEditor {...baseProps()} />);
    const textbox = screen.getByRole("textbox", { name: "Page source" });
    const view = editorViewFromTextbox(textbox);

    rerender(
      <CodeMirrorMarkdownEditor
        {...baseProps({
          disabled: true,
          ariaLabel: "Readonly source",
          describedBy: "editor-help",
        })}
      />,
    );

    const reconfigured = screen.getByRole("textbox", { name: "Readonly source" });
    expect(editorViewFromTextbox(reconfigured)).toBe(view);
    expect(reconfigured).toHaveAttribute("aria-multiline", "true");
    expect(reconfigured).toHaveAttribute("aria-describedby", "editor-help");
    expect(reconfigured).toHaveAttribute("aria-disabled", "true");
    expect(reconfigured).toHaveAttribute("contenteditable", "false");
    expect(view.state.readOnly).toBe(true);

    rerender(<CodeMirrorMarkdownEditor {...baseProps({ ariaLabel: "Writable source" })} />);
    expect(screen.getByRole("textbox", { name: "Writable source" })).not.toHaveAttribute(
      "aria-describedby",
    );
    expect(view.state.readOnly).toBe(false);
  });

  it("focuses only through the imperative handle", () => {
    const ref = createRef<MarkdownEditorHandle>();
    render(<CodeMirrorMarkdownEditor ref={ref} {...baseProps()} />);
    const textbox = screen.getByRole("textbox");
    const focus = vi.spyOn(editorViewFromTextbox(textbox), "focus");

    act(() => ref.current?.focus());

    expect(focus).toHaveBeenCalledOnce();
  });

  it.each([
    ["Enter", { ctrlKey: true }],
    ["s", { ctrlKey: true }],
  ] as const)("%s saves the current synchronous document", (key, options) => {
    const onSave = vi.fn();
    render(<CodeMirrorMarkdownEditor {...baseProps({ onSave })} />);
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    act(() => replaceDocument(view, "current\nsource"));

    let event!: KeyboardEvent;
    act(() => {
      event = pressKey(textbox, key, options);
    });

    expect(onSave).toHaveBeenCalledWith("current\nsource");
    expect(event.defaultPrevented).toBe(true);
  });

  it("commits browser-order composition input once and immediately saves the final snapshot", async () => {
    const onDocumentChange = vi.fn();
    const onSave = vi.fn();
    const onCancel = vi.fn();
    render(
      <CodeMirrorMarkdownEditor
        {...baseProps({ onDocumentChange, onSave, onCancel })}
      />,
    );
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    act(() => {
      view.focus();
      selectRange(view, view.state.doc.length);
    });

    act(() => {
      fireEvent.compositionStart(textbox, { data: "中" });
      fireEvent.compositionUpdate(textbox, { data: "中" });

      textbox.dispatchEvent(
        new InputEvent("beforeinput", {
          bubbles: true,
          cancelable: true,
          data: "中",
          inputType: "insertCompositionText",
          isComposing: true,
        }),
      );

      const finalLine = textbox.querySelectorAll(".cm-line").item(1);
      const composedText = document.createTextNode("中");
      finalLine.append(composedText);
      const range = document.createRange();
      range.setStartAfter(composedText);
      range.collapse(true);
      const selection = window.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);

      textbox.dispatchEvent(
        new InputEvent("input", {
          bubbles: true,
          data: "中",
          inputType: "insertCompositionText",
          isComposing: true,
        }),
      );
      fireEvent.compositionEnd(textbox, { data: "中" });
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(view.compositionStarted).toBe(false);
    expect(view.composing).toBe(false);

    let saveEvent!: KeyboardEvent;
    act(() => {
      saveEvent = pressKey(textbox, "s", { ctrlKey: true });
    });

    expect(view.state.doc.toString()).toBe("alpha\nbeta中");
    expect(onDocumentChange).toHaveBeenCalledOnce();
    expect(onDocumentChange).toHaveBeenCalledWith("alpha\nbeta中");
    expect(view.state.selection.main).toMatchObject({
      anchor: "alpha\nbeta中".length,
      head: "alpha\nbeta中".length,
    });
    expect(saveEvent.defaultPrevented).toBe(true);
    expect(onSave).toHaveBeenCalledOnce();
    expect(onSave).toHaveBeenCalledWith("alpha\nbeta中");
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("consumes custom shortcuts without action or mutation after disabled reconfiguration", () => {
    const onSave = vi.fn();
    const onCancel = vi.fn();
    const { rerender } = render(
      <CodeMirrorMarkdownEditor {...baseProps({ onSave, onCancel })} />,
    );
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    rerender(
      <CodeMirrorMarkdownEditor {...baseProps({ disabled: true, onSave, onCancel })} />,
    );
    const before = view.state.doc.toString();

    let saveEvent!: KeyboardEvent;
    let enterEvent!: KeyboardEvent;
    let escapeEvent!: KeyboardEvent;
    act(() => {
      saveEvent = pressKey(textbox, "s", { ctrlKey: true });
      enterEvent = pressKey(textbox, "Enter", { ctrlKey: true });
      escapeEvent = pressKey(textbox, "Escape");
    });

    expect(saveEvent.defaultPrevented).toBe(true);
    expect(enterEvent.defaultPrevented).toBe(true);
    expect(escapeEvent.defaultPrevented).toBe(true);
    expect(view.state.doc.toString()).toBe(before);
    expect(onSave).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("cancels on Escape outside composition and leaves Tab unhandled", () => {
    const onCancel = vi.fn();
    render(<CodeMirrorMarkdownEditor {...baseProps({ onCancel })} />);
    const textbox = screen.getByRole("textbox");

    let tab!: KeyboardEvent;
    act(() => {
      pressKey(textbox, "Escape");
      tab = pressKey(textbox, "Tab");
    });

    expect(onCancel).toHaveBeenCalledOnce();
    expect(tab.defaultPrevented).toBe(false);
  });

  it("pastes a URL literally over selected text", () => {
    render(<CodeMirrorMarkdownEditor {...baseProps({ initialDocument: "select me" })} />);
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    act(() => selectRange(view, 0, view.state.doc.length));

    act(() => pasteText(textbox, "https://wenlan.app/path"));

    expect(view.state.doc.toString()).toBe("https://wenlan.app/path");
  });

  it.each([
    ["bullet list", "- item", "- item\n- "],
    ["ordered list", "1. item", "1. item\n2. "],
    ["task list", "- [ ] item", "- [ ] item\n- [ ] "],
    ["blockquote", "> quote", "> quote\n> "],
  ] as const)("continues a Markdown %s on Enter", (_name, source, expected) => {
    render(<CodeMirrorMarkdownEditor {...baseProps({ initialDocument: source })} />);
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    act(() => selectRange(view, view.state.doc.length));

    act(() => pressKey(textbox, "Enter"));

    expect(view.state.doc.toString()).toBe(expected);
  });


  it("connects the stock Find/Replace panel and preserves package undo history", () => {
    const ref=createRef<MarkdownEditorHandle>();
    render(<CodeMirrorMarkdownEditor ref={ref} {...baseProps({initialDocument:"alpha beta alpha"})} />);
    const view=editorViewFromTextbox(screen.getByRole("textbox"));
    act(()=>{expect(ref.current?.runCommand("find")).toBe(true);});
    expect(screen.getByRole("textbox",{name:"Find"})).toBeInTheDocument();
    act(()=>{view.dispatch({effects:setSearchQuery.of(new SearchQuery({search:"alpha",replace:"中文"}))});replaceAll(view);});
    expect(view.state.doc.toString()).toBe("中文 beta 中文");
    act(()=>{undo(view);});
    expect(view.state.doc.toString()).toBe("alpha beta alpha");
  });
  it("closes Find before leaving when Escape is pressed back in the document", () => {
    const ref=createRef<MarkdownEditorHandle>();const props=baseProps();
    render(<CodeMirrorMarkdownEditor ref={ref} {...props} />);
    const textbox=screen.getByRole("textbox",{name:"Page source"});const view=editorViewFromTextbox(textbox);
    act(()=>{ref.current?.runCommand("find");view.focus();pressKey(textbox,"Escape");});
    expect(screen.queryByRole("textbox",{name:"Find"})).not.toBeInTheDocument();
    expect(props.onCancel).not.toHaveBeenCalled();
    act(()=>{pressKey(textbox,"Escape");});expect(props.onCancel).toHaveBeenCalledTimes(1);
  });
  it("routes a native primary shortcut into the focused editor and blocks it during composition", () => {
    render(<CodeMirrorMarkdownEditor {...baseProps({initialDocument:"alpha"})} />);
    const textbox=screen.getByRole("textbox");const view=editorViewFromTextbox(textbox);
    act(()=>{view.focus();selectRange(view,0,5);expect(requestFocusedMarkdownLink()).toBe(true);});
    expect(view.state.doc.toString()).toBe("[alpha]()");
    fireEvent.compositionStart(textbox);
    act(()=>{expect(requestFocusedMarkdownLink()).toBe(true);});
    expect(view.state.doc.toString()).toBe("[alpha]()");
  });
  it("removes an empty Markdown marker with markup-aware Backspace", () => {
    render(<CodeMirrorMarkdownEditor {...baseProps({ initialDocument: "- " })} />);
    const textbox = screen.getByRole("textbox");
    const view = editorViewFromTextbox(textbox);
    act(() => selectRange(view, view.state.doc.length));

    act(() => pressKey(textbox, "Backspace"));

    expect(view.state.doc.toString()).toBe("");
  });
});
