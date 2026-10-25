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

describe("a saved video's card", () => {
  const VIDEO = "https://www.youtube.com/watch?v=msVuCEs8Ydo";

  it("shows the video's cover once it loads, and plays YouTube's own player in its place", async () => {
    const container = document.createElement("span");
    document.body.append(container);
    const actions = { label: "Saved in Sources", open: vi.fn(), menu: { ...menu, play: "Play video" }, excerpt: vi.fn(async () => null) };
    await act(async () => { sourceLinkCardMount(() => actions)(container, VIDEO, "Is there something faster than light? · Veritasium", { canPreview: true, preview: true, setPreview: vi.fn(), resized: vi.fn() }); });
    const cover = container.querySelector<HTMLButtonElement>(".source-link-card__cover")!;
    expect(container.querySelector(".source-link-card--media")).toBeNull();
    expect(cover.querySelector("img")?.getAttribute("src")).toBe("https://i.ytimg.com/vi/msVuCEs8Ydo/hqdefault.jpg");
    await act(async () => { cover.querySelector("img")!.dispatchEvent(new Event("load")); });
    expect(container.querySelector(".source-link-card--media")).not.toBeNull();
    expect(cover.getAttribute("aria-label")).toBe("Play video: Is there something faster than light? · Veritasium");
    // Nothing is asked of YouTube's player until the video is played.
    expect(container.querySelector("iframe")).toBeNull();
    await act(async () => { cover.click(); });
    expect(container.querySelector("iframe")?.getAttribute("src")).toBe("https://wenlan.app/embed/youtube.html#msVuCEs8Ydo");
    container.remove();
  });

  it("stays words only when the cover cannot load", async () => {
    const container = document.createElement("span");
    document.body.append(container);
    await act(async () => { sourceLinkCardMount(actionsWith(vi.fn(), null))(container, VIDEO, "A video", { canPreview: true, preview: true, setPreview: vi.fn() }); });
    await act(async () => { container.querySelector(".source-link-card__cover img")!.dispatchEvent(new Event("error")); });
    expect(container.querySelector(".source-link-card__cover")).toBeNull();
    expect(container.querySelector(".source-link-card__title")?.textContent).toBe("A video");
    container.remove();
  });
});

describe("a saved post's card", () => {
  async function postCard() {
    const container = document.createElement("span");
    document.body.append(container);
    const resized = vi.fn();
    await act(async () => { sourceLinkCardMount(actionsWith(vi.fn(), "@jack: just setting up my twttr"))(container, "https://x.com/jack/status/20?s=20", "@jack: just setting up my twttr", { canPreview: true, preview: true, setPreview: vi.fn(), resized }); });
    const frame = container.querySelector<HTMLIFrameElement>(".source-link-card__post")!;
    const tell = (data: unknown, origin = "https://platform.twitter.com", source: MessageEventSource | null = frame.contentWindow) => act(async () => {
      window.dispatchEvent(new MessageEvent("message", { data, origin, source }));
    });
    return { container, frame, tell, resized };
  }
  const resize = (height: number) => ({ "twttr.embed": { method: "twttr.private.resize", params: [{ height, width: 550 }] } });

  it("loads X's own embed page, out of sight until X says how tall the post is", async () => {
    const { container, frame, tell, resized } = await postCard();
    expect(frame.getAttribute("src")).toMatch(/^https:\/\/platform\.twitter\.com\/embed\/Tweet\.html\?id=20&dnt=true&theme=/);
    expect(frame.className).toContain("source-link-card__post--waiting");
    expect(container.querySelector(".source-link-card__title")?.textContent).toBe("@jack: just setting up my twttr");
    // Another frame, or another site, cannot size it.
    await tell(resize(412), "https://platform.twitter.com", window);
    await tell(resize(412), "https://evil.example");
    expect(frame.className).toContain("source-link-card__post--waiting");
    await tell(resize(412));
    expect(frame.className).not.toContain("--waiting");
    expect(frame.style.height).toBe("412px");
    expect(container.querySelector(".source-link-card--media")).not.toBeNull();
    expect(resized).toHaveBeenCalled();
    container.remove();
  });

  it("falls back to its words when the site does not draw the post in time", async () => {
    vi.useFakeTimers();
    try {
      const { container } = await postCard();
      await act(async () => { vi.advanceTimersByTime(15_000); });
      expect(container.querySelector("iframe")).toBeNull();
      expect(container.querySelector(".source-link-card__title")?.textContent).toBe("@jack: just setting up my twttr");
      container.remove();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("a saved link's menu near the bottom of the window", () => {
  it("opens upward when there is no room below", async () => {
    const container = document.createElement("span");
    document.body.append(container);
    await act(async () => { sourceLinkCardMount(actionsWith(vi.fn(), "You own your data"))(container, `${WEB}/bottom`, "Local-first software", { canPreview: true, preview: true, setPreview: vi.fn() }); });
    const more = container.querySelector<HTMLButtonElement>(".source-link-card__more")!;
    more.getBoundingClientRect = () => ({ top: window.innerHeight - 30, bottom: window.innerHeight - 4, left: 100, right: 126, width: 26, height: 26, x: 100, y: window.innerHeight - 30, toJSON: () => ({}) });
    const height = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(120);
    try {
      await act(async () => { more.click(); });
      const menu = document.querySelector<HTMLElement>(".source-link-menu")!;
      expect(Number.parseFloat(menu.style.top)).toBe(window.innerHeight - 30 - 120 - 4);
    } finally {
      height.mockRestore();
      container.remove();
    }
  });
});

