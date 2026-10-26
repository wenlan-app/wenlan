// SPDX-License-Identifier: AGPL-3.0-only
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import ConnectionListDisclosure from "./ConnectionListDisclosure";

describe("ConnectionListDisclosure", () => {
  afterEach(async () => {
    await i18n.changeLanguage("en");
  });

  it.each([
    ["en", "Show 2 more", "Show less"],
    ["zh-Hans", "再显示 2 个", "收起"],
    ["zh-Hant", "再顯示 2 個", "收起"],
  ])("renders matching disclosure icons in %s", async (locale, moreLabel, lessLabel) => {
    await i18n.changeLanguage(locale);
    const onToggle = vi.fn();
    const { rerender } = render(
      <ConnectionListDisclosure expanded={false} hiddenCount={2} onToggle={onToggle} />,
    );

    const moreButton = screen.getByRole("button", { name: moreLabel });
    expect(moreButton.querySelector('svg[aria-hidden="true"][data-direction="down"]')).not.toBeNull();
    await userEvent.click(moreButton);
    expect(onToggle).toHaveBeenCalledOnce();

    rerender(<ConnectionListDisclosure expanded hiddenCount={0} onToggle={onToggle} />);
    const lessButton = screen.getByRole("button", { name: lessLabel });
    expect(lessButton.querySelector('svg[aria-hidden="true"][data-direction="up"]')).not.toBeNull();
  });
});
