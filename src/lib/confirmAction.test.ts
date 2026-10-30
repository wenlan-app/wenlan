// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, fireEvent, screen } from "@testing-library/react";
import fs from "node:fs";
import path from "node:path";
import { confirmAction, setConfirmPresenter } from "./confirmAction";

const labels = { confirm: "Delete page", cancel: "Cancel" };

async function ask(): Promise<{ answer: Promise<boolean> }> {
  let answer!: Promise<boolean>;
  await act(async () => { answer = confirmAction("Delete this page?", labels); });
  return { answer };
}

describe("confirmAction", () => {
  beforeEach(() => setConfirmPresenter());
  afterEach(() => setConfirmPresenter((message) => Promise.resolve(window.confirm(message))));

  it("asks in the app's own dialog with the action's button focused", async () => {
    const trigger = document.createElement("button");
    document.body.appendChild(trigger);
    trigger.focus();
    const { answer } = await ask();
    const dialog = await screen.findByRole("alertdialog", { name: "Delete this page?" });
    expect(screen.getByRole("button", { name: "Delete page" })).toHaveFocus();
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Delete page" })); });
    await expect(answer).resolves.toBe(true);
    expect(dialog).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    trigger.remove();
  });

  it("answers no to Cancel, Escape and the backdrop", async () => {
    let { answer } = await ask();
    await act(async () => { fireEvent.click(await screen.findByRole("button", { name: "Cancel" })); });
    await expect(answer).resolves.toBe(false);

    ({ answer } = await ask());
    await screen.findByRole("alertdialog");
    await act(async () => { fireEvent.keyDown(document, { key: "Escape" }); });
    await expect(answer).resolves.toBe(false);

    ({ answer } = await ask());
    const overlay = (await screen.findByRole("alertdialog")).parentElement!;
    await act(async () => { fireEvent.click(overlay); });
    await expect(answer).resolves.toBe(false);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("keeps the confirm button red unless the caller asks for the neutral tone", async () => {
    let { answer } = await ask();
    let confirm = await screen.findByRole("button", { name: "Delete page" });
    expect(confirm).toHaveAttribute("data-tone", "danger");
    expect(confirm.className).toContain("--mem-status-danger");
    await act(async () => { fireEvent.click(confirm); });
    await expect(answer).resolves.toBe(true);

    await act(async () => {
      answer = confirmAction("Archive this page?", { confirm: "Archive page", cancel: "Cancel", tone: "neutral" });
    });
    confirm = await screen.findByRole("button", { name: "Archive page" });
    expect(confirm).toHaveAttribute("data-tone", "neutral");
    expect(confirm.className).not.toContain("--mem-status-danger");
    expect(confirm.className).toContain("--mem-accent-indigo");
    expect(confirm).toHaveFocus();
    await act(async () => { fireEvent.click(confirm); });
    await expect(answer).resolves.toBe(true);
  });

  it("keeps one question open: a newer one answers the older with no", async () => {
    const { answer: first } = await ask();
    await screen.findByRole("alertdialog");
    const { answer: second } = await ask();
    await expect(first).resolves.toBe(false);
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Delete page" })); });
    await expect(second).resolves.toBe(true);
  });
});

function listSourceFiles(dir: string): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = path.join(dir, entry.name);
    if (entry.isDirectory()) return entry.name === "test" ? [] : listSourceFiles(fullPath);
    if (!/\.tsx?$/.test(entry.name) || /\.test\.tsx?$/.test(entry.name)) return [];
    return [fullPath];
  });
}

describe("app code", () => {
  it("never opens a browser dialog, which the macOS app answers with no", () => {
    const offenders = listSourceFiles(path.resolve("src")).flatMap((file) => {
      const text = fs.readFileSync(file, "utf8");
      const declaresConfirm = /\b(const|let|function)\s+confirm\b/.test(text);
      const calls: string[] = [...text.matchAll(/\bwindow\.(confirm|alert|prompt)\s*\(|(?<![.\w])(alert|prompt)\s*\(/g)].map(match => match[0]);
      if (!declaresConfirm && /(?<![.\w])confirm\s*\(/.test(text)) calls.push("confirm(");
      return calls.map(call => `${path.relative(process.cwd(), file)}: ${call}`);
    });
    expect(offenders).toEqual([]);
  });
});
