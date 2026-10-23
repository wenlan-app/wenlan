// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi, beforeEach } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import IdentityCard from "../IdentityCard";
import { getServiceNotices, setUpdateNotice } from "../../../lib/serviceNotices";
import { onUpdateAvailable, onUpdateProgress, onUpdateStatus } from "../../../lib/appUpdate";

const emitMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/event", () => ({ emit: emitMock }));

vi.mock("../../../lib/tauri", () => ({
  getProfile: vi.fn(),
  listAgents: vi.fn().mockResolvedValue([]),
  listEntities: vi.fn(),
  getEntityDetail: vi.fn(),
}));

import * as tauri from "../../../lib/tauri";

function renderIdentityCard(compact = true) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenSettings = vi.fn();
  const onOpenAbout = vi.fn();
  return render(
    <QueryClientProvider client={qc}>
      <IdentityCard
        compact={compact}
        onOpenDetail={() => {}}
        onOpenSettings={onOpenSettings}
        onOpenAbout={onOpenAbout}
      />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  vi.clearAllMocks();
  await i18n.changeLanguage("en");
  vi.mocked(tauri.getProfile).mockResolvedValue({
    id: "p1",
    name: "Lucian",
    display_name: "Lucian",
    email: null,
    bio: null,
    avatar_path: "/missing/avatar.png",
    created_at: 0,
    updated_at: 0,
  } as any);
  vi.mocked(tauri.listEntities).mockResolvedValue([
    { id: "person-lucian", name: "Lucian", entity_type: "person" },
  ] as any);
  vi.mocked(tauri.getEntityDetail).mockResolvedValue({ observations: [] } as any);
});

describe("IdentityCard", () => {
  it("falls back to initials when the saved avatar cannot be loaded", async () => {
    renderIdentityCard();

    const avatar = await screen.findByRole("img", { name: "Lucian" });
    fireEvent.error(avatar);

    expect(screen.queryByRole("img", { name: "Lucian" })).not.toBeInTheDocument();
    expect(screen.getByText("L")).toBeInTheDocument();
  });

  it("renders an icon account entry with the name available on demand", async () => {
    vi.mocked(tauri.getEntityDetail).mockResolvedValue({
      observations: [{ content: "The user is a senior engineer working on Wenlan." }],
    } as any);

    renderIdentityCard();

    const trigger = await screen.findByRole("button", { name: /Lucian account menu/ });
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(tauri.getEntityDetail).not.toHaveBeenCalled();
    expect(trigger).toHaveAttribute("aria-haspopup", "menu");
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(trigger).toHaveAttribute("title", "Lucian");
    expect(screen.queryByText("Lucian")).not.toBeInTheDocument();
    expect(screen.queryByText(/senior engineer/i)).not.toBeInTheDocument();
    expect(screen.queryByText("Set up your profile")).not.toBeInTheDocument();
  });

  it("aligns the expanded account avatar inside the shared navigation icon slot", async () => {
    renderIdentityCard(false);

    const trigger = await screen.findByRole("button", { name: /Lucian account menu/ });
    const slot = trigger.querySelector(".identity-account-slot");
    const avatar = slot?.querySelector("img");

    expect(trigger).toHaveClass("notes-primary-link");
    expect(slot).toHaveClass("notes-navigation-glyph");
    expect(avatar).toHaveStyle({ width: "24px", height: "24px" });
    expect(trigger).toHaveTextContent("Lucian");
  });

  it("opens a minimal avatar menu without profile-specific or tool-connection shortcuts", async () => {
    const user = userEvent.setup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onOpenSettings = vi.fn();
    const onOpenAbout = vi.fn();

    render(
      <QueryClientProvider client={qc}>
        <IdentityCard
          onOpenDetail={() => {}}
          onOpenSettings={onOpenSettings}
          onOpenAbout={onOpenAbout}
        />
      </QueryClientProvider>,
    );

    await user.click(await screen.findByRole("button", { name: /Lucian account menu/ }));

    expect(screen.getByRole("menu")).toHaveClass("identity-rail-menu");
    expect(screen.queryByText("Lucian")).not.toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Settings" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "About Wenlan" })).toBeInTheDocument();
    expect(screen.queryByText("Profile settings")).not.toBeInTheDocument();
    expect(screen.queryByText("Connect tools")).not.toBeInTheDocument();
    expect(screen.queryByText("Local account")).not.toBeInTheDocument();

    await user.click(screen.getByRole("menuitem", { name: "Settings" }));
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
    expect(onOpenAbout).not.toHaveBeenCalled();
  });

  it("supports keyboard menu navigation and returns focus on Escape", async () => {
    const user = userEvent.setup();
    renderIdentityCard();
    const trigger = await screen.findByRole("button", { name: /Lucian account menu/ });
    await user.click(trigger);
    expect(screen.getByRole("menuitem", { name: "What your AI can do" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "Settings" })).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitem", { name: "About Wenlan" })).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it("opens what your AI can do from the menu", async () => {
    renderIdentityCard();
    fireEvent.click(await screen.findByRole("button", { name: /Lucian account menu/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "What your AI can do" }));
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "What your AI can do" })).toBeInTheDocument();
  });

  it("does not mistake the first person in the knowledge base for the user", async () => {
    vi.mocked(tauri.getProfile).mockResolvedValue(null);
    renderIdentityCard();
    expect(await screen.findByRole("button", { name: "Account menu" })).toBeInTheDocument();
    expect(screen.queryByText("Lucian")).not.toBeInTheDocument();
    expect(tauri.listEntities).not.toHaveBeenCalled();
  });

  it("localizes the empty account menu state", async () => {
    await i18n.changeLanguage("zh-Hant");
    vi.mocked(tauri.getProfile).mockResolvedValue(null);
    vi.mocked(tauri.listEntities).mockResolvedValue([]);

    renderIdentityCard();

    expect(await screen.findByRole("button", { name: "帳戶選單" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "帳戶選單" })).toHaveAttribute("title", "帳戶");
    expect(screen.queryByText("設定你的個人資料")).not.toBeInTheDocument();
    expect(screen.queryByText("Set up your profile")).not.toBeInTheDocument();
  });
});

