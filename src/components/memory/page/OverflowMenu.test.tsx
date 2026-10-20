// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { OverflowMenu } from "./OverflowMenu";

function setup(extra: Partial<React.ComponentProps<typeof OverflowMenu>> = {}) {
  const first = vi.fn();
  const second = vi.fn();
  render(<>
    <button type="button">outside</button>
    <OverflowMenu
      label="More actions"
      items={[
        { key: "one", label: "First", hint: "Quiet line", onSelect: first },
        { key: "two", label: "Second", onSelect: second },
        { key: "three", label: "Third", disabled: true, onSelect: vi.fn() },
      ]}
      {...extra}
    />
  </>);
  return { first, second };
}

afterEach(cleanup);

describe("OverflowMenu", () => {
  it("opens on the trigger, focuses the first enabled item and runs a choice once", () => {
    const { first } = setup();
    const trigger = screen.getByRole("button", { name: "More actions" });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("menu")).toBeNull();

    fireEvent.click(trigger);
    expect(trigger).toHaveAttribute("aria-expanded", "true");
    expect(screen.getByRole("menuitem", { name: /First/ })).toHaveFocus();
    expect(screen.getByRole("menuitem", { name: /First/ })).toHaveTextContent("Quiet line");

    fireEvent.click(screen.getByRole("menuitem", { name: /First/ }));
    expect(first).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("moves with the arrow keys over enabled items only, wrapping around", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    const menu = screen.getByRole("menu");
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: "Second" })).toHaveFocus();
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(screen.getByRole("menuitem", { name: /First/ })).toHaveFocus();
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(screen.getByRole("menuitem", { name: "Second" })).toHaveFocus();
  });

  it("closes on Escape and returns focus to the trigger", () => {
    setup();
    const trigger = screen.getByRole("button", { name: "More actions" });
    fireEvent.click(trigger);
    fireEvent.keyDown(document.activeElement as HTMLElement, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it("closes when the pointer goes elsewhere", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    fireEvent.pointerDown(screen.getByRole("button", { name: "outside" }));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("does not run a disabled item", () => {
    const { first, second } = setup();
    fireEvent.click(screen.getByRole("button", { name: "More actions" }));
    expect(screen.getByRole("menuitem", { name: "Third" })).toBeDisabled();
    expect(first).not.toHaveBeenCalled();
    expect(second).not.toHaveBeenCalled();
  });

  it("renders nothing without items and honours a disabled trigger", () => {
    const { unmount } = render(<OverflowMenu label="More actions" items={[]} />);
    expect(screen.queryByRole("button", { name: "More actions" })).toBeNull();
    unmount();

    setup({ disabled: true });
    expect(screen.getByRole("button", { name: "More actions" })).toBeDisabled();
  });
});
