// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { finishRenameLinks, renameLinksMessage } from "./renameLinksNotice";
import { clearRenamedLinks, renameRewriteExplaining } from "./renameLinkRewrite";

const toast = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast }));

const renamed = { id: "page-a", title: "New", version: 4 };

describe("rename link notice", () => {
  beforeEach(async () => {
    toast.mockReset();
    clearRenamedLinks();
    await i18n.changeLanguage("en");
  });

  it("says nothing when no other page linked to the old title", () => {
    expect(renameLinksMessage(i18n.t, renamed)).toBeNull();
    expect(renameLinksMessage(i18n.t, { ...renamed, links_updated: 0 })).toBeNull();
  });

  it("names every outcome in one line, in pages", () => {
    expect(renameLinksMessage(i18n.t, { ...renamed, links_updated: 1 })).toBe("Updated links in 1 page.");
    expect(
      renameLinksMessage(i18n.t, { ...renamed, links_updated: 3, links_pending_review: 1, links_not_updated: 2 }),
    ).toBe(
      "Updated links in 3 pages. 1 page asks you first, so its link update is waiting for your review. 2 pages still link to the old title.",
    );
  });

  it.each([
    ["zh-Hant", "已更新 2 篇筆記中的連結。 1 篇筆記仍連到舊標題。"],
    ["zh-Hans", "已更新 2 篇笔记中的链接。 1 篇笔记仍链接到旧标题。"],
  ])("reads naturally in %s", async (language, expected) => {
    await i18n.changeLanguage(language);
    expect(renameLinksMessage(i18n.t, { ...renamed, links_updated: 2, links_not_updated: 1 })).toBe(expected);
  });

  it("refreshes other open pages and shows the toast when links changed", () => {
    const client = new QueryClient();
    client.setQueryData(["page", "page-b"], { id: "page-b", version: 3 });
    client.setQueryData(["page-revisions", "page-b"], []);
    finishRenameLinks(i18n.t, client, { ...renamed, links_updated: 1 }, "Old");
    expect(client.getQueryState(["page", "page-b"])?.isInvalidated).toBe(true);
    // Recorded before the refresh, so an open editor can explain the change.
    expect(renameRewriteExplaining("See [[Old]].", "See [[New]].")).not.toBeNull();
    expect(client.getQueryState(["page-revisions", "page-b"])?.isInvalidated).toBe(true);
    expect(toast).toHaveBeenCalledWith("Updated links in 1 page.");
  });

  it("leaves other pages alone when no link changed", () => {
    const client = new QueryClient();
    client.setQueryData(["page", "page-b"], { id: "page-b", version: 3 });
    finishRenameLinks(i18n.t, client, renamed, "Old");
    expect(client.getQueryState(["page", "page-b"])?.isInvalidated).toBe(false);
    expect(renameRewriteExplaining("See [[Old]].", "See [[New]].")).toBeNull();
    expect(toast).not.toHaveBeenCalled();
  });
});
