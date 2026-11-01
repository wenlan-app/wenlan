// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";

const resolvePageEmbed = vi.hoisted(() => vi.fn());
vi.mock("./tauri", () => ({ resolvePageEmbed, attachBytesToPage: vi.fn(), pageAttachmentsDir: vi.fn() }));

import {
  mediaEmbedUrl,
  parseMediaEmbed,
  resetAttachmentsDirForTest,
  resetMediaEmbedsForTest,
  subscribeAttachmentsDir,
} from "./pageAttachments";

afterEach(() => {
  resetAttachmentsDirForTest(null);
  resetMediaEmbedsForTest();
  resolvePageEmbed.mockReset();
});

describe("Obsidian media embeds", () => {
  it("reads the name, a width and alt text, and ignores notes and hidden files", () => {
    expect(parseMediaEmbed("shot.png")).toEqual({ name: "shot.png", kind: "image", alt: "shot.png" });
    expect(parseMediaEmbed("img/shot.PNG|300")).toEqual({ name: "img/shot.PNG", kind: "image", width: 300, alt: "shot.PNG" });
    expect(parseMediaEmbed("shot.png|300x200")?.width).toBe(300);
    expect(parseMediaEmbed("clip.mp4|The demo")).toEqual({ name: "clip.mp4", kind: "video", alt: "The demo" });
    expect(parseMediaEmbed("Some note")).toBeNull();
    expect(parseMediaEmbed("Some note#Heading")).toBeNull();
    expect(parseMediaEmbed(".hidden.png")).toBeNull();
  });

  it("uses the file the daemon finds anywhere in the pages folder", async () => {
    resolvePageEmbed.mockResolvedValue("/vault/img/shot.png");
    const heard = vi.fn();
    const stop = subscribeAttachmentsDir(heard);
    const embed = parseMediaEmbed("shot.png")!;
    expect(mediaEmbedUrl(embed)).toBeNull();
    await vi.waitFor(() => expect(heard).toHaveBeenCalled());
    expect(mediaEmbedUrl(embed)).toBe("asset://localhost//vault/img/shot.png");
    expect(resolvePageEmbed).toHaveBeenCalledOnce();
    stop();
  });

  it("falls back to the attachments folder for a bare name the daemon can't find", async () => {
    resetAttachmentsDirForTest("/vault/_attachments");
    resolvePageEmbed.mockResolvedValue(null);
    const embed = parseMediaEmbed("shot one.png")!;
    expect(mediaEmbedUrl(embed)).toBe("asset://localhost//vault/_attachments/shot one.png");
    expect(mediaEmbedUrl(parseMediaEmbed("img/shot.png")!)).toBeNull();
  });
});
