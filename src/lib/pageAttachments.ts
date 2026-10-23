// SPDX-License-Identifier: AGPL-3.0-only
import { convertFileSrc } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import { attachBytesToPage, pageAttachmentsDir, type AttachedFile } from "./tauri";

/**
 * Images and videos placed in pages live in the pages folder's
 * `_attachments/` and are linked as `![alt](_attachments/name.png)`, the
 * vault-root link Obsidian also reads. This module turns that link into a URL
 * the webview can load and uploads pasted or dropped files.
 */
export const ATTACHMENTS_PREFIX = "_attachments/";

const IMAGE_EXTENSIONS = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "heic"]);
const VIDEO_EXTENSIONS = new Set(["mp4", "m4v", "mov", "webm"]);

export type AttachmentKind = "image" | "video";

let attachmentsDir: string | null = null;
let pending: Promise<string | null> | null = null;
const listeners = new Set<() => void>();

/** Ask once for the attachments folder; later calls share the answer. */
export function primeAttachmentsDir(): Promise<string | null> {
  if (attachmentsDir !== null) return Promise.resolve(attachmentsDir);
  pending ??= Promise.resolve()
    .then(() => pageAttachmentsDir())
    .then((dir) => {
      if (typeof dir !== "string" || !dir) throw new Error("no attachments folder");
      attachmentsDir = dir;
      listeners.forEach((listener) => listener());
      return dir;
    })
    .catch(() => {
      pending = null;
      return null;
    });
  return pending;
}

/** The attachments folder once known, or null. */
export function knownAttachmentsDir(): string | null {
  return attachmentsDir;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  void primeAttachmentsDir();
  return () => listeners.delete(listener);
}

/** Call `listener` when the attachments folder becomes known; does not ask for it. */
export function subscribeAttachmentsDir(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The attachments folder, re-rendering when it becomes known. */
export function useAttachmentsDir(): string | null {
  return useSyncExternalStore(subscribe, knownAttachmentsDir, knownAttachmentsDir);
}

/** For tests: forget the cached folder. */
export function resetAttachmentsDirForTest(dir: string | null = null): void {
  attachmentsDir = dir;
  pending = null;
  listeners.forEach((listener) => listener());
}

function extensionOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

/** The file name an `_attachments/` link points at, or null for any other link. */
export function attachmentName(src: string | undefined | null): string | null {
  if (!src) return null;
  let link = src.trim();
  if (link.startsWith("<") && link.endsWith(">")) link = link.slice(1, -1);
  if (!link.startsWith(ATTACHMENTS_PREFIX)) return null;
  let name = link.slice(ATTACHMENTS_PREFIX.length);
  try {
    name = decodeURIComponent(name);
  } catch {
    return null;
  }
  if (!name || name.startsWith(".") || /[/\\]/.test(name)) return null;
  return name;
}

export function attachmentKind(name: string): AttachmentKind | null {
  const extension = extensionOf(name);
  if (IMAGE_EXTENSIONS.has(extension)) return "image";
  if (VIDEO_EXTENSIONS.has(extension)) return "video";
  return null;
}

/** The webview URL for an `_attachments/` link, or null when it is not one. */
export function attachmentUrl(src: string | undefined | null, dir = attachmentsDir): string | null {
  const name = attachmentName(src);
  if (!name || !dir || !attachmentKind(name)) return null;
  const separator = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  const base = dir.endsWith(separator) ? dir : `${dir}${separator}`;
  return convertFileSrc(`${base}${name}`);
}

/** A video URL that starts at its first frame, so WebKit paints a still before play. */
export function videoSource(url: string): string {
  return url.includes("#") ? url : `${url}#t=0.1`;
}

/** Whether a dropped or pasted file is one a page can show. */
export function isAttachableFile(file: File): boolean {
  return attachmentKind(uploadName(file)) !== null;
}

function mimeExtension(type: string): string {
  switch (type) {
    case "image/jpeg": return "jpg";
    case "image/svg+xml": return "svg";
    case "video/quicktime": return "mov";
    default: return type.split("/")[1] ?? "";
  }
}

/** A name with an extension the daemon accepts, for pasted files named "image.png" or nothing. */
export function uploadName(file: File): string {
  if (file.name && attachmentKind(file.name)) return file.name;
  const stem = file.type.startsWith("video/") ? "video" : "pasted-image";
  return `${stem}.${mimeExtension(file.type)}`;
}

/** Copy one pasted or dropped file into the pages folder. */
export async function uploadAttachment(file: File): Promise<AttachedFile> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  return attachBytesToPage(uploadName(file), bytes);
}
