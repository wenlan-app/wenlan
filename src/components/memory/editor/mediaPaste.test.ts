// SPDX-License-Identifier: AGPL-3.0-only
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { installCodeMirrorDomPolyfills } from "./editorTestUtils";
import { mediaInsertText, mediaPaste } from "./mediaPaste";
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

describe("dropping files from Finder or Explorer", () => {
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
