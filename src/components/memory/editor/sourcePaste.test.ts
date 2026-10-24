// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { installCodeMirrorDomPolyfills, pasteText, pressKey } from "./editorTestUtils";
import {
  pastedSourceUrl,
  setSourceLinkUrls,
  sourceLinkMarkdown,
  sourceLinkUrls,
  sourcePaste,
  type PastedSourceSaved,
  type SourcePasteOptions,
} from "./sourcePaste";
import { writingPresentation } from "./writingPresentation";

beforeAll(() => installCodeMirrorDomPolyfills());

const views: EditorView[] = [];
afterEach(() => {
  for (const view of views.splice(0)) view.destroy();
  document.body.replaceChildren();
  vi.useRealTimers();
});

const labels = {
  saving: "Saving…", savedLinkOnly: "This site needs a sign-in, so only the link was saved",
  unreadable: "Couldn't read this page, so only the link was saved",
  failed: "Couldn't save this link.", retry: "Try again",
};

const X_URL = "https://x.com/simonw/status/1849";

function editor(doc: string, at: number, options: Partial<SourcePasteOptions> = {}): EditorView {
  const parent = document.createElement("div");
  document.body.append(parent);
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc,
      selection: EditorSelection.single(at),
      extensions: [
        markdown({ base: markdownLanguage }),
        sourceLinkUrls,
        sourcePaste({ blocked: () => false, enabled: () => true, labels: () => labels, save: vi.fn(), ...options }),
      ],
    }),
  });
  views.push(view);
  return view;
}

