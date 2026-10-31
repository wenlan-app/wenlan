// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, WidgetType } from "@codemirror/view";
import { attachImageUrlToPage, type AttachedFile } from "../../../lib/tauri";
import { isAttachableFile, primeAttachmentsDir, uploadAttachment } from "../../../lib/pageAttachments";

export interface MediaPasteOptions {
  blocked(view: EditorView): boolean;
  /** Upload one file; defaults to copying it into the pages folder. */
  upload?(file: File): Promise<AttachedFile>;
  /** Fetch an image the clipboard names only by its web address; defaults to the app downloading it. */
  uploadUrl?(url: string): Promise<AttachedFile>;
  /** Turn an image the page cannot show (TIFF, BMP) into a PNG; defaults to redrawing it. */
  convertImage?(file: File): Promise<File>;
  /** `name` is empty for an image the clipboard gave no name. */
  onError?(name: string, error: unknown): void;
  /** Shown where the file will land while it uploads; `name` is empty for an unnamed image. */
  pendingLabel?(name: string): string;
}

/** One file on its way into the page. */
interface MediaJob {
  name: string;
  run(): Promise<AttachedFile>;
}

/** The name of an image that arrived without one; callers word it themselves. */
const PASTED_IMAGE_NAME = "";
const MAX_PASTED_IMAGES = 20;

/** Every file on the clipboard, read from `items` too where `files` comes back empty. */
export function clipboardFiles(data: DataTransfer | null | undefined): File[] {
  if (!data) return [];
  const files = [...(data.files ?? [])];
  if (files.length > 0) return files;
  for (const item of [...(data.items ?? [])]) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (file) files.push(file);
  }
  return files;
}

/** An image in a format the page cannot show as is, but the webview can draw. */
function isConvertibleImage(file: File): boolean {
  return file.type.startsWith("image/") && !isAttachableFile(file);
}

/**
 * The images a paste names only through HTML: what a browser's "Copy image"
 * leaves when the image data itself is missing or unreadable. Only a paste
 * with no text of its own counts, so copying a passage with pictures in it
 * still pastes the passage.
 */
export function pastedImageSources(data: DataTransfer | null | undefined): string[] {
  if (!data) return [];
  const html = data.getData("text/html");
  if (!html || !/<img\b/i.test(html)) return [];
  const doc = new DOMParser().parseFromString(html, "text/html");
  const sources = [...doc.querySelectorAll("img")]
    .map((image) => image.getAttribute("src")?.trim() ?? "")
    .filter((src) => /^https?:\/\//i.test(src) || /^data:image\//i.test(src));
  const unique = [...new Set(sources)].slice(0, MAX_PASTED_IMAGES);
  if (unique.length === 0) return [];
  if ((doc.body?.textContent ?? "").trim()) return [];
  const text = data.getData("text/plain").trim();
  if (text && !unique.includes(text)) return [];
  return unique;
}

/** A `data:image/...` address as a file, or null when it is not one. */
export function dataUrlFile(src: string): File | null {
  const match = /^data:(image\/[\w.+-]+)((?:;[^,;]*)*?)(;base64)?,(.*)$/is.exec(src);
  if (!match) return null;
  const [, type, , base64, payload] = match;
  try {
    let bytes: Uint8Array<ArrayBuffer>;
    if (base64) {
      const binary = atob(payload.replace(/\s+/g, ""));
      bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    } else {
      bytes = new TextEncoder().encode(decodeURIComponent(payload));
    }
    return new File([bytes], "", { type: type.toLowerCase() });
  } catch {
    return null;
  }
}

/** Redraw an image the webview can decode as a PNG the page can show. */
async function redrawAsPng(file: File): Promise<File> {
  const url = URL.createObjectURL(file);
  try {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement("canvas");
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext("2d");
    if (!context || !canvas.width || !canvas.height) throw new Error("image could not be drawn");
    context.drawImage(image, 0, 0);
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
    if (!blob) throw new Error("image could not be converted");
    const stem = file.name.replace(/\.[^.]*$/, "") || "pasted-image";
    return new File([blob], `${stem}.png`, { type: "image/png" });
  } finally {
    URL.revokeObjectURL(url);
  }
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
 *
 * Clipboards differ by browser and system: an image copied in Chrome or Edge
 * can arrive as a file (Windows, and Mac when WebKit reads it), as TIFF data,
 * or only as HTML naming the image's web address. Each shape lands as the
 * same image.
 */
export function mediaPaste(options: MediaPasteOptions): Extension {
  const upload = options.upload ?? uploadAttachment;
  const uploadUrl = options.uploadUrl ?? attachImageUrlToPage;
  const convertImage = options.convertImage ?? redrawAsPng;
  const fileJob = (file: File): MediaJob => ({
    name: file.name || PASTED_IMAGE_NAME,
    run: async () => upload(isConvertibleImage(file) ? await convertImage(file) : file),
  });
  const sourceJob = (src: string): MediaJob | null => {
    if (/^data:/i.test(src)) {
      const file = dataUrlFile(src);
      return file && (isAttachableFile(file) || isConvertibleImage(file))
        ? { ...fileJob(file), name: PASTED_IMAGE_NAME }
        : null;
    }
    return { name: PASTED_IMAGE_NAME, run: () => uploadUrl(src) };
  };
  const insertJobs = (view: EditorView, jobs: MediaJob[], at: number): void => {
    const pending = jobs.map((job) => ({ job, id: nextPendingId++ }));
    view.dispatch({
      effects: pending.map(({ job, id }) =>
        addPending.of({ id, at, label: options.pendingLabel?.(job.name) ?? (job.name || "image") })),
    });
    void (async () => {
      for (const { job, id } of pending) {
        let attached: AttachedFile | null = null;
        try {
          attached = await job.run();
        } catch (error) {
          options.onError?.(job.name, error);
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
  const take = (view: EditorView, list: Iterable<File> | undefined | null): MediaJob[] | null => {
    const files = [...(list ?? [])].filter((file) => isAttachableFile(file) || isConvertibleImage(file));
    return files.length > 0 && !options.blocked(view) ? files.map(fileJob) : null;
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
      const jobs = take(view, event.dataTransfer?.files);
      if (!jobs) return;
      event.preventDefault();
      event.stopPropagation();
      const at = view.posAtCoords({ x: event.clientX, y: event.clientY }, false);
      insertJobs(view, jobs, at);
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
        if (options.blocked(view)) return false;
        const data = event.clipboardData;
        const jobs = take(view, clipboardFiles(data))
          ?? pastedImageSources(data).map(sourceJob).filter((job): job is MediaJob => job !== null);
        if (jobs.length === 0) return false;
        event.preventDefault();
        insertJobs(view, jobs, view.state.selection.main.head);
        return true;
      },
    }),
  ];
}
