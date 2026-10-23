// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import type { AttachedFile } from "../../../lib/tauri";
import { isAttachableFile, primeAttachmentsDir, uploadAttachment } from "../../../lib/pageAttachments";

export interface MediaPasteOptions {
  blocked(view: EditorView): boolean;
  /** Upload one file; defaults to copying it into the pages folder. */
  upload?(file: File): Promise<AttachedFile>;
  onError?(file: File, error: unknown): void;
  /** Shown where the file will land while it uploads. */
  pendingLabel?(file: File): string;
}

class PendingWidget extends WidgetType {
  constructor(readonly label: string) {
    super();
  }

  eq(other: PendingWidget): boolean {
    return other.label === this.label;
  }

  toDOM(): HTMLElement {
    const note = document.createElement("span");
    note.className = "cm-media-pending";
    note.textContent = this.label;
    return note;
  }
}

const addPending = StateEffect.define<{ id: number; at: number; label: string }>({
  map: (value, mapping) => ({ ...value, at: mapping.mapPos(value.at, 1) }),
});
const settlePending = StateEffect.define<number>();

// A decoration, not text, so an upload in flight never reaches the saved page.
const pendingUploads = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(set, transaction) {
    let next = set.map(transaction.changes);
    for (const effect of transaction.effects) {
      if (effect.is(addPending)) {
        const { id, at, label } = effect.value;
        next = next.update({
          add: [Decoration.widget({ widget: new PendingWidget(label), side: 1, pendingId: id }).range(at)],
        });
      } else if (effect.is(settlePending)) {
        next = next.update({ filter: (_from, _to, value) => value.spec.pendingId !== effect.value });
      }
    }
    return next;
  },
});

function pendingPosition(state: EditorState, id: number): number | null {
  let found: number | null = null;
  state.field(pendingUploads).between(0, state.doc.length, (from, _to, value) => {
    if (value.spec.pendingId !== id) return;
    found = from;
    return false;
  });
  return found;
}

let nextPendingId = 0;

const pendingTheme = EditorView.theme({
  ".cm-media-pending": { color: "var(--mem-text-tertiary)" },
});

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
    const pending = files.map((file) => ({ file, id: nextPendingId++ }));
    view.dispatch({
      effects: pending.map(({ file, id }) =>
        addPending.of({ id, at, label: options.pendingLabel?.(file) ?? file.name })),
    });
    void (async () => {
      for (const { file, id } of pending) {
        let attached: AttachedFile | null = null;
        try {
          attached = await upload(file);
        } catch (error) {
          options.onError?.(file, error);
        }
        if (attached) void primeAttachmentsDir();
        if (!view.dom.isConnected) return;
        const position = pendingPosition(view.state, id);
        if (!attached || position === null || view.state.readOnly) {
          view.dispatch({ effects: settlePending.of(id) });
          continue;
        }
        const insert = mediaInsertText(view.state, position, attached.markdown);
        view.dispatch({
          changes: { from: position, insert },
          selection: { anchor: position + insert.length },
          effects: settlePending.of(id),
          userEvent: "input.paste",
          scrollIntoView: true,
        });
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
    pendingUploads,
    EditorView.decorations.from(pendingUploads),
    pendingTheme,
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
