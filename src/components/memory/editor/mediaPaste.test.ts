// SPDX-License-Identifier: AGPL-3.0-only
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { installCodeMirrorDomPolyfills } from "./editorTestUtils";
import { dataUrlFile, mediaInsertText, mediaPaste, pastedImageSources, type MediaPasteOptions } from "./mediaPaste";
import type { AttachedFile } from "../../../lib/tauri";

beforeAll(() => installCodeMirrorDomPolyfills());

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
});

function attached(markdown: string): AttachedFile {
  return { path: "_attachments/x.png", absolute_path: "/p/_attachments/x.png", kind: "image", bytes: 1, markdown, created: true };
}

function pasteEvent(files: File[]): Event {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", { value: { files, getData: () => "", types: ["Files"] } });
  return event;
}

describe("media paste", () => {
  it("puts the Markdown line on its own line", () => {
    const state = EditorState.create({ doc: "before after" });
    expect(mediaInsertText(state, 6, "![a](_attachments/a.png)")).toBe("\n![a](_attachments/a.png)\n");
    expect(mediaInsertText(EditorState.create({ doc: "line\n" }), 5, "![a](x)")).toBe("![a](x)");
  });

  it("uploads a pasted image and inserts its line at the cursor", async () => {
    const upload = vi.fn(async () => attached("![shot](_attachments/shot-1.png)"));
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: "Intro\n",
        selection: EditorSelection.single(6),
        extensions: [mediaPaste({ blocked: () => false, upload })],
      }),
    });
    views.push(view);
    const event = pasteEvent([new File(["png"], "shot.png", { type: "image/png" })]);
    view.contentDOM.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("Intro\n![shot](_attachments/shot-1.png)"));
    expect(upload).toHaveBeenCalledTimes(1);
  });

  it("shows where a file will land while it uploads, without writing it into the page", async () => {
    let finish: (value: AttachedFile) => void = () => {};
    const upload = vi.fn(() => new Promise<AttachedFile>((resolve) => { finish = resolve; }));
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: "Intro\n",
        selection: EditorSelection.single(6),
        extensions: [mediaPaste({ blocked: () => false, upload, pendingLabel: (name) => `Adding ${name}…` })],
      }),
    });
    views.push(view);
    view.contentDOM.dispatchEvent(pasteEvent([new File(["png"], "shot.png", { type: "image/png" })]));

    expect(view.contentDOM.querySelector(".cm-media-pending")?.textContent).toBe("Adding shot.png…");
    expect(view.state.doc.toString()).toBe("Intro\n");
    view.dispatch({ changes: { from: 0, insert: "New " } });

    finish(attached("![shot](_attachments/shot-1.png)"));
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("New Intro\n![shot](_attachments/shot-1.png)"));
    expect(view.contentDOM.querySelector(".cm-media-pending")).toBeNull();
  });

  it("clears the upload note when the upload fails", async () => {
    const onError = vi.fn();
    const upload = vi.fn(async () => { throw new Error("nope"); });
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: "", extensions: [mediaPaste({ blocked: () => false, upload, onError })] }),
    });
    views.push(view);
    view.contentDOM.dispatchEvent(pasteEvent([new File(["png"], "shot.png", { type: "image/png" })]));

    await vi.waitFor(() => expect(onError).toHaveBeenCalledTimes(1));
    expect(view.contentDOM.querySelector(".cm-media-pending")).toBeNull();
    expect(view.state.doc.toString()).toBe("");
  });

  it("leaves non-media pastes to the editor", () => {
    const upload = vi.fn();
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: "", extensions: [mediaPaste({ blocked: () => false, upload })] }),
    });
    views.push(view);
    const event = pasteEvent([new File(["x"], "notes.txt", { type: "text/plain" })]);
    view.contentDOM.dispatchEvent(event);
    expect(upload).not.toHaveBeenCalled();
    expect(view.state.doc.toString()).toBe("");
  });
});

/** A paste as the system clipboard hands it to the webview. */
function clipboardPaste(shape: { files?: File[]; items?: File[]; html?: string; text?: string }): Event {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  const data: Record<string, string> = {};
  if (shape.html !== undefined) data["text/html"] = shape.html;
  if (shape.text !== undefined) data["text/plain"] = shape.text;
  const items = (shape.items ?? []).map((file) => ({ kind: "file", type: file.type, getAsFile: () => file }));
  Object.defineProperty(event, "clipboardData", {
    value: {
      files: shape.files ?? [],
      items,
      types: [...Object.keys(data), ...(shape.files?.length || items.length ? ["Files"] : [])],
      getData: (type: string) => data[type] ?? "",
    },
  });
  return event;
}

