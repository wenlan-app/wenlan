// SPDX-License-Identifier: AGPL-3.0-only
import { act, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const emitMock = vi.hoisted(() => vi.fn());
const listenMock = vi.hoisted(() => vi.fn());
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>());

vi.mock("@tauri-apps/api/event", () => ({
  emit: emitMock,
  listen: listenMock,
}));

import UpdateWatcher from "./UpdateWatcher";
import { getServiceNotices, setUpdateNotice } from "../lib/serviceNotices";

describe("UpdateWatcher", () => {
  beforeEach(() => {
    emitMock.mockReset().mockResolvedValue(undefined);
    listeners.clear();
    listenMock.mockReset().mockImplementation((event, callback) => {
      listeners.set(event, callback);
      return Promise.resolve(() => listeners.delete(event));
    });
  });
  afterEach(() => {
    act(() => setUpdateNotice(null));
  });

  it("announces that it is listening after mounting, and draws nothing", async () => {
    const { container } = render(<UpdateWatcher />);

    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://ui-ready"));
    expect(container).toBeEmptyDOMElement();
  });

  it("hands an offered version to the sidebar's service row", async () => {
    render(<UpdateWatcher />);
    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://ui-ready"));

    act(() => {
      listeners.get("updater://available")?.({ payload: { version: "0.20.1" } });
    });
    expect(getServiceNotices().update).toEqual({ phase: "available", version: "0.20.1", pct: null });

    act(() => {
      listeners.get("updater://status")?.({ payload: { state: "current" } });
    });
    expect(getServiceNotices().update).toBeNull();
  });
});
