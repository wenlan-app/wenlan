// SPDX-License-Identifier: AGPL-3.0-only
import { createSpacesNavigationFixture, type SpacesNavigationFixture } from "./spacesNavigation";

export const SPACE_PROJECT_SOURCE_FOLDER_ID = "directory-reading-club-preview";
export const SPACE_PROJECT_SOURCE_FOLDER_PATH = "/preview/sources/讀書會資料";

export function createSpaceProjectFixture(empty = false): SpacesNavigationFixture {
  const fixture = createSpacesNavigationFixture();
  const wenlan = { ...fixture.spaces[0], id: "project-wenlan", name: "Wenlan", description: "Pages and references for a simpler writing experience." };
  const reading = { ...wenlan, id: "project-reading", name: "讀書會", description: "一起閱讀，留下自己的理解。" };
  const sourceFolderPath = SPACE_PROJECT_SOURCE_FOLDER_PATH;
  return { ...fixture, spaces: [wenlan, reading], pages: fixture.pages.slice(0, 2).map(page => ({ ...page, domain: "Wenlan", space: "Wenlan" })), documents: empty ? [] : [
    { file: { source: "file", source_id: "design.md", title: "Writing experience observations", chunk_count: 1, last_modified: 1, domain: "Wenlan", space: "Wenlan" }, content: "# Writing experience observations\n\nStart with the page itself. Keep references close when the reader needs them." },
    { file: { source: "webpage", source_id: "https://example.com/reading", title: "A reference for reading", chunk_count: 1, last_modified: 1, domain: "Wenlan", space: "Wenlan" }, content: "Saved reference text for the Wenlan project. This is isolated fixture content." },
    { file: { source: "file", source_id: "design.md", title: "讀書會摘錄", chunk_count: 1, last_modified: 1, domain: "讀書會", space: "讀書會" }, content: "這份文件只屬於讀書會，用來驗證來源讀取不會跨越空間。" },
    { file: { source: "memory", source_id: `${SPACE_PROJECT_SOURCE_FOLDER_ID}::${sourceFolderPath}/閱讀札記.txt`, title: "讀書會・閱讀札記（示範）", source_agent: "folder", chunk_count: 1, last_modified: 1, domain: "讀書會", space: "讀書會" }, content: "讀書會・閱讀札記\n\n這是虛構的文字檔範例。每次討論後，記下最有啟發的一句話，以及它讓我們想到的問題。\n\n本週摘錄：先記錄自己的理解，再回頭核對原文。" },
    { file: { source: "memory", source_id: `${SPACE_PROJECT_SOURCE_FOLDER_ID}::${sourceFolderPath}/每週摘錄.md`, title: "每週摘錄", source_agent: "folder", chunk_count: 1, last_modified: 1, domain: "讀書會", space: "讀書會" }, content: "# 每週摘錄\n\n## 本週問題\n哪一段內容改變了我們原先的想法？\n\n## 討論後記\n保留不同觀點，並連回各自引用的段落。" },
    { file: { source: "webpage", source_id: "https://example.org/reading-club/slow-reading", url: "https://example.org/reading-club/slow-reading", title: "慢讀與討論｜讀書會參考文章（示範）", chunk_count: 1, last_modified: 1, domain: "example.org", space: "讀書會" }, content: "# 慢讀與討論\n\n這是使用 example.org 網址的虛構離線範例，沒有連線或擷取網站內容。\n\n閱讀時先提出問題，再整理支持與反對的段落，最後記下仍待討論的地方。" },
  ] };
}
