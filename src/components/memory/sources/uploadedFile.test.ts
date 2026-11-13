import { describe, expect, it } from "vitest";
import { uploadedFilePath } from "./uploadedFile";

describe("uploadedFilePath", () => {
  const folder = "/Users/me/.wenlan/sources";
  it("finds a file added with Add your material, named absolutely or under the knowledge home", () => {
    expect(uploadedFilePath("dir-1::/Users/me/.wenlan/sources/paper.pdf", folder)).toBe("/Users/me/.wenlan/sources/paper.pdf");
    expect(uploadedFilePath("dir-1::sources/Notes/week1/intro.md", folder)).toBe("sources/Notes/week1/intro.md");
    expect(uploadedFilePath("dir-1::C:\\Users\\me\\.wenlan\\sources\\a.docx", "C:\\Users\\me\\.wenlan\\sources")).toBe("C:/Users/me/.wenlan/sources/a.docx");
  });
  it("never offers a file from the person's own folders", () => {
    expect(uploadedFilePath("dir-2::/Users/me/Documents/paper.pdf", folder)).toBeNull();
    expect(uploadedFilePath("dir-2::/Users/me/.wenlan/sources-old/paper.pdf", folder)).toBeNull();
    expect(uploadedFilePath("dir-2::/sources/paper.pdf", folder)).toBeNull();
    expect(uploadedFilePath("webpage-abc", folder)).toBeNull();
    expect(uploadedFilePath("dir-1::sources/paper.pdf", undefined)).toBeNull();
  });
});