describe("IdentityCard: a new version on offer", () => {
  beforeEach(() => {
    emitMock.mockReset().mockResolvedValue(undefined);
    setUpdateNotice({ phase: "available", version: "0.20.1", pct: null });
  });
  afterEach(() => {
    setUpdateNotice(null);
  });

  it("puts a dot on the account entry and names it", async () => {
    renderIdentityCard(false);
    expect(screen.getByTestId("identity-update-dot")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: /Update available/ })).toBeInTheDocument();
  });

  it("installs from the account menu", async () => {
    renderIdentityCard(false);
    fireEvent.click(screen.getByRole("button", { name: /account menu/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Update to 0.20.1" }));

    expect(emitMock).toHaveBeenCalledWith("updater://action", "install");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.queryByTestId("identity-update-dot")).not.toBeInTheDocument();
  });

  it("shows nothing extra without an offer", () => {
    setUpdateNotice(null);
    renderIdentityCard(false);
    expect(screen.queryByTestId("identity-update-dot")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /account menu/i }));
    expect(screen.queryByRole("menuitem", { name: /Update to/ })).not.toBeInTheDocument();
  });
});

describe("IdentityCard: an update under way", () => {
  beforeEach(() => {
    emitMock.mockReset().mockResolvedValue(undefined);
    act(() => onUpdateAvailable("0.20.1"));
  });
  afterEach(() => {
    act(() => setUpdateNotice(null));
  });

  async function install() {
    fireEvent.click(screen.getByRole("button", { name: /account menu/i }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Update to 0.20.1" }));
    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://action", "install"));
  }

  it("fills a ring on the account entry and says how far it got", async () => {
    renderIdentityCard(true);
    await install();

    expect(screen.getByTestId("identity-update-ring")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Updating…/ })).toBeInTheDocument();

    act(() => onUpdateProgress({ chunk: 0, total: 1000 }));
    act(() => onUpdateProgress({ chunk: 420 }));
    const trigger = screen.getByRole("button", { name: /Updating 42%/ });
    expect(screen.getByTestId("identity-update-ring").style.getPropertyValue("--identity-update-pct")).toBe("42%");

    fireEvent.click(trigger);
    expect(screen.getByTestId("identity-update-progress")).toHaveTextContent("Updating 42%");
    expect(screen.getByTestId("identity-update-progress")).toBeDisabled();
    // The quiet line is not where the menu puts focus.
    expect(screen.getByRole("menuitem", { name: "What your AI can do" })).toHaveFocus();
  });

  it("turns amber when the install fails, and tries again in one press", async () => {
    renderIdentityCard(true);
    await install();
    act(() => onUpdateProgress({ error: "network unavailable" }));

    expect(screen.getByTestId("identity-update-failed")).toBeInTheDocument();
    const trigger = screen.getByRole("button", { name: /Update failed/ });
    emitMock.mockClear();

    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole("menuitem", { name: "Try updating again" }));
    await waitFor(() => expect(emitMock).toHaveBeenCalledWith("updater://check-now"));
    expect(screen.getByTestId("identity-update-ring")).toBeInTheDocument();

    // The check offers the version again; "Try updating again" already said yes.
    act(() => onUpdateAvailable("0.20.1"));
    expect(emitMock).toHaveBeenCalledWith("updater://action", "install");
    expect(screen.getByTestId("identity-update-ring")).toBeInTheDocument();
  });

  it("ignores background check reports when nothing was offered", () => {
    act(() => setUpdateNotice(null));
    renderIdentityCard(true);
    act(() => onUpdateStatus({ state: "error", error: "offline" }));
    expect(getServiceNotices().update).toBeNull();
    expect(screen.queryByTestId("identity-update-failed")).not.toBeInTheDocument();
  });
});
