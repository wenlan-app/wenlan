// SPDX-License-Identifier: AGPL-3.0-only
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import type { LookData } from "../../../../lib/tauri";
import { INNER_HELPER_SCRIPT } from "./lookDocument";
import { LOOK_IMAGE_MAX_BYTES, withLookImages, type LookImageReader } from "./lookImages";

const PNG = new Uint8Array([137, 80, 78, 71]);

const data = (images: { src: string; alt: string }[], html = ""): LookData => ({
  api: 1,
  page: {
    id: "pg_a", title: "A", space: "", created: "", modified: "", text: "", html,
    sections: [{ heading: "", level: 0, text: "", html, items: [], tables: [] }], links: [], images,
  },
  space: { name: "", pages: [], more: false },
  theme: { mode: "dark" },
});

const reader = (files: Record<string, Blob>): LookImageReader => ({
  locate: async (src) => (src in files ? `asset://${src}` : null),
  fetchBlob: async (url) => files[url.slice("asset://".length)],
});

describe("UX-LOOK-8 a page's pictures inside its look", () => {
  it("hands each picture in as a data: URL, and an empty one when it cannot", async () => {
    const out = await withLookImages(
      data([
        { src: "_attachments/a.png", alt: "A" },
        { src: "missing.png", alt: "Gone" },
        { src: "notes.png", alt: "Not a picture" },
        { src: "huge.png", alt: "Too big" },
      ]),
      reader({
        "_attachments/a.png": new Blob([PNG], { type: "image/png" }),
        "notes.png": new Blob(["<script>"], { type: "text/html" }),
        "huge.png": new Blob([new Uint8Array(LOOK_IMAGE_MAX_BYTES + 1)], { type: "image/png" }),
      }),
    );
    expect(out.page.images?.map((image) => image.url)).toEqual(["data:image/png;base64,iVBORw==", "", "", ""]);
    expect(out.page.images?.[0].alt).toBe("A");
  });

  it("leaves a page without pictures as it was", async () => {
    const input = data([]);
    expect(await withLookImages(input, reader({}))).toBe(input);
  });

  it("puts each picture in its place in the page's html inside the frame", () => {
    const html = '<p><img data-wenlan-image="0" alt="A"> and <img data-wenlan-image="1" alt="B"></p>';
    const input = data([{ src: "a.png", alt: "A" }, { src: "b.png", alt: "B" }], html);
    input.page.images = [
      { src: "a.png", alt: "A", url: "data:image/png;base64,iVBORw==" },
      { src: "b.png", alt: "B", url: "javascript:alert(1)" },
    ];
    const context: Record<string, unknown> = {};
    context.window = context;
    context.parent = { postMessage: () => {} };
    context.addEventListener = () => {};
    context.document = { getElementById: () => ({ textContent: JSON.stringify(input) }), addEventListener: () => {} };
    for (const name of ["Window", "Document", "HTMLElement", "SVGElement", "EventTarget"]) context[name] = function () {};
    runInNewContext(INNER_HELPER_SCRIPT, context);
    const wenlan = context.wenlan as LookData;
    const expected = '<p><img src="data:image/png;base64,iVBORw==" data-wenlan-image="0" alt="A"> and <img data-wenlan-image="1" alt="B"></p>';
    expect(wenlan.page.html).toBe(expected);
    expect(wenlan.page.sections[0].html).toBe(expected);
  });
});
