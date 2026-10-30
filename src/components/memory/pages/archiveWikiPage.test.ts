// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { setConfirmPresenter, type ConfirmActionLabels } from "../../../lib/confirmAction";

const tauriMocks = vi.hoisted(() => ({ archivePage: vi.fn() }));
vi.mock("../../../lib/tauri", () => tauriMocks);

import { archivePageFromList } from "./archiveWikiPage";

const t = ((key: string) => key) as unknown as TFunction;
const page = { id: "page-1", status: "active", creation_kind: null } as const;

afterEach(() => {
  setConfirmPresenter((message) => Promise.resolve(window.confirm(message)));
  tauriMocks.archivePage.mockReset();
});

describe("archivePageFromList", () => {
  it("asks with the neutral tone, since an archived page can be restored", async () => {
    const asked: ConfirmActionLabels[] = [];
    setConfirmPresenter(async (_message, labels) => { asked.push(labels); return true; });
    tauriMocks.archivePage.mockResolvedValue(undefined);
    const onArchived = vi.fn();
    await expect(archivePageFromList(new QueryClient(), page, { t, onArchived })).resolves.toBe(true);
    expect(asked).toEqual([{ confirm: "pageDetail.archivePage", cancel: "common.cancel", tone: "neutral" }]);
    expect(tauriMocks.archivePage).toHaveBeenCalledWith("page-1");
    expect(onArchived).toHaveBeenCalledWith("page-1");
  });
});
