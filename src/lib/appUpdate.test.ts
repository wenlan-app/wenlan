// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/event", () => ({ emit: vi.fn().mockResolvedValue(undefined) }));
import { installUpdate, onUpdateAvailable, onUpdateStatus } from "./appUpdate";
import { getServiceNotices, setUpdateNotice } from "./serviceNotices";

beforeEach(() => setUpdateNotice(null));

it("keeps installing while adopting the freshly checked target", async () => {
  onUpdateAvailable("0.20.1");
  await installUpdate();
  onUpdateStatus({ state: "available", version: "0.24.0" });
  expect(getServiceNotices().update).toEqual({ phase: "installing", version: "0.24.0", pct: null });
});

it("recovers from install-time recheck failure or a withdrawn offer", async () => {
  onUpdateAvailable("0.20.1");
  await installUpdate();
  onUpdateStatus({ state: "error", error: "offline" });
  expect(getServiceNotices().update?.phase).toBe("failed");
  onUpdateStatus({ state: "current" });
  expect(getServiceNotices().update).toBeNull();
});
