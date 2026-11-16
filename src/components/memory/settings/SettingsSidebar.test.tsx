// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import SettingsSidebar from "./SettingsSidebar";

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(() => new Promise(() => {})),
}));

vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn(() => Promise.resolve()) }));

function renderSettingsSidebar(extraProps: Partial<React.ComponentProps<typeof SettingsSidebar>> = {}) {
  return render(
    <SettingsSidebar
      hidden={false}
      mode="labels"
      active="general"
      onSelect={() => {}}
      {...extraProps}
    />,
  );
}

describe("SettingsSidebar", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("shows only browser-safe groups in the web build", () => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    renderSettingsSidebar();

    expect(screen.getByRole("button", { name: "General" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Appearance" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connections" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Diagnostics" })).not.toBeInTheDocument();
  });

  it("starts with Back to home and aligned General navigation without a Settings caption", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    const onNavigateHome = vi.fn();
    renderSettingsSidebar({ onSelect, onNavigateHome });

    const home = screen.getByRole("button", { name: "Back to home" });
    const general = screen.getByRole("button", { name: "General" });
    expect(screen.queryByText("Settings")).not.toBeInTheDocument();
    expect(home.parentElement).toHaveClass("notes-sidebar-search");
    expect(general).toHaveClass("notes-primary-link", "settings-sidebar-group");
    expect(general).toHaveAttribute("data-active", "true");
    expect(general.closest("nav")).toHaveClass("notes-rail-nav", "notes-rail-nav--labels");
    expect(home).not.toHaveAttribute("aria-expanded");
    expect(screen.queryByRole("heading", { name: "Wenlan" })).toBeNull();

    await user.click(home);

    expect(onNavigateHome).toHaveBeenCalledOnce();

    await user.click(general);

    expect(onSelect).toHaveBeenCalledWith("general");
  });

  it("keeps the policy link in its own block above the Wenlan footer", () => {
    renderSettingsSidebar();

    const general = screen.getByRole("button", { name: "General" });
    const privacy = screen.getByRole("link", { name: "Privacy policy" });
    const brand = screen.getByText("Wenlan");
    const policy = privacy.closest(".settings-sidebar-policy");
    const footer = brand.closest(".settings-sidebar-footer");
    expect(screen.queryByRole("button", { name: "Wenlan" })).toBeNull();

    expect(privacy).toHaveAttribute("href", "https://github.com/wenlan-app/wenlan/blob/main/docs/PRIVACY.md");
    expect(policy).not.toBeNull();
    expect(policy?.querySelector("svg")).toHaveAttribute("width", "13");
    expect(policy?.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(footer).not.toBeNull();
    expect(general.compareDocumentPosition(privacy) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(policy!.compareDocumentPosition(footer!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps collapsed desktop controls inert and out of the tab order", () => {
    const { container } = renderSettingsSidebar({ hidden: true });
    const sidebar = container.querySelector(".settings-sidebar");

    expect(sidebar).toHaveAttribute("aria-hidden", "true");
    expect(sidebar).toHaveAttribute("inert");
    expect(sidebar?.querySelector("button")).not.toHaveFocus();
  });

  it("uses the compact Back to home entry without a visible label or shortcut", async () => {
    const user = userEvent.setup();
    const onNavigateHome = vi.fn();
    const { container } = renderSettingsSidebar({ mode: "icons", onNavigateHome });
    const home = screen.getByRole("button", { name: "Back to home" });

    expect(home).toHaveClass("notes-search-entry", "notes-search-entry--compact");
    expect(home).toHaveAttribute("title", "Back to home");
    expect(home).not.toHaveAttribute("aria-expanded");
    expect(container.querySelector(".notes-search-entry-label")).toBeNull();
    expect(container.querySelector(".notes-search-entry-shortcut")).toBeNull();
    const privacy = screen.getByRole("link", { name: "Privacy policy" });
    expect(privacy).toHaveAttribute("title", "Privacy policy");
    expect(privacy.querySelector("svg")).toHaveAttribute("width", "22");
    expect(privacy.querySelector("svg")).toHaveAttribute("aria-hidden", "true");
    expect(privacy).not.toHaveTextContent("Privacy policy");

    await user.click(home);
    expect(onNavigateHome).toHaveBeenCalledOnce();
  });

  it("disables Back to home navigation when the caller disables navigation", async () => {
    const user = userEvent.setup();
    const onNavigateHome = vi.fn();
    renderSettingsSidebar({ navigationDisabled: true, onNavigateHome });
    const home = screen.getByRole("button", { name: "Back to home" });

    expect(home).toBeDisabled();
    await user.click(home);
    expect(onNavigateHome).not.toHaveBeenCalled();
  });

  it("focuses, traps, and closes the narrow overlay", async () => {
    const user = userEvent.setup();
    const onRequestClose = vi.fn();
    const onSelect = vi.fn();
    const onNavigateHome = vi.fn();
    const { container, rerender } = renderSettingsSidebar({
      hidden: true,
      active: "general",
      onSelect,
      open: false,
      presentation: "overlay",
      onRequestClose,
      onNavigateHome,
    });
    const sidebar = container.querySelector(".settings-sidebar");

    expect(sidebar).toHaveAttribute("aria-hidden", "true");
    expect(sidebar).toHaveAttribute("inert");

    rerender(
      <SettingsSidebar
        hidden={false}
        mode="labels"
        active="general"
        onSelect={onSelect}
        open
        presentation="overlay"
        onRequestClose={onRequestClose}
        onNavigateHome={onNavigateHome}
      />,
    );
    const first = screen.getByRole("button", { name: "Back to home" });
    const last = screen.getByRole("link", { name: "Privacy policy" });
    await waitFor(() => expect(first).toHaveFocus());

    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(first).toHaveFocus();
    fireEvent.keyDown(first, { key: "Tab", shiftKey: true });
    expect(last).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "Close sidebar" }));
    expect(onRequestClose).toHaveBeenCalledOnce();
  });
});
