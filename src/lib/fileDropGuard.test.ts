// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it } from "vitest";
import { refuseStrayFileDrops } from "./fileDropGuard";

function dragEvent(type: string, types: string[]): Event {
  const event = new Event(type, { bubbles: true, cancelable: true });
  Object.defineProperty(event, "dataTransfer", { value: { types, dropEffect: "copy" } });
  return event;
}

let release = () => {};
afterEach(() => {
  release();
  document.body.replaceChildren();
});

describe("stray file drops", () => {
  it("refuses a file dropped where nothing takes it", () => {
    release = refuseStrayFileDrops();
    const over = dragEvent("dragover", ["Files"]);
    document.body.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(true);
    expect((over as DragEvent).dataTransfer?.dropEffect).toBe("none");
    const drop = dragEvent("drop", ["Files"]);
    document.body.dispatchEvent(drop);
    expect(drop.defaultPrevented).toBe(true);
  });

  it("leaves a drop zone that takes the file alone", () => {
    release = refuseStrayFileDrops();
    const zone = document.createElement("div");
    zone.addEventListener("dragover", (event) => event.preventDefault());
    document.body.append(zone);
    const over = dragEvent("dragover", ["Files"]);
    zone.dispatchEvent(over);
    expect((over as DragEvent).dataTransfer?.dropEffect).toBe("copy");
  });

  it("ignores drags that carry no files", () => {
    release = refuseStrayFileDrops();
    const over = dragEvent("dragover", ["text/plain"]);
    document.body.dispatchEvent(over);
    expect(over.defaultPrevented).toBe(false);
  });
});
