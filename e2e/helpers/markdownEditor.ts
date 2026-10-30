// SPDX-License-Identifier: AGPL-3.0-only
import { expect, type Locator } from "@playwright/test";

type CodeMirrorContent = HTMLElement & {
  cmTile?: { root?: { view?: { state: { doc: { toString(): string } } } } };
};

/**
 * The exact Markdown source a page editor holds. The editor is CodeMirror
 * (`.cm-content`, a contenteditable `textbox`) or, when CodeMirror fails to
 * load, the native `<textarea>`; `toHaveValue` only works on the second.
 * Pass the editor's textbox or any element that contains it.
 */
export async function editorSource(editor: Locator): Promise<string> {
  return editor.evaluate((element) => {
    if (element instanceof HTMLTextAreaElement) return element.value;
    const content = (
      element.classList.contains("cm-content") ? element : element.querySelector(".cm-content")
    ) as CodeMirrorContent | null;
    const view = content?.cmTile?.root?.view;
    if (!view) throw new Error("CodeMirror view is unavailable");
    return view.state.doc.toString();
  });
}

/** Retrying exact-source assertion, the editor counterpart of `toHaveValue`. */
export async function expectEditorSource(editor: Locator, expected: string): Promise<void> {
  await expect.poll(() => editorSource(editor), { message: "editor source" }).toBe(expected);
}