describe("pasting an image copied in a browser", () => {
  function editor(options: Partial<MediaPasteOptions>): EditorView {
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: "", extensions: [mediaPaste({ blocked: () => false, ...options })] }),
    });
    views.push(view);
    return view;
  }

  // Chrome's "Copy image" on Windows: the bitmap as a PNG file plus HTML naming it.
  it("prefers the image data over the HTML that names it", async () => {
    const upload = vi.fn(async () => attached("![a](_attachments/image.png)"));
    const uploadUrl = vi.fn();
    const view = editor({ upload, uploadUrl });
    const event = clipboardPaste({
      files: [new File(["png"], "image.png", { type: "image/png" })],
      html: '<meta charset="utf-8"><img src="https://encrypted-tbn0.gstatic.com/images?q=tbn:abc">',
    });
    view.contentDOM.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("![a](_attachments/image.png)"));
    expect(uploadUrl).not.toHaveBeenCalled();
  });

  it("reads the image from the clipboard items when the file list is empty", async () => {
    const upload = vi.fn(async () => attached("![a](_attachments/image.png)"));
    const view = editor({ upload });
    view.contentDOM.dispatchEvent(clipboardPaste({ items: [new File(["png"], "image.png", { type: "image/png" })] }));
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("![a](_attachments/image.png)"));
  });

  // Copying an image on a Mac often leaves only TIFF data.
  it("turns an image the page cannot show into a PNG first", async () => {
    const upload = vi.fn(async (_file: File) => attached("![a](_attachments/image.png)"));
    const convertImage = vi.fn(async () => new File(["png"], "image.png", { type: "image/png" }));
    const view = editor({ upload, convertImage });
    view.contentDOM.dispatchEvent(clipboardPaste({ files: [new File(["tiff"], "image.tiff", { type: "image/tiff" })] }));
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("![a](_attachments/image.png)"));
    expect(convertImage).toHaveBeenCalledTimes(1);
    expect(upload.mock.calls[0][0].type).toBe("image/png");
  });

  it("downloads an image the clipboard names only by its web address", async () => {
    const uploadUrl = vi.fn(async () => attached("![a](_attachments/pasted-image.jpg)"));
    const labels: string[] = [];
    const view = editor({ uploadUrl, pendingLabel: (name) => { labels.push(name); return "Adding image…"; } });
    const event = clipboardPaste({
      html: '<html><body><!--StartFragment--><img src="https://example.com/cat.jpg" alt="cat"><!--EndFragment--></body></html>',
    });
    view.contentDOM.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("![a](_attachments/pasted-image.jpg)"));
    expect(uploadUrl).toHaveBeenCalledWith("https://example.com/cat.jpg");
    expect(labels).toEqual([""]);
  });

  it("saves an image the HTML carries inline", async () => {
    const upload = vi.fn(async (_file: File) => attached("![a](_attachments/pasted-image.png)"));
    const uploadUrl = vi.fn();
    const view = editor({ upload, uploadUrl });
    view.contentDOM.dispatchEvent(clipboardPaste({ html: '<img src="data:image/png;base64,iVBORw0KGgo=">' }));
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("![a](_attachments/pasted-image.png)"));
    expect(uploadUrl).not.toHaveBeenCalled();
    expect(upload.mock.calls[0][0].type).toBe("image/png");
  });

  it("leaves a copied passage with pictures in it to the editor", () => {
    const uploadUrl = vi.fn();
    const view = editor({ uploadUrl });
    const event = clipboardPaste({
      html: '<p>A cat <img src="https://example.com/cat.jpg"> sleeping</p>',
      text: "A cat sleeping",
    });
    view.contentDOM.dispatchEvent(event);
    expect(uploadUrl).not.toHaveBeenCalled();
  });
});

