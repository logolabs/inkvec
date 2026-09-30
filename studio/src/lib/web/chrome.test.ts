import { beforeAll, beforeEach, describe, expect, it } from "vitest";

import { pastedImages } from "./chrome";

/** What the paste listener handed over, per paste. */
const taken: File[][] = [];

/**
 * A `paste` event on `target` carrying `files` in the clipboard's file list, or only as
 * items when `asItems` is set, the way some webviews hand a copied bitmap over.
 */
function paste(target: EventTarget, files: File[], asItems = false): Event {
  const e = new Event("paste", { bubbles: true, cancelable: true });
  const items = files.map((f) => ({ kind: "file", getAsFile: () => f }));
  const clipboardData = asItems ? { files: [], items: [...items, { kind: "string", getAsFile: () => null }] } : { files, items };
  Object.defineProperty(e, "clipboardData", { value: clipboardData });
  target.dispatchEvent(e);
  return e;
}

const png = () => new File(["\x89PNG"], "image.png", { type: "image/png" });

beforeAll(() => {
  // The listener is on the window for the life of the page; install it once.
  pastedImages((files) => taken.push(files));
});

beforeEach(() => {
  taken.length = 0;
  document.body.innerHTML = '<div id="stage"></div><input id="field"><textarea id="notes"></textarea><div id="rich" contenteditable="true"><span id="inner"></span></div>';
});

describe("pastedImages", () => {
  it("takes the files a paste carries, and keeps the paste from doing anything else", () => {
    const file = png();
    const e = paste(document.getElementById("stage")!, [file]);
    expect(taken).toEqual([[file]]);
    expect(e.defaultPrevented).toBe(true);
  });

  it("falls back to the clipboard's items when its file list is empty", () => {
    const file = png();
    paste(document.getElementById("stage")!, [file], true);
    expect(taken).toEqual([[file]]);
  });

  it("ignores a paste into a text field, which is somebody typing", () => {
    for (const id of ["field", "notes", "rich", "inner"]) {
      const e = paste(document.getElementById(id)!, [png()]);
      expect(e.defaultPrevented).toBe(false);
    }
    expect(taken).toEqual([]);
  });

  it("ignores a paste with no files in it, and leaves it alone", () => {
    const e = paste(document.getElementById("stage")!, []);
    expect(taken).toEqual([]);
    expect(e.defaultPrevented).toBe(false);
  });

  it("takes a paste aimed at the page itself", () => {
    paste(window, [png()]);
    expect(taken).toHaveLength(1);
  });
});
