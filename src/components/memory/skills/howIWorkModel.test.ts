// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { Page } from "../../../lib/tauri";
import { isHowIWorkPage } from "./howIWorkModel";

describe("how-I-work page detection", () => {
  it("treats a list row without a body as an ordinary page", () => {
    const row = { title: "Reading list" } as unknown as Pick<Page, "content" | "title">;
    expect(isHowIWorkPage(row)).toBe(false);
  });
});
