// SPDX-License-Identifier: AGPL-3.0-only
//
// A page's pictures inside its look (UX-LOOK-8). The look's frame cannot load
// anything, so the app reads each picture the page keeps in its pages folder
// and hands it in as a data: URL. A picture that cannot be read, is not an
// image, or is too big gets an empty url and the look shows its alt text.

import { convertFileSrc } from "@tauri-apps/api/core";
import { attachmentUrl, ATTACHMENTS_PREFIX } from "../../../../lib/pageAttachments";
import { resolvePageEmbed, type LookData } from "../../../../lib/tauri";

/** Largest picture handed to a look. */
export const LOOK_IMAGE_MAX_BYTES = 4 * 1024 * 1024;
/** Largest total of pictures one look gets. */
export const LOOK_IMAGES_MAX_BYTES = 12 * 1024 * 1024;

/** The only URLs a picture may carry into the frame. */
export const LOOK_IMAGE_URL = /^data:image\/[a-z0-9.+-]+;base64,[A-Za-z0-9+/=]*$/;

export interface LookImageReader {
  /** A URL the app's own webview can fetch for `src`, or null. */
  locate: (src: string, pageId: string) => Promise<string | null>;
  fetchBlob: (url: string) => Promise<Blob>;
}

const defaultReader: LookImageReader = {
  async locate(src, pageId) {
    if (src.startsWith(ATTACHMENTS_PREFIX)) {
      const direct = attachmentUrl(src);
      if (direct) return direct;
    }
    let name = src;
    try { name = decodeURIComponent(src); } catch { /* keep as written */ }
    const path = await resolvePageEmbed(name, pageId);
    return typeof path === "string" && path ? convertFileSrc(path) : null;
  },
  async fetchBlob(url) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`picture ${response.status}`);
    return response.blob();
  },
};

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === "string" ? reader.result : "");
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

/** `data` with each picture's `url` filled in, or empty when it cannot be. */
export async function withLookImages(data: LookData, reader: LookImageReader = defaultReader): Promise<LookData> {
  const images = data.page.images ?? [];
  if (images.length === 0) return data;
  let total = 0;
  const urls: string[] = [];
  // One at a time, so the total cap holds and a page full of pictures does
  // not read them all at once.
  for (const image of images) {
    let url = "";
    try {
      const located = await reader.locate(image.src, data.page.id);
      if (located) {
        const blob = await reader.fetchBlob(located);
        if (blob.type.startsWith("image/") && blob.size <= LOOK_IMAGE_MAX_BYTES && total + blob.size <= LOOK_IMAGES_MAX_BYTES) {
          const read = await blobToDataUrl(blob);
          if (LOOK_IMAGE_URL.test(read)) {
            url = read;
            total += blob.size;
          }
        }
      }
    } catch {
      url = "";
    }
    urls.push(url);
  }
  return { ...data, page: { ...data.page, images: images.map((image, i) => ({ ...image, url: urls[i] })) } };
}
