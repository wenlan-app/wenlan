// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import AboutWenlanDialog from "./AboutWenlanDialog";
const api = vi.hoisted(() => ({ emit: vi.fn(), listen: vi.fn(), open: vi.fn(), getVersion: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: api.listen, emit: api.emit }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: api.getVersion }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: api.open }));
describe("About Wenlan", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getVersion.mockResolvedValue("0.18.4");
    api.listen.mockResolvedValue(vi.fn());
    api.emit.mockResolvedValue(undefined);
  });
  it("shows the installed version and checks without closing the dialog", async () => {
    render(<AboutWenlanDialog open onClose={vi.fn()} />);
    expect(await screen.findByText("Version 0.18.4")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(api.emit).toHaveBeenCalledWith("updater://check-now");
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeDisabled();
    const handler = api.listen.mock.calls[0][1];
    act(() => handler({ payload: { state: "current" } }));
    expect(screen.getByRole("status")).toHaveTextContent("You’re up to date.");
    expect(document.querySelector(".about-wenlan-feedback")).toHaveTextContent("You’re up to date.");
    expect(screen.getByRole("button", { name: "Check for updates" })).toBeEnabled();
  });
  it("does not announce background or replayed current status as a manual confirmation", async () => {
    const { rerender } = render(<AboutWenlanDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(api.emit).toHaveBeenCalledWith("updater://ui-ready"));
    const handler = api.listen.mock.calls[0][1];
    act(() => handler({ payload: { state: "current" } }));
    expect(document.querySelector(".about-wenlan-feedback")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    rerender(<AboutWenlanDialog open={false} onClose={vi.fn()} />);
    act(() => handler({ payload: { state: "current" } }));
    rerender(<AboutWenlanDialog open onClose={vi.fn()} />);
    expect(document.querySelector(".about-wenlan-feedback")).toBeNull();
  });
  it.each(["error", "available"] as const)("does not report a %s result as current", async (state) => {
    render(<AboutWenlanDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(api.emit).toHaveBeenCalledWith("updater://ui-ready"));
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    const handler = api.listen.mock.calls[0][1];
    act(() => handler({ payload: { state, version: "0.19.0" } }));
    expect(document.querySelector(".about-wenlan-feedback")).toBeNull();
    expect(screen.getByRole("status")).toHaveTextContent(state === "error" ? "Couldn’t check for updates. Try again." : "Version 0.19.0 is available.");
    act(() => handler({ payload: { state: "current" } }));
    expect(document.querySelector(".about-wenlan-feedback")).toBeNull();
  });
  it("shows backend recovery details only for an error status", async () => {
    render(<AboutWenlanDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(api.emit).toHaveBeenCalledWith("updater://ui-ready"));
    const handler = api.listen.mock.calls[0][1];
    const error = "Previous app could not be restored; backup kept at /tmp/tauri-backup/Fixture.app";

    act(() => handler({ payload: { state: "error", error } }));
    const details = screen.getByText(error);
    expect(screen.getByRole("status")).toHaveTextContent("Couldn’t check for updates. Try again.");
    expect(details).toHaveClass("select-text", "break-words");

    act(() => handler({ payload: { state: "checking" } }));
    expect(screen.queryByText(error)).not.toBeInTheDocument();
    act(() => handler({ payload: { state: "current" } }));
    expect(screen.queryByText(error)).not.toBeInTheDocument();
  });
  it.each([false, true])("explains the isolated runtime guard, including after a manual request (%s)", async (manual) => {
    api.open.mockResolvedValue(undefined);
    render(<AboutWenlanDialog open onClose={vi.fn()} />);
    await waitFor(() => expect(api.emit).toHaveBeenCalledWith("updater://ui-ready"));
    if (manual) fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    const handler = api.listen.mock.calls[0][1];
    act(() => handler({ payload: { state: "error", error: "Update checks are disabled for development or custom data directories" } }));
    expect(screen.getByRole("status")).toHaveTextContent("Updates are disabled in this test session.");
    expect(screen.queryByRole("button", { name: "Check for updates" })).not.toBeInTheDocument();
    expect(document.querySelector(".about-wenlan-feedback")).toBeNull();
    api.emit.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "View releases" }));
    await waitFor(() => expect(api.open).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/releases"));
    expect(api.emit).not.toHaveBeenCalledWith("updater://check-now");
  });
  it("traps keyboard focus, closes with Escape and restores the trigger", async () => {
    const trigger = document.createElement("button"); document.body.append(trigger); trigger.focus();
    const onClose = vi.fn();
    const { unmount } = render(<AboutWenlanDialog open onClose={onClose} />);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveFocus();
    fireEvent.keyDown(dialog, {key:"Tab", shiftKey:true});
    expect(screen.getByRole("link", {name:"Report an issue"})).toHaveFocus();
    fireEvent.keyDown(document.activeElement!, {key:"Tab"});
    expect(screen.getByRole("button", {name:"Close"})).toHaveFocus();
    fireEvent.keyDown(dialog, {key:"Escape"}); expect(onClose).toHaveBeenCalledOnce();
    unmount(); expect(trigger).toHaveFocus(); trigger.remove();
  });
  it("makes check failures retryable and opens real support links", async () => {
    api.emit.mockImplementation((event: string) => event === "updater://check-now" ? Promise.reject(new Error("offline")) : Promise.resolve());
    api.open.mockResolvedValue(undefined);
    render(<AboutWenlanDialog open onClose={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", {name:"Check for updates"}));
    expect(await screen.findByText("Couldn’t check for updates. Try again.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link",{name:"Release notes"}));
    await waitFor(() => expect(api.open).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/releases"));
  });
});
