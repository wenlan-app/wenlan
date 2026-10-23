// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { deleteCharBackward, deleteCharForward } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { forceParsing } from "@codemirror/language";
import { EditorSelection, EditorState, type RangeSet } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";
import { installCodeMirrorDomPolyfills } from "./editorTestUtils";
import { prepareMarkdownSource, serializeMarkdownSource } from "./markdownSourceContract";
import {
  setWritingCompositionActive,
  writingPresentation,
} from "./writingPresentation";
import { resetAttachmentsDirForTest } from "../../../lib/pageAttachments";

beforeAll(() => installCodeMirrorDomPolyfills());

const liveViews: EditorView[] = [];

afterEach(() => {
  for (const view of liveViews.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function makeView(
  doc: string,
  options: {
    selection?: EditorSelection;
    language?: boolean;
    focused?: boolean;
    onDocumentUpdate?: () => void;
  } = {},
): EditorView {
  const parent = document.createElement("div");
  document.body.append(parent);
  const extensions = [
    ...(options.language === false
      ? []
      : [
          markdown({
            base: markdownLanguage,
            addKeymap: false,
            pasteURLAsLink: false,
            completeHTMLTags: false,
          }),
        ]),
    writingPresentation(),
    ...(options.onDocumentUpdate
      ? [
          EditorView.updateListener.of((update) => {
            if (update.docChanged) options.onDocumentUpdate?.();
          }),
        ]
      : []),
  ];
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: options.selection,
      extensions,
    }),
  });
  if (options.focused !== false) view.focus();
  liveViews.push(view);
  return view;
}

function settleFocusChange(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 15));
}

interface PresentationRange {
  from: number;
  to: number;
}

function rangesFromSet(set: RangeSet<Decoration>, docLength: number): PresentationRange[] {
  const ranges: PresentationRange[] = [];
  set.between(0, docLength, (from, to, value) => {
    if (
      value instanceof Decoration &&
      (value.spec as { writingConceal?: boolean }).writingConceal
    ) {
      ranges.push({ from, to });
    }
  });
  return ranges.sort((left, right) => left.from - right.from || left.to - right.to);
}

function concealedDecorationRanges(view: EditorView): PresentationRange[] {
  const ranges: PresentationRange[] = [];
  for (const source of view.state.facet(EditorView.decorations)) {
    const set = typeof source === "function" ? source(view) : source;
    ranges.push(...rangesFromSet(set, view.state.doc.length));
  }
  return ranges.sort((left, right) => left.from - right.from || left.to - right.to);
}

function concealedAtomicRanges(view: EditorView): PresentationRange[] {
  const ranges: PresentationRange[] = [];
  for (const source of view.state.facet(EditorView.atomicRanges)) {
    ranges.push(...rangesFromSet(source(view), view.state.doc.length));
  }
  return ranges.sort((left, right) => left.from - right.from || left.to - right.to);
}

function rangeFor(source: string, text: string, offset = 0): PresentationRange {
  const from = source.indexOf(text, offset);
  if (from < 0) throw new Error(`Expected ${JSON.stringify(text)} in source`);
  return { from, to: from + text.length };
}