function saved(title: string, linkOnly?: PastedSourceSaved["linkOnly"], remove?: () => Promise<void>): PastedSourceSaved {
  return { title, linkOnly, remove };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const note = (view: EditorView) => view.dom.querySelector<HTMLElement>(".cm-source-paste");
const buttonNamed = (view: EditorView, name: string) =>
  [...view.dom.querySelectorAll<HTMLButtonElement>(".cm-source-paste button")].find((b) => b.textContent === name);

describe("pasted links as sources", () => {
  it("only takes one public http(s) address", () => {
    expect(pastedSourceUrl(`  ${X_URL}\n`)).toBe(X_URL);
    expect(pastedSourceUrl("see https://x.com/a")).toBeNull();
    expect(pastedSourceUrl("ftp://example.com/a")).toBeNull();
    expect(pastedSourceUrl("https://me:pw@example.com/a")).toBeNull();
    expect(pastedSourceUrl("http://localhost:3000")).toBeNull();
  });

  it("writes the link with its title, escaping brackets", () => {
    expect(sourceLinkMarkdown("A [draft]  post", "https://a.com/x")).toBe("[A \\[draft\\] post](https://a.com/x)");
    expect(sourceLinkMarkdown("  ", "https://a.com/x")).toBe("https://a.com/x");
  });

  it("saves a pasted link at once and shows it under its title, with no note once saved", async () => {
    const save = vi.fn(async () => saved("Simon on local-first"));
    const view = editor("Reactions: ", 11, { save });
    const event = pasteText(view.contentDOM, X_URL);
    expect(event.defaultPrevented).toBe(true);
    expect(save).toHaveBeenCalledWith(X_URL);
    expect(note(view)?.textContent).toBe("Saving…");
    await flush();
    expect(view.state.doc.toString()).toBe(`Reactions: [Simon on local-first](${X_URL})`);
    expect(note(view)).toBeNull();
  });

  it("says when only the link was kept, then puts the note away", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const view = editor("", 0, { save: vi.fn(async () => saved("X link", "signIn")) });
    pasteText(view.contentDOM, X_URL);
    await vi.waitFor(() => expect(note(view)?.textContent).toBe(labels.savedLinkOnly));
    vi.advanceTimersByTime(5000);
    expect(note(view)).toBeNull();
  });

  it("a page that could not be read says so, not that it needs a sign-in", async () => {
    const view = editor("", 0, { save: vi.fn(async () => saved("example.com", "unreadable")) });
    pasteText(view.contentDOM, "https://example.com/post");
    await vi.waitFor(() => expect(note(view)?.textContent).toBe(labels.unreadable));
  });

  it("editing the link's tracking parameters or trailing slash keeps its source", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const remove = vi.fn(async () => undefined);
    const url = "https://www.example.com/post/?utm_source=x#top";
    const view = editor("", 0, { save: vi.fn(async () => saved("Post", undefined, remove)) });
    pasteText(view.contentDOM, url);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe(`[Post](${url})`));
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "[Post](https://example.com/post)" } });
    vi.advanceTimersByTime(5000);
    expect(remove).not.toHaveBeenCalled();
    // Pointing it at another page is a different link.
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "[Post](https://example.com/other)" } });
    vi.advanceTimersByTime(5000);
    expect(remove).toHaveBeenCalledTimes(1);
  });

  it("deleting the link deletes the source the paste created", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const remove = vi.fn(async () => undefined);
    const save = vi.fn(async () => saved("Post", undefined, remove));
    const view = editor("", 0, { save });
    pasteText(view.contentDOM, X_URL);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe(`[Post](${X_URL})`));
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "" } });
    vi.advanceTimersByTime(2000);
    expect(remove).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1000);
    expect(remove).toHaveBeenCalledTimes(1);
    // Pasted again, it is saved again.
    pasteText(view.contentDOM, X_URL);
    expect(save).toHaveBeenCalledTimes(2);
  });

  it("a cut and paste of the link keeps its source", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const remove = vi.fn(async () => undefined);
    const view = editor("", 0, { save: vi.fn(async () => saved("Post", undefined, remove)) });
    pasteText(view.contentDOM, X_URL);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe(`[Post](${X_URL})`));
    const link = view.state.doc.toString();
    view.dispatch({ changes: { from: 0, to: link.length, insert: "" } });
    vi.advanceTimersByTime(1000);
    view.dispatch({ changes: { from: 0, insert: `Moved: ${link}` } });
    vi.advanceTimersByTime(5000);
    expect(remove).not.toHaveBeenCalled();
  });

  it("keeps a source that was saved before when its link is deleted", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const view = editor("", 0, { save: vi.fn(async () => saved("Post")) });
    pasteText(view.contentDOM, X_URL);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe(`[Post](${X_URL})`));
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "" } });
    vi.advanceTimersByTime(5000);
    expect(note(view)).toBeNull();
  });

  it("offers a retry when saving fails, and keeps the link", async () => {
    const save = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(saved("Post"));
    const view = editor("", 0, { save });
    pasteText(view.contentDOM, X_URL);
    await flush();
    expect(view.state.doc.toString()).toBe(X_URL);
    expect(note(view)?.textContent).toBe("Couldn't save this link.Try again");
    buttonNamed(view, "Try again")!.click();
    await flush();
    expect(view.state.doc.toString()).toBe(`[Post](${X_URL})`);
  });

  it("Escape puts the failure note away", async () => {
    const view = editor("", 0, { save: vi.fn().mockRejectedValue(new Error("offline")) });
    pasteText(view.contentDOM, X_URL);
    await flush();
    pressKey(view.contentDOM, "Escape");
    expect(note(view)).toBeNull();
  });

  it("keeps the note through typing while the save is still running", async () => {
    let finish: (value: PastedSourceSaved) => void = () => undefined;
    const view = editor("", 0, { save: vi.fn(() => new Promise<PastedSourceSaved>((resolve) => { finish = resolve; })) });
    pasteText(view.contentDOM, X_URL);
    view.dispatch({ changes: { from: view.state.doc.length, insert: " nice" } });
    expect(note(view)?.textContent).toBe("Saving…");
    finish(saved("Post"));
    await flush();
    expect(view.state.doc.toString()).toBe(`[Post](${X_URL}) nice`);
  });

  it("leaves code, link targets and repeated links to the normal paste", async () => {
    const save = vi.fn(async () => saved("Post"));
    const inCode = editor("```\n\n```", 4, { save });
    pasteText(inCode.contentDOM, X_URL);
    const inTarget = editor("[read](", 7, { save });
    pasteText(inTarget.contentDOM, X_URL);
    const known = editor("", 0, { save });
    known.dispatch({ effects: setSourceLinkUrls.of(new Set([X_URL])) });
    pasteText(known.contentDOM, X_URL);
    expect(save).not.toHaveBeenCalled();
    expect([inCode, inTarget, known].map(note)).toEqual([null, null, null]);
    const twice = editor("", 0, { save });
    pasteText(twice.contentDOM, X_URL);
    await flush();
    twice.dispatch({ changes: { from: twice.state.doc.length, insert: "\n" }, selection: { anchor: twice.state.doc.length + 1 } });
    pasteText(twice.contentDOM, X_URL);
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("does nothing when saving is off for this editor", () => {
    const save = vi.fn(async () => saved("Post"));
    const view = editor("", 0, { enabled: () => false, save });
    pasteText(view.contentDOM, X_URL);
    expect(save).not.toHaveBeenCalled();
    expect(note(view)).toBeNull();
  });
});

describe("links saved as sources", () => {
  it("show the site icon in place of the outside-link arrow", () => {
    const parent = document.createElement("div");
    document.body.append(parent);
    const view = new EditorView({
      parent,
      state: EditorState.create({
        doc: `[Post](${X_URL}) and [Other](https://example.com/a)\n\nend`,
        selection: EditorSelection.single(60),
        extensions: [markdown({ base: markdownLanguage }), sourceLinkUrls, writingPresentation()],
      }),
    });
    views.push(view);
    view.dispatch({ effects: setSourceLinkUrls.of(new Set([X_URL])) });
    expect(view.dom.querySelectorAll(".cm-source-link-icon")).toHaveLength(1);
    expect(view.dom.querySelectorAll(".cm-writing-link-external")).toHaveLength(1);
  });
});
