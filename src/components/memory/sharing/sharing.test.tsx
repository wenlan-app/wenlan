// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { i18n } from "../../../i18n";
import type { PageChangelogEntry, SharedStatus } from "../../../lib/tauri";
import { historyItemLabel } from "../page/historyItems";
import { ConcurrentEditLine, markConcurrentEditSeen, unseenConcurrentEdit } from "./ConcurrentEditLine";
import { cloudName, revealFolderLabel, shortFolder } from "./sharing";

const t = i18n.t.bind(i18n);

const shares: SharedStatus[] = [{
  kind: "space",
  target: "Wedding",
  folder: "/Users/me/Google Drive/Wedding",
  member_id: "me",
  member_name: "Qi-Xuan",
  members: [{ id: "me", name: "Qi-Xuan" }, { id: "yl", name: "Yiling" }],
}];

beforeEach(async () => {
  await i18n.changeLanguage("en");
  localStorage.clear();
});
afterEach(() => vi.restoreAllMocks());

describe("revealFolderLabel", () => {
  it("names each platform's own file manager", () => {
    expect(revealFolderLabel(t, "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("Show in Finder");
    expect(revealFolderLabel(t, "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("Show in File Explorer");
    expect(revealFolderLabel(t, "Mozilla/5.0 (X11; Linux x86_64)")).toBe("Open folder");
    expect(revealFolderLabel(t, "")).toBe("Open folder");
  });
});

describe("cloudName", () => {
  it("knows the cloud folders of macOS and Windows", () => {
    expect(cloudName("/Users/me/Library/CloudStorage/GoogleDrive-me@example.com/My Drive/Wedding", t)).toBe("Google Drive");
    expect(cloudName("G:\\My Drive\\Wedding", t)).toBe("Google Drive");
    expect(cloudName("H:\\我的雲端硬碟\\婚禮", t)).toBe("Google Drive");
    expect(cloudName("G:\\Shared drives\\Team\\Wedding", t)).toBe("Google Drive");
    expect(cloudName("/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Wedding", t)).toBe("iCloud Drive");
    expect(cloudName("C:\\Users\\me\\iCloudDrive\\Wedding", t)).toBe("iCloud Drive");
    expect(cloudName("C:\\Users\\me\\OneDrive\\Wedding", t)).toBe("OneDrive");
    expect(cloudName("C:\\Users\\me\\Dropbox\\Wedding", t)).toBe("Dropbox");
  });

  it("does not take an ordinary folder for a cloud one", () => {
    expect(cloudName("C:\\Users\\me\\Documents\\Wedding", t)).toBeNull();
    expect(cloudName("D:\\Backup\\My Drive\\Wedding", t)).toBeNull();
    expect(cloudName("/Users/me/Documents/Wedding", t)).toBeNull();
  });

  it("shows a Windows Google Drive folder by the cloud's name", () => {
    expect(shortFolder("G:\\My Drive\\Wenlan\\Wedding", t)).toBe("Google Drive › Wenlan › Wedding");
  });
});

describe("historyItemLabel", () => {
  it("names the member behind a change taken from a shared folder", () => {
    const item = { kind: "person_edit" as const, at: 1, version: 4, edited_by: "shared_person:yl" };
    expect(historyItemLabel(t, item, shares)).toBe("Yiling");
    expect(historyItemLabel(t, { ...item, edited_by: "shared_ai:yl" }, shares)).toBe("Yiling's AI tool");
    // Without the shares it falls back to the plain writer label.
    expect(historyItemLabel(t, item)).toBe(t("revisionLabel.writer.other"));
  });
});

describe("concurrent edit line", () => {
  const entries: PageChangelogEntry[] = [
    { version: 6, at: 10, edited_by: "manual_edit" },
    { version: 7, at: 20, edited_by: "shared_person:yl", concurrent_with: 6 },
  ];

  it("shows for the current version until the person closes it", () => {
    expect(unseenConcurrentEdit("page-1", 7, entries)?.concurrent_with).toBe(6);
    expect(unseenConcurrentEdit("page-1", 8, entries)).toBeUndefined();
    expect(unseenConcurrentEdit("page-1", 6, entries)).toBeUndefined();
    markConcurrentEditSeen("page-1", 7);
    expect(unseenConcurrentEdit("page-1", 7, entries)).toBeUndefined();
    expect(unseenConcurrentEdit("page-2", 7, entries)).toBeDefined();
  });

  it("says whose version is shown and where the person's is", async () => {
    const onSeeMine = vi.fn();
    const onDismiss = vi.fn();
    render(<ConcurrentEditLine name="Yiling" onSeeMine={onSeeMine} onDismiss={onDismiss} />);
    expect(screen.getByRole("status")).toHaveTextContent(
      "You and Yiling edited this page at the same time. Yiling's version is shown; yours is in the revision history.",
    );
    await userEvent.click(screen.getByRole("button", { name: "See mine" }));
    await userEvent.click(screen.getByRole("button", { name: "OK" }));
    expect(onSeeMine).toHaveBeenCalledOnce();
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