describe("reading the clipboard's images", () => {
  function data(html: string, text = ""): DataTransfer {
    return { getData: (type: string) => (type === "text/html" ? html : type === "text/plain" ? text : "") } as unknown as DataTransfer;
  }

  it("takes web and inline images from an image-only paste", () => {
    expect(pastedImageSources(data('<img src="https://a.test/1.png"><img src="https://a.test/1.png"><img src="data:image/gif;base64,R0lG">')))
      .toEqual(["https://a.test/1.png", "data:image/gif;base64,R0lG"]);
  });

  it("accepts plain text that is only the image's address", () => {
    expect(pastedImageSources(data('<img src="https://a.test/1.png">', "https://a.test/1.png"))).toEqual(["https://a.test/1.png"]);
    expect(pastedImageSources(data('<img src="https://a.test/1.png">', "caption"))).toEqual([]);
  });

  it("ignores addresses the app cannot load", () => {
    expect(pastedImageSources(data('<img src="blob:https://a.test/x"><img src="file:///tmp/a.png">'))).toEqual([]);
  });

  it("decodes base64 and plain data addresses", () => {
    const png = dataUrlFile("data:image/png;base64,iVBORw0KGgo=");
    expect(png?.type).toBe("image/png");
    expect(png?.size).toBe(8);
    const svg = dataUrlFile("data:image/svg+xml;charset=utf-8,%3Csvg%2F%3E");
    expect(svg?.type).toBe("image/svg+xml");
    expect(svg?.size).toBe(6);
    expect(dataUrlFile("data:text/plain,hi")).toBeNull();
  });
});

describe("dropping files from Finder or Explorer", () => {
  function dragEvent(type: string, files: File[]): Event {
    const event = new Event(type, { bubbles: true, cancelable: true });
    const dataTransfer = { files, types: ["Files"], dropEffect: "none", getData: () => "" };
    Object.defineProperties(event, { dataTransfer: { value: dataTransfer }, clientX: { value: 0 }, clientY: { value: 0 } });
    return event;
  }

  function editor(upload: () => Promise<AttachedFile>, blocked = false): EditorView {
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({ doc: "Intro\n", extensions: [mediaPaste({ blocked: () => blocked, upload })] }),
    });
    views.push(view);
    return view;
  }

  // A preview widget or the editor's margin is not editable text, so the drop
  // lands outside CodeMirror's content; left alone, WebKit opens the file in
  // place of the app.
  it("takes a file dragged over any part of the editor", async () => {
    const upload = vi.fn(async () => attached("![a](_attachments/a.png)"));
    const view = editor(upload);
    const over = dragEvent("dragover", [new File(["png"], "a.png", { type: "image/png" })]);
    view.dom.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    const drop = dragEvent("drop", [new File(["png"], "a.png", { type: "image/png" })]);
    view.dom.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(view.state.doc.toString()).toContain("![a](_attachments/a.png)"));
    expect(upload).toHaveBeenCalledTimes(1);
  });

  it("keeps CodeMirror from reading a dropped image as text", async () => {
    const upload = vi.fn(async () => attached("![a](_attachments/a.png)"));
    const view = editor(upload);
    view.contentDOM.dispatchEvent(dragEvent("drop", [new File(["png-bytes"], "a.png", { type: "image/png" })]));
    await vi.waitFor(() => expect(view.state.doc.toString()).toContain("![a](_attachments/a.png)"));
    expect(view.state.doc.toString()).not.toContain("png-bytes");
  });

  it("does not accept a drop while the editor is blocked", () => {
    const upload = vi.fn(async () => attached("![a](x)"));
    const view = editor(upload, true);
    const over = dragEvent("dragover", [new File(["png"], "a.png", { type: "image/png" })]);
    view.dom.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(false);
    view.dom.dispatchEvent(dragEvent("drop", [new File(["png"], "a.png", { type: "image/png" })]));
    expect(upload).not.toHaveBeenCalled();
  });

  // With Tauri's own file drop on, the webview never sees the drop, so the
  // editor's drop handler above would not run.
  it.each(["tauri.conf.json", "tauri.review.conf.json", "tauri.dev-live.conf.json"])(
    "%s hands file drops to the page",
    (name) => {
      const config = JSON.parse(readFileSync(resolve(process.cwd(), "app", name), "utf8"));
      const main = config.app.windows.find((window: { label: string }) => window.label === "main");
      expect(main.dragDropEnabled).toBe(false);
    },
  );
});
