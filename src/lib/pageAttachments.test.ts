// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it } from "vitest";
import {
  attachmentKind,
  attachmentName,
  attachmentUrl,
  isAttachableFile,
  resetAttachmentsDirForTest,
  uploadName,
} from "./pageAttachments";

afterEach(() => resetAttachmentsDirForTest(null));

describe("page attachments", () => {
  it("recognizes only links into the attachments folder", () => {
    expect(attachmentName("_attachments/home-1.png")).toBe("home-1.png");
    expect(attachmentName("<_attachments/home 1.png>")).toBe("home 1.png");
    expect(attachmentName("_attachments/%E9%A6%96%E9%A0%81.png")).toBe("首頁.png");
    expect(attachmentName("https://example.com/a.png")).toBeNull();
    expect(attachmentName("_attachments/../secret.png")).toBeNull();
    expect(attachmentName("_attachments/.hidden.png")).toBeNull();
    expect(attachmentName("./_attachments/a.png")).toBeNull();
  });

  it("tells images from videos by extension", () => {
    expect(attachmentKind("a.PNG")).toBe("image");
    expect(attachmentKind("a.mov")).toBe("video");
    expect(attachmentKind("a.pdf")).toBeNull();
  });

  it("builds a webview URL only once the folder is known", () => {
    expect(attachmentUrl("_attachments/a.png")).toBeNull();
    resetAttachmentsDirForTest("/pages/_attachments");
    expect(attachmentUrl("_attachments/a.png")).toBe("asset://localhost//pages/_attachments/a.png");
    expect(attachmentUrl("_attachments/a.txt")).toBeNull();
    expect(attachmentUrl("_attachments/b.mp4", "C:\\Users\\me\\pages\\_attachments"))
      .toBe("asset://localhost/C:\\Users\\me\\pages\\_attachments\\b.mp4");
  });

  it("names pasted files so the daemon accepts them", () => {
    expect(uploadName(new File(["x"], "shot.png", { type: "image/png" }))).toBe("shot.png");
    expect(uploadName(new File(["x"], "", { type: "image/jpeg" }))).toBe("pasted-image.jpg");
    expect(uploadName(new File(["x"], "clip", { type: "video/quicktime" }))).toBe("video.mov");
    expect(isAttachableFile(new File(["x"], "notes.txt", { type: "text/plain" }))).toBe(false);
    expect(isAttachableFile(new File(["x"], "film.webm", { type: "" }))).toBe(true);
  });
});
