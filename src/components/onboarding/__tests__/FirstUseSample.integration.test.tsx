// SPDX-License-Identifier: AGPL-3.0-only
import { StrictMode } from "react";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { FirstUseSample } from "../FirstUseSample";
import { i18n } from "../../../i18n";
import { clipboardWrite } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", () => ({ clipboardWrite: vi.fn() }));

beforeEach(async () => {
  vi.mocked(clipboardWrite).mockReset().mockResolvedValue(undefined);
  await i18n.changeLanguage("en");
});

function renderInApp() {
  return render(<StrictMode>
    <button type="button">App navigation</button>
    <FirstUseSample onBack={vi.fn()} onFinish={vi.fn()} />
  </StrictMode>);
}

it("reports a successful copy in the app's StrictMode lifecycle", async () => {
  const user = userEvent.setup();
  renderInApp();
  await user.click(screen.getByRole("button", { name: "Skip to result" }));
  await user.click(screen.getByRole("button", { name: "Commands for later" }));
  await user.click(screen.getAllByRole("button", { name: "Copy command" })[0]);
  expect(clipboardWrite).toHaveBeenCalledOnce();
  expect(await screen.findByText("Copied")).toBeVisible();
});

it("keeps Shift+Tab inside the modal immediately after opening", async () => {
  const user = userEvent.setup();
  renderInApp();
  await user.click(screen.getByRole("button", { name: "Skip to result" }));
  await user.click(screen.getByTestId("sample-citation-sample-cite-2"));
  const dialog = screen.getByRole("dialog");
  expect(dialog.contains(document.activeElement)).toBe(true);
  await user.tab({ shift: true });
  expect(dialog.contains(document.activeElement)).toBe(true);
  await act(async () => { await user.keyboard("{Escape}"); });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(screen.getByTestId("sample-citation-sample-cite-2")).toHaveFocus();
});
