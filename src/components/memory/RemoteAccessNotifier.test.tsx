// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, cleanup, waitFor, act, configure } from "@testing-library/react";
import { i18n } from "../../i18n";
import RemoteAccessNotifier, { NOTICE_MESSAGE_KEY } from "./RemoteAccessNotifier";

const stop = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ listen }));
const notification = vi.hoisted(() => ({
  isPermissionGranted: vi.fn(), requestPermission: vi.fn(), sendNotification: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-notification", () => notification);
const takeRemoteAccessNotice = vi.hoisted(() => vi.fn());
vi.mock("../../lib/tauri", () => ({ takeRemoteAccessNotice }));

const WORDS: Record<string, Record<string, string>> = {
  en: { stopped: "Wenlan Web access stopped. Open Wenlan to reconnect.", expired: "Wenlan Web access ended. Open Wenlan to turn it on again." },
  "zh-Hans": { stopped: "文澜网页访问已停止。打开文澜以重新连接。", expired: "文澜网页访问已结束。打开文澜重新开启。" },
  "zh-Hant": { stopped: "文瀾網頁存取已停止。開啟文瀾以重新連接。", expired: "文瀾網頁存取已結束。開啟文瀾重新開啟。" },
};
function wakeUp() {
  return (listen.mock.calls[0][1] as () => void);
}

// jsdom accessible-name queries are slow on a busy machine; the first test of a
// file also pays the cold start. 4s still fails ahead of the 5s test timeout.
configure({ asyncUtilTimeout: 4_000 });

beforeEach(() => {
  vi.resetAllMocks();
  listen.mockImplementation(() => Promise.resolve(stop));
  takeRemoteAccessNotice.mockResolvedValue(null);
  notification.isPermissionGranted.mockResolvedValue(true);
  notification.requestPermission.mockResolvedValue("granted");
  notification.sendNotification.mockResolvedValue(undefined);
});
afterEach(async () => { cleanup(); await i18n.changeLanguage("en"); });

describe("RemoteAccessNotifier", () => {
  it("has one localized sentence for every kind the native side can send", () => {
    expect(Object.keys(NOTICE_MESSAGE_KEY).sort()).toEqual(["expired", "stopped"]);
  });

  it.each(Object.keys(WORDS))("sends each kind as a %s notification when woken by the event", async (locale) => {
    await i18n.changeLanguage(locale);
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(listen).toHaveBeenCalledWith("remote-access-notice", expect.any(Function)));
    for (const kind of ["stopped", "expired"] as const) {
      notification.sendNotification.mockClear();
      takeRemoteAccessNotice.mockResolvedValueOnce({ kind });
      await act(async () => { wakeUp()(); });
      await waitFor(() => expect(notification.sendNotification).toHaveBeenCalledWith({ title: "Wenlan", body: WORDS[locale][kind] }));
    }
  });

  it("shows a notice that was left before the window was listening", async () => {
    takeRemoteAccessNotice.mockResolvedValueOnce({ kind: "expired" });
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(notification.sendNotification).toHaveBeenCalledWith({ title: "Wenlan", body: WORDS.en.expired }));
    expect(notification.sendNotification).toHaveBeenCalledTimes(1);
  });

  it("says nothing when there is no notice", async () => {
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(takeRemoteAccessNotice).toHaveBeenCalled());
    await act(async () => { wakeUp()(); });
    expect(notification.sendNotification).not.toHaveBeenCalled();
  });

  it("ignores a kind it does not know", async () => {
    takeRemoteAccessNotice.mockResolvedValueOnce({ kind: "from-the-future" });
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(takeRemoteAccessNotice).toHaveBeenCalled());
    await Promise.resolve();
    expect(notification.sendNotification).not.toHaveBeenCalled();
  });

  it("uses the language the app is in when the notice arrives, not when it mounted", async () => {
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(listen).toHaveBeenCalled());
    await act(async () => { await i18n.changeLanguage("zh-Hant"); });
    takeRemoteAccessNotice.mockResolvedValueOnce({ kind: "stopped" });
    await act(async () => { wakeUp()(); });
    await waitFor(() => expect(notification.sendNotification).toHaveBeenCalledWith({ title: "Wenlan", body: WORDS["zh-Hant"].stopped }));
  });

  it("asks for permission once, and respects a no", async () => {
    notification.isPermissionGranted.mockResolvedValue(false);
    notification.requestPermission.mockResolvedValue("denied");
    takeRemoteAccessNotice.mockResolvedValueOnce({ kind: "stopped" });
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(notification.requestPermission).toHaveBeenCalledTimes(1));
    expect(notification.sendNotification).not.toHaveBeenCalled();
  });

  it("does not ask again when notifications are already allowed", async () => {
    takeRemoteAccessNotice.mockResolvedValueOnce({ kind: "expired" });
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(notification.sendNotification).toHaveBeenCalledTimes(1));
    expect(notification.requestPermission).not.toHaveBeenCalled();
  });

  it("survives a notification plugin that throws", async () => {
    notification.sendNotification.mockRejectedValue(new Error("no permission"));
    takeRemoteAccessNotice.mockResolvedValueOnce({ kind: "stopped" });
    render(<RemoteAccessNotifier />);
    await waitFor(() => expect(notification.sendNotification).toHaveBeenCalled());
  });

  it("stops listening when it unmounts", async () => {
    const view = render(<RemoteAccessNotifier />);
    await waitFor(() => expect(listen).toHaveBeenCalled());
    await Promise.resolve();
    view.unmount();
    expect(stop).toHaveBeenCalledTimes(1);
  });
});
