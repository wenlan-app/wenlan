// SPDX-License-Identifier: AGPL-3.0-only

/**
 * With Tauri's own file drop off, a file dropped where no element takes it
 * makes WebKit open that file in place of the app. Refuse such drops so the
 * drag snaps back; drop zones that take files still cancel the event first.
 */
export function refuseStrayFileDrops(target: Window = window): () => void {
  const carriesFiles = (event: DragEvent) => event.dataTransfer?.types.includes("Files") ?? false;
  const dragover = (event: DragEvent) => {
    if (event.defaultPrevented || !carriesFiles(event)) return;
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = "none";
  };
  const drop = (event: DragEvent) => {
    if (carriesFiles(event)) event.preventDefault();
  };
  target.addEventListener("dragover", dragover);
  target.addEventListener("drop", drop);
  return () => {
    target.removeEventListener("dragover", dragover);
    target.removeEventListener("drop", drop);
  };
}