describe("writingPresentation", () => {
  it("renders common parsed Markdown as live preview and keeps unknown syntax literal", () => {
    const source = [
      "# Heading #   ",
      "",
      "**bold** and *italic* and `code`",
      "- bullet",
      "1. ordered",
      "- [ ] task",
      "",
      "> quote",
      "",
      "[link](target)",
      "",
      "---",
      "",
      "~~strike~~",
      "",
      "[[Wiki]] ![[embed]]",
      "::: malformed ** marker",
      "away",
    ].join("\n");
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    const bold = source.indexOf("**bold**");
    const italic = source.indexOf("*italic*");
    const code = source.indexOf("`code`");
    const bullet = source.indexOf("- bullet");
    const ordered = source.indexOf("1. ordered");
    const task = source.indexOf("- [ ] task");
    const quote = source.indexOf("> quote");
    const link = source.indexOf("[link](target)");
    const horizontalRule = source.indexOf("---");
    const strike = source.indexOf("~~strike~~");
    const expected = [
      { from: 0, to: 2 },
      rangeFor(source, "#", source.indexOf("Heading") + "Heading".length),
      { from: bold, to: bold + 2 },
      { from: bold + 6, to: bold + 8 },
      { from: italic, to: italic + 1 },
      { from: italic + 7, to: italic + 8 },
      { from: code, to: code + 1 },
      { from: code + 5, to: code + 6 },
      { from: bullet, to: bullet + 2 },
      { from: ordered, to: ordered + 3 },
      { from: task, to: task + 2 },
      { from: task + 2, to: task + 5 },
      { from: quote, to: quote + 2 },
      { from: link, to: link + 1 },
      { from: link + 5, to: link + "[link](target)".length },
      { from: horizontalRule, to: horizontalRule + 3 },
      { from: strike, to: strike + 2 },
      { from: strike + 8, to: strike + 10 },
    ];

    expect(view.dom).toHaveAttribute("data-editor-presentation-mode", "writing");
    expect(concealedDecorationRanges(view)).toEqual(expected);
    expect(concealedAtomicRanges(view)).toEqual(expected);
    expect(view.state.doc.toString()).toBe(source);
    expect(view.contentDOM.textContent).toContain("Heading    ");
    expect(view.contentDOM.textContent).toContain("bold and italic and code");
    expect(view.contentDOM.textContent).toContain("[[Wiki]] ![[embed]]");
    expect(view.contentDOM.textContent).toContain("::: malformed ** marker");
    expect(view.contentDOM.textContent).not.toContain("[link](target)");
    expect(view.contentDOM.textContent).not.toContain("> quote");
    expect(view.contentDOM.querySelector('[data-writing-list-marker="bullet"]'))
      .toHaveTextContent("•");
    expect(view.contentDOM.querySelector('[data-writing-list-marker="ordered"]'))
      .toHaveTextContent("1.");
    expect(view.contentDOM.querySelector(".cm-writing-task-checkbox"))
      .toHaveAttribute("type", "checkbox");
    expect(view.contentDOM.querySelector(".cm-writing-blockquote")).not.toBeNull();
    expect(view.contentDOM.querySelector(".cm-writing-link")).toHaveTextContent("link");
    expect(view.contentDOM.querySelector(".cm-writing-horizontal-rule")).not.toBeNull();
    expect(view.contentDOM.querySelector(".cm-writing-strikethrough"))
      .toHaveTextContent("strike");
  });

  it("marks only browser-bound links, labelled or bare, as external", () => {
    const source = [
      "See [docs](https://wenlan.app/docs) and [[Wiki]].",
      "",
      "Bare https://github.com/wenlan-app/wenlan/ here.",
      "",
      "[local](#concept:p-1)",
    ].join("\n");
    const view = makeView(source, { selection: EditorSelection.single(source.length) });

    const external = [...view.contentDOM.querySelectorAll(".cm-writing-link-external")];
    expect(external.map((node) => node.textContent)).toEqual([
      "docs",
      "github.com/wenlan-app/wenlan",
    ]);
    expect(view.contentDOM.textContent).not.toContain("https://");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("reveals every construct on the active line and restores concealment without a document update", async () => {
    const source = "# Heading\n\n**bold** and *italic*\naway";
    const onDocumentUpdate = vi.fn();
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
      onDocumentUpdate,
    });
    const initial = concealedDecorationRanges(view);
    const inlineLine = source.indexOf("bold") + 2;
    await settleFocusChange();

    view.dispatch({ selection: EditorSelection.single(inlineLine) });

    expect(concealedDecorationRanges(view)).toEqual([{ from: 0, to: 2 }]);
    expect(view.contentDOM.textContent).toContain("**bold** and *italic*");
    expect(view.state.doc.toString()).toBe(source);
    expect(onDocumentUpdate).not.toHaveBeenCalled();

    view.dispatch({ selection: EditorSelection.single(source.length) });

    expect(concealedDecorationRanges(view)).toEqual(initial);
    expect(view.state.doc.toString()).toBe(source);
    expect(onDocumentUpdate).not.toHaveBeenCalled();
  });

  it("reveals complete constructs intersected by a selection", async () => {
    const source = "# Heading\n\n**bold** and *italic*\naway";
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });
    await settleFocusChange();

    view.dispatch({
      selection: EditorSelection.single(
        source.indexOf("Heading"),
        source.indexOf("italic") + 3,
      ),
    });

    expect(concealedDecorationRanges(view)).toEqual([]);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("reveals selected source only while the editor content is focused", async () => {
    const source = "# Heading\n\n> quoted\n\n[link](https://example.com)\naway";
    const selection = EditorSelection.single(0, source.indexOf("\naway"));
    const onDocumentUpdate = vi.fn();
    const view = makeView(source, { selection, focused: false, onDocumentUpdate });
    const originalSelection = view.state.selection;
    const originalRanges = concealedDecorationRanges(view);

    expect(originalRanges).not.toEqual([]);

    view.focus();
    await settleFocusChange();

    expect(concealedDecorationRanges(view)).toEqual([]);
    expect(view.state.doc.toString()).toBe(source);
    expect(view.state.selection.eq(originalSelection)).toBe(true);
    expect(onDocumentUpdate).not.toHaveBeenCalled();

    view.contentDOM.blur();
    await settleFocusChange();

    expect(concealedDecorationRanges(view)).toEqual(originalRanges);
    expect(view.state.doc.toString()).toBe(source);
    expect(view.state.selection.eq(originalSelection)).toBe(true);
    expect(onDocumentUpdate).not.toHaveBeenCalled();
  });

  it("reveals the active quoted line without exposing neighboring Markdown", async () => {
    const source = [
      "> first",
      "> second",
      "",
      "- bullet",
      "",
      "[link](target)",
      "away",
    ].join("\n");
    const secondQuote = source.indexOf("> second");
    const bullet = source.indexOf("- bullet");
    const link = source.indexOf("[link](target)");
    const view = makeView(source, {
      selection: EditorSelection.single(secondQuote + 4),
    });
    await settleFocusChange();

    expect(concealedDecorationRanges(view)).toEqual([
      { from: 0, to: 2 },
      { from: bullet, to: bullet + 2 },
      { from: link, to: link + 1 },
      { from: link + 5, to: link + "[link](target)".length },
    ]);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("presents every line of a multi-line blockquote", () => {
    const source = "> first\n> second\n\naway";
    const secondQuote = source.indexOf("> second");
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    expect(concealedDecorationRanges(view)).toEqual([
      { from: 0, to: 2 },
      { from: secondQuote, to: secondQuote + 2 },
    ]);
    expect(view.contentDOM.querySelectorAll(".cm-writing-blockquote")).toHaveLength(2);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("reveals all source during composition and restores it without changing document or selection", () => {
    const source = "**bold**\naway";
    const view = makeView(source, { selection: EditorSelection.single(source.length) });
    const selection = view.state.selection;
    const beforeRanges = concealedDecorationRanges(view);

    view.dispatch({ effects: setWritingCompositionActive.of(true) });

    expect(concealedDecorationRanges(view)).toEqual([]);
    expect(view.contentDOM.textContent).toContain("**bold**");
    expect(view.state.doc.toString()).toBe(source);
    expect(view.state.selection.eq(selection)).toBe(true);

    view.dispatch({ effects: setWritingCompositionActive.of(false) });

    expect(concealedDecorationRanges(view)).toEqual(beforeRanges);
    expect(view.state.doc.toString()).toBe(source);
    expect(view.state.selection.eq(selection)).toBe(true);
  });

  it("toggles a rendered task checkbox through one source update and restores keyboard focus", () => {
    const source = "- [ ] verify exact source\naway";
    const onDocumentUpdate = vi.fn();
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
      onDocumentUpdate,
    });
    const selection = view.state.selection;
    const checkbox = view.contentDOM.querySelector<HTMLInputElement>(
      ".cm-writing-task-checkbox",
    );

    expect(checkbox).not.toBeNull();
    expect(checkbox).not.toBeChecked();
    expect(checkbox).toHaveProperty("tabIndex", 0);
    expect(checkbox).toHaveAccessibleName("verify exact source");
    checkbox?.focus();
    expect(checkbox).toHaveFocus();
    checkbox?.click();

    expect(view.state.doc.toString()).toBe("- [x] verify exact source\naway");
    expect(view.state.selection.eq(selection)).toBe(true);
    expect(onDocumentUpdate).toHaveBeenCalledTimes(1);
    expect(
      view.contentDOM.querySelector(".cm-writing-task-checkbox"),
    ).toHaveFocus();
  });

  it("uses readable task text for the rendered checkbox name", () => {
    const source =
      "- [ ] **verify** [exact source](https://wenlan.app) with `tests`\naway";
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    expect(view.contentDOM.querySelector(".cm-writing-task-checkbox"))
      .toHaveAccessibleName("verify exact source with tests");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("renders a safe CommonMark image without changing its source", () => {
    const source = "![Wenlan graph](https://wenlan.app/graph.png)\naway";
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });
    const image = view.contentDOM.querySelector<HTMLImageElement>(
      ".cm-writing-image",
    );

    expect(image).not.toBeNull();
    expect(image).toHaveAttribute("src", "https://wenlan.app/graph.png");
    expect(image).toHaveAttribute("alt", "Wenlan graph");
    expect(image).toHaveAttribute("loading", "lazy");
    expect(view.contentDOM.textContent).not.toContain("![Wenlan graph]");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("shows page images and videos from the pages folder once it is known", () => {
    resetAttachmentsDirForTest(null);
    const source = "![Home](_attachments/home-3f2a9c1b0d.png)\n\n![Film](_attachments/film-0a1b2c3d4e.mp4)\naway";
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });
    expect(view.contentDOM.querySelector(".cm-writing-image")).toBeNull();

    resetAttachmentsDirForTest("/Users/me/.wenlan/pages/_attachments");
    const image = view.contentDOM.querySelector<HTMLImageElement>("img.cm-writing-image");
    const video = view.contentDOM.querySelector<HTMLVideoElement>("video.cm-writing-video");
    expect(image).toHaveAttribute("src", "asset://localhost//Users/me/.wenlan/pages/_attachments/home-3f2a9c1b0d.png");
    expect(image).toHaveAttribute("alt", "Home");
    expect(video).toHaveAttribute("src", "asset://localhost//Users/me/.wenlan/pages/_attachments/film-0a1b2c3d4e.mp4#t=0.1");
    expect(video?.controls).toBe(true);
    expect(view.state.doc.toString()).toBe(source);
    resetAttachmentsDirForTest(null);
  });

  it("keeps an image shown while the caret touches it, so one key removes it whole", async () => {
    const image = "![Wenlan graph](https://wenlan.app/graph.png)";
    const source = `before\n${image}\nafter`;
    const { from, to } = rangeFor(source, image);
    const view = makeView(source, { selection: EditorSelection.single(to) });
    await settleFocusChange();

    expect(view.contentDOM.querySelector(".cm-writing-image")).not.toBeNull();
    expect(deleteCharBackward(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("before\n\nafter");

    view.dispatch({
      changes: { from, insert: image },
      selection: EditorSelection.single(from),
    });
    expect(view.contentDOM.querySelector(".cm-writing-image")).not.toBeNull();
    expect(deleteCharForward(view)).toBe(true);
    expect(view.state.doc.toString()).toBe("before\n\nafter");
  });

  it("keeps a clicked image in view above its editable source", async () => {
    const image = "![Wenlan graph](https://wenlan.app/graph.png)";
    const source = `${image}\naway`;
    const view = makeView(source, { selection: EditorSelection.single(source.length) });
    await settleFocusChange();

    const rendered = view.contentDOM.querySelector<HTMLImageElement>("img.cm-writing-image");
    expect(rendered).not.toBeNull();
    rendered!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, detail: 1 }));

    expect(view.state.selection.main.head).toBe(2);
    expect(view.contentDOM.querySelector("img.cm-writing-image-open")).toHaveAttribute("src", "https://wenlan.app/graph.png");
    expect(view.contentDOM.querySelector(".cm-writing-media-source")?.textContent).toBe(image);
    expect(concealedAtomicRanges(view)).toEqual([]);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("removes an image and the empty line under it with one Backspace", async () => {
    const image = "![Wenlan graph](https://wenlan.app/graph.png)";
    const source = `before\n${image}\n\nafter`;
    const view = makeView(source, { selection: EditorSelection.single(source.indexOf("\nafter")) });
    await settleFocusChange();

    expect(view.contentDOM.querySelector(".cm-writing-image")).not.toBeNull();
    view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace", keyCode: 8, bubbles: true, cancelable: true }));

    expect(view.state.doc.toString()).toBe("before\nafter");
    expect(view.state.selection.main.head).toBe("before".length);
  });

  it("keeps Backspace ordinary under text or an unrendered image", async () => {
    for (const above of ["plain words", "![local](./attachment.png)"]) {
      const source = `${above}\n\nafter`;
      const view = makeView(source, { selection: EditorSelection.single(above.length + 1) });
      await settleFocusChange();
      view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "Backspace", keyCode: 8, bubbles: true, cancelable: true }));
      expect(view.state.doc.toString()).toBe(source);
    }
  });

  it("keeps unsafe or unresolved image destinations as editable source", () => {
    const source = [
      "![local](./attachment.png)",
      "![unsafe](javascript:alert(1))",
      "away",
    ].join("\n");
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    expect(view.contentDOM.querySelector(".cm-writing-image")).toBeNull();
    expect(view.contentDOM.textContent).toContain("![local](./attachment.png)");
    expect(view.contentDOM.textContent).toContain("![unsafe](javascript:alert(1))");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("presents an Obsidian callout while preserving its Markdown", () => {
    const source = "> [!NOTE] Exact source\n> Body stays editable\n\naway";
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    expect(view.contentDOM.querySelectorAll(".cm-writing-callout")).toHaveLength(2);
    expect(view.contentDOM.querySelector(".cm-writing-callout-badge"))
      .toHaveTextContent("Note");
    expect(view.contentDOM.textContent).not.toContain("[!NOTE]");
    expect(view.contentDOM.textContent).not.toContain("> Body");
    expect(view.contentDOM.textContent).toContain("Body stays editable");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("reuses the horizontal-rule widget when unrelated selection state changes", () => {
    const source = "---\n\nfirst\nsecond";
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });
    const initialRule = view.contentDOM.querySelector(".cm-writing-horizontal-rule");

    view.dispatch({ selection: EditorSelection.single(source.indexOf("first")) });

    expect(initialRule).not.toBeNull();
    expect(view.contentDOM.querySelector(".cm-writing-horizontal-rule")).toBe(initialRule);
  });

  it("presents a parsed fenced code block without changing its source", () => {
    const source = "```ts\nconst value = 1;\n```\naway";
    const closingFence = source.indexOf("```", 3);
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    expect(concealedDecorationRanges(view)).toEqual([
      { from: 0, to: "```ts".length },
      { from: closingFence, to: closingFence + 3 },
    ]);
    expect(view.contentDOM.textContent).not.toContain("```ts");
    expect(view.contentDOM.querySelectorAll(".cm-writing-fenced-code")).toHaveLength(3);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("reveals fenced-code source when the caret is on its closing line", async () => {
    const source = "```ts\nconst value = 1;\n```\naway";
    const closingFence = source.lastIndexOf("```");
    const view = makeView(source, {
      selection: EditorSelection.single(closingFence + 1),
    });
    await settleFocusChange();

    expect(concealedDecorationRanges(view)).toEqual([]);
    expect(view.contentDOM.textContent).toContain("```ts");
    expect(view.contentDOM.textContent).toContain("```");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("keeps unsupported and unparsed source visible and losslessly serializable", () => {
    const rawSource = [
      "\uFEFF---",
      "key: value",
      "---",
      "<div>html</div>",
      "$$ math $$",
      "![[embed]] [[wiki]]",
      "```md",
      "# literal",
      "```",
      "::: malformed ** marker  ",
      "",
    ].join("\r\n");
    const prepared = prepareMarkdownSource(rawSource);
    const parsed = makeView(prepared.editorDocument, {
      selection: EditorSelection.single(prepared.editorDocument.length),
    });
    const unparsedSource = "# Heading and **bold**";
    const unparsed = makeView(unparsedSource, {
      language: false,
      selection: EditorSelection.single(unparsedSource.length),
    });

    expect(concealedDecorationRanges(parsed)).toEqual([]);
    expect(
      serializeMarkdownSource(parsed.state.doc.toString(), prepared.profile),
    ).toBe(rawSource);
    expect(parsed.contentDOM.textContent).toContain("![[embed]] [[wiki]]");
    expect(parsed.contentDOM.textContent).toContain("```md");
    expect(concealedDecorationRanges(unparsed)).toEqual([]);
    expect(unparsed.contentDOM.textContent).toContain("# Heading and **bold**");
    expect(unparsed.state.doc.toString()).toBe(unparsedSource);
  });

  it("builds concealment only for currently visible ranges in a large document", () => {
    const source = Array.from({ length: 10_000 }, (_, index) => `**line ${index}**`).join("\n");
    const view = makeView(source, {
      selection: EditorSelection.single(0),
    });
    expect(forceParsing(view, view.visibleRanges[0]?.to, 100)).toBe(true);
    const concealed = concealedDecorationRanges(view);

    expect(concealed.length).toBeGreaterThan(0);
    expect(concealed.length).toBeLessThan(20_000);
    expect(
      concealed.every((range) =>
        view.visibleRanges.some(
          (visible) => range.from < visible.to && range.to > visible.from,
        ),
      ),
    ).toBe(true);
  });

  it("presents parsed syntax on the first paint of a very long visible line", () => {
    const source = `**${"visible".repeat(2_000)}**\n\naway`;
    const view = makeView(source, {
      selection: EditorSelection.single(source.length),
    });

    expect(view.contentDOM.querySelector(".cm-writing-strong")).not.toBeNull();
    expect(view.contentDOM.textContent).not.toContain("**visible");
    expect(view.state.doc.toString()).toBe(source);
  });
});
