// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Page } from "../../../lib/tauri";
import { folderSection, pageSection, sectionFolderPath } from "./pageSections";

const page = (overrides: Partial<Page>): Page => ({ id: "p", title: "p", summary: null, content: "", entity_id: null, domain: null, source_memory_ids: [], version: 1, status: "active", created_at: "2026-07-16", last_modified: "2026-07-16", last_compiled: "2026-07-16", ...overrides });

describe("page sections", () => {
  it("reads Rules and Skills from the folder a page already sits in, in any app language and at any depth", () => {
    for (const path of ["Rules", "work/rules", "規則", "筆記/规则/提交", "a\\RULES\\b"]) expect(folderSection(path)).toBe("rules");
    for (const path of ["Skills", "team/skills/writing", "技能"]) expect(folderSection(path)).toBe("skills");
    for (const path of ["", null, "Work", "My Rules", "Rules 2", "Skillset"]) expect(folderSection(path)).toBe("docs");
  });

  it("does not take a Space's own folder for a rules folder", () => {
    expect(folderSection("Rules", "Rules")).toBe("docs");
    expect(folderSection("Rules/Rules", "Rules")).toBe("rules");
    expect(folderSection("Rules", "Work")).toBe("rules");
  });

  it("puts every page in exactly one section: a How I work page is a rules page wherever its file is", () => {
    expect(pageSection(page({ storage_path: "Work/plan.md" }))).toBe("docs");
    expect(pageSection(page({ storage_path: "plan.md" }))).toBe("docs");
    expect(pageSection(page({}))).toBe("docs");
    expect(pageSection(page({ storage_path: "Rules/git.md" }))).toBe("rules");
    expect(pageSection(page({ storage_path: "Skills/release-notes.md" }))).toBe("skills");
    expect(pageSection(page({ storage_path: "Work/how.md", title: "How I work — Wenlan" }))).toBe("rules");
    expect(pageSection(page({ status: "draft", folder_path: "規則" }))).toBe("rules");
  });

  it("finds the section's folder for a new page, nearest the top first", () => {
    expect(sectionFolderPath("rules", ["Work", "Work/Rules", "規則"])).toBe("規則");
    expect(sectionFolderPath("skills", ["Work", "Rules"])).toBeNull();
    expect(sectionFolderPath("skills", ["a/Skills"])).toBe("a/Skills");
  });
});
