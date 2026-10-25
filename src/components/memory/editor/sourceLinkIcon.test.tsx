// SPDX-License-Identifier: AGPL-3.0-only
import { act } from "react";
import { describe, expect, it, vi } from "vitest";
import { sourceLinkCardMount, sourceLinkIconMount, type SourceLinkIconActions } from "./sourceLinkIcon";

const URL = "https://x.com/simonw/status/1849";

describe("a saved link's icon", () => {
  it("says it is saved and opens its source", async () => {
    const open = vi.fn();
    const container = document.createElement("span");
    document.body.append(container);
    const mount = sourceLinkIconMount(() => ({ label: "Saved in Sources", open }));
    await act(async () => { mount(container, URL); });
    const button = container.querySelector("button")!;
    expect(button.title).toBe("Saved in Sources");
    expect(button.getAttribute("aria-label")).toBe("Saved in Sources");
    const down = new MouseEvent("mousedown", { bubbles: true, cancelable: true });
    button.dispatchEvent(down);
    expect(down.defaultPrevented).toBe(true);
    await act(async () => { button.click(); });
    expect(open).toHaveBeenCalledWith(URL);
  });

  it("is only a picture without anything to open", async () => {
    const container = document.createElement("span");
    await act(async () => { sourceLinkIconMount()(container, URL); });
    expect(container.querySelector("button")).toBeNull();
    expect(container.firstElementChild?.getAttribute("aria-hidden")).toBe("true");
  });
});

const WEB = "https://www.example.com/local-first";
const menu = { showLink: "Show as link", showPreview: "Show as preview", openSource: "Open source", more: "More for this link" };

function actionsWith(open: (url: string) => void, excerpt: string | null): () => SourceLinkIconActions {
  const actions = { label: "Saved in Sources", open, menu, excerpt: vi.fn(async () => excerpt) };
  return () => actions;
}

const item = (name: string) => [...document.querySelectorAll<HTMLButtonElement>(".source-link-menu button")].find((b) => b.textContent === name);

describe("a saved link's menu", () => {
  it("offers a card for a link alone on its line with words to show", async () => {
    const open = vi.fn();
    const setPreview = vi.fn();
    const container = document.createElement("span");
    document.body.append(container);
    await act(async () => { sourceLinkIconMount(actionsWith(open, "You own your data"))(container, WEB, { canPreview: true, preview: false, setPreview }); });
    await act(async () => { container.querySelector("button")!.click(); });
    expect(open).not.toHaveBeenCalled();
    expect(item("Show as link")?.getAttribute("aria-checked")).toBe("true");
    await act(async () => { item("Show as preview")!.click(); });
    expect(setPreview).toHaveBeenCalledWith(true);
    expect(document.querySelector(".source-link-menu")).toBeNull();
    await act(async () => { container.querySelector("button")!.click(); });
    await act(async () => { item("Open source")!.click(); });
    expect(open).toHaveBeenCalledWith(WEB);
    container.remove();
  });

  it("just opens the source when there is no card to offer", async () => {
    const open = vi.fn();
    const container = document.createElement("span");
    document.body.append(container);
    await act(async () => { sourceLinkIconMount(actionsWith(open, null))(container, `${WEB}/b`, { canPreview: true, preview: false, setPreview: vi.fn() }); });
    await act(async () => { container.querySelector("button")!.click(); });
    expect(document.querySelector(".source-link-menu")).toBeNull();
    expect(open).toHaveBeenCalledWith(`${WEB}/b`);
    container.remove();
  });
});

describe("a saved link's card", () => {
  it("shows the title, site and first words, opens the source and turns back into a link", async () => {
    const open = vi.fn();
    const setPreview = vi.fn();
    const container = document.createElement("span");
    document.body.append(container);
    await act(async () => { sourceLinkCardMount(actionsWith(open, "You own your data, in spite of the cloud"))(container, `${WEB}/card`, "Local-first software", { canPreview: true, preview: true, setPreview }); });
    expect(container.querySelector(".source-link-card__title")?.textContent).toBe("Local-first software");
    expect(container.querySelector(".source-link-card__site")?.textContent).toBe("example.com");
    expect(container.querySelector(".source-link-card__excerpt")?.textContent).toBe("You own your data, in spite of the cloud");
    await act(async () => { container.querySelector<HTMLButtonElement>(".source-link-card__body")!.click(); });
    expect(open).toHaveBeenCalledWith(`${WEB}/card`);
    await act(async () => { container.querySelector<HTMLButtonElement>(".source-link-card__more")!.click(); });
    expect(item("Show as preview")?.getAttribute("aria-checked")).toBe("true");
    await act(async () => { item("Show as link")!.click(); });
    expect(setPreview).toHaveBeenCalledWith(false);
    container.remove();
  });

  it("says a short post once when its words are its title", async () => {
    const container = document.createElement("span");
    document.body.append(container);
    await act(async () => { sourceLinkCardMount(actionsWith(vi.fn(), "@jack: just setting up my twttr"))(container, "https://x.com/jack/status/20", "@jack: just setting up my twttr", { canPreview: true, preview: true, setPreview: vi.fn() }); });
    expect(container.querySelector(".source-link-card__title")?.textContent).toBe("@jack: just setting up my twttr");
    expect(container.querySelector(".source-link-card__excerpt")).toBeNull();
    container.remove();
    // A long post titled with its words cut short is not said twice either.
    const long = document.createElement("span");
    document.body.append(long);
    await act(async () => { sourceLinkCardMount(actionsWith(vi.fn(), "@zuck: We're building personal superintelligence for everyone."))(long, "https://www.threads.com/@zuck/post/1", "@zuck: We're building personal…", { canPreview: true, preview: true, setPreview: vi.fn() }); });
    expect(long.querySelector(".source-link-card__excerpt")).toBeNull();
    // Its words are shown whole instead of cut short.
    expect(long.querySelector(".source-link-card__title")?.textContent).toBe("@zuck: We're building personal superintelligence for everyone.");
    long.remove();
  });
});
