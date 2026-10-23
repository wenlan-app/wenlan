// SPDX-License-Identifier: AGPL-3.0-only
import type { EditorState, Extension } from "@codemirror/state";
import { EditorView, ViewPlugin } from "@codemirror/view";
import type { AttachedFile } from "../../../lib/tauri";
import { isAttachableFile, primeAttachmentsDir, uploadAttachment } from "../../../lib/pageAttachments";

export interface MediaPasteOptions {
  blocked(view: EditorView): boolean;
  /** Upload one file; defaults to copying it into the pages folder. */
  upload?(file: File): Promise<AttachedFile>;
  onError?(file: File, error: unknown): void;
}

/** The Markdown line on a line of its own at `at`. */
export function mediaInsertText(state: EditorState, at: number, markdown: string): string {
  const before = at > 0 ? state.sliceDoc(at - 1, at) : "\n";
  const after = at < state.doc.length ? state.sliceDoc(at, at + 1) : "\n";
  return `${before === "\n" ? "" : "\n"}${markdown}${after === "\n" ? "" : "\n"}`;
}

/**
 * Pasting or dropping an image or video copies it into the pages folder and
 * puts its Markdown line where it landed. Other pastes and drops are left to
 * CodeMirror.
 */
export function mediaPaste(options: MediaPasteOptions): Extension {
  const upload = options.upload ?? uploadAttachment;
  const insertFiles = (view: EditorView, files: File[], at: number): void => {
    void (async () => {
      let position = at;
      for (const file of files) {
        let attached: AttachedFile;
        try {
          attached = await upload(file);
        } catch (error) {
          options.onError?.(file, error);
          continue;
        }
        void primeAttachmentsDir();
        if (!view.dom.isConnected || view.state.readOnly) return;
        position = Math.min(position, view.state.doc.length);
        const insert = mediaInsertText(view.state, position, attached.markdown);
        view.dispatch({
          changes: { from: position, insert },
          selection: { anchor: position + insert.length },
          userEvent: "input.paste",
          scrollIntoView: true,
        });
        position += insert.length;
      }
    })();
  };
  const take = (view: EditorView, list: FileList | undefined | null): File[] | null => {
    const files = [...(list ?? [])].filter(isAttachableFile);
    return files.length > 0 && !options.blocked(view) ? files : null;
  };
  // CodeMirror only sees drops on editable text: an image or video preview is
  // a widget it ignores, and WebKit opens a file dropped there in place of the
  // app. Listening on the whole editor takes the drop anywhere inside it.
  const drops = ViewPlugin.define((view) => {
    const dragover = (event: DragEvent) => {
      if (!event.dataTransfer?.types.includes("Files") || options.blocked(view)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
    };
    // Capture phase, so CodeMirror's own file drop does not read the image as text.
    const drop = (event: DragEvent) => {
      const files = take(view, event.dataTransfer?.files);
      if (!files) return;
      event.preventDefault();
      event.stopPropagation();
      const at = view.posAtCoords({ x: event.clientX, y: event.clientY }, false);
      insertFiles(view, files, at);
    };
    view.dom.addEventListener("dragover", dragover);
    view.dom.addEventListener("drop", drop, true);
    return {
      destroy() {
        view.dom.removeEventListener("dragover", dragover);
        view.dom.removeEventListener("drop", drop, true);
      },
    };
  });
  return [
    drops,
    EditorView.domEventHandlers({
      paste(event, view) {
        const files = take(view, event.clipboardData?.files);
        if (!files) return false;
        event.preventDefault();
        insertFiles(view, files, view.state.selection.main.head);
        return true;
      },
    }),
  ];
}
