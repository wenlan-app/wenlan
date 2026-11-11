// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import SourcesSection from "./SourcesSection";

vi.mock("../../sources/SourcesSection", () => ({ default: () => <div data-testid="sources-list" /> }));

// Past AI chats and memories come in through the one "Add your material"
// dialog now, so this section keeps only the sources and the library.
describe("SourcesSection", () => {
  it("lists sources and opens the library, with no separate import blocks", () => {
    const onOpenLibrary = vi.fn();
    render(<SourcesSection onOpenLibrary={onOpenLibrary} />);
    expect(screen.getByTestId("sources-list")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Paste or import" })).toBeNull();
    expect(screen.queryByText("How to export")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open list" }));
    expect(onOpenLibrary).toHaveBeenCalled();
  });
});
