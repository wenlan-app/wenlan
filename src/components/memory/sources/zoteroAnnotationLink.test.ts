import { describe, expect, it } from "vitest";
import { parseZoteroAnnotationLink } from "./zoteroAnnotationLink";

const base = "zotero://open-pdf/library/items/AB12CD34";
describe("Zotero annotation link allowlist", () => {
  it.each(["page=12&annotation=ZX98YU76", "annotation=ZX98YU76&page=12"])("reads only the canonical target and positive page: %s", query => {
    expect(parseZoteroAnnotationLink(`${base}?${query}`)).toEqual({ attachmentKey: "AB12CD34", page: 12, annotationKey: "ZX98YU76" });
  });
  it.each([
    undefined, "", "https://example.com/paper.pdf?page=2&annotation=ZX98YU76", "javascript:alert(1)",
    "zotero://select/library/items/AB12CD34?page=2&annotation=ZX98YU76",
    "zotero://user@open-pdf/library/items/AB12CD34?page=2&annotation=ZX98YU76",
    "zotero://open-pdf:80/library/items/AB12CD34?page=2&annotation=ZX98YU76",
    "zotero://open-pdf/library/items/../AB12CD34?page=2&annotation=ZX98YU76",
    "zotero://open-pdf/library/items/%41B12CD34?page=2&annotation=ZX98YU76",
    "zotero://open-pdf/library/items/ab12cd34?page=2&annotation=ZX98YU76",
    `${base}/?page=2&annotation=ZX98YU76`, `${base}?page=2`, `${base}?annotation=ZX98YU76`,
    `${base}?page=0&annotation=ZX98YU76`, `${base}?page=-2&annotation=ZX98YU76`,
    `${base}?page=1.5&annotation=ZX98YU76`, `${base}?page=Infinity&annotation=ZX98YU76`,
    `${base}?page=9007199254740992&annotation=ZX98YU76`, `${base}?page=2&annotation=short`,
    `${base}?page=2&annotation=zx98yu76`, `${base}?page=2&annotation=ZX98YU76&execute=1`,
    `${base}?page=2&page=3&annotation=ZX98YU76`, `${base}?%70age=2&annotation=ZX98YU76`,
    `${base}?page=2&annotation=ZX98YU76#fragment`, ` ${base}?page=2&annotation=ZX98YU76`,
    `${base}?page=2&annotation=ZX98YU76\n`,
  ])("rejects unsafe or ambiguous links: %s", href => {
    expect(parseZoteroAnnotationLink(href)).toBeNull();
  });
});
