import { beforeAll, beforeEach, describe, expect, it } from "vitest";

import { carriedFile, HANDOFF_MAX, handOffHash, pastedImages } from "./chrome";

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

/** The presentation page's encoder (`base64url` in web/index.html), as it writes the fragment. */
function base64url(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

describe("carriedFile", () => {
  it("gives back every byte the presentation page put in the fragment, whatever the padding", () => {
    const all = Uint8Array.from({ length: 256 * 3 + 2 }, (_, i) => (i * 37 + 11) & 255);
    for (const n of [1, 2, 3, 4, 255, 256, all.length]) {
      const bytes = all.subarray(0, n);
      const got = carriedFile(`#open=${base64url(bytes)}&name=${encodeURIComponent("logo.png")}`);
      expect(got?.name).toBe("logo.png");
      expect([...(got?.bytes ?? [])]).toEqual([...bytes]);
    }
  });

  it("keeps a name with spaces, ampersands and accents", () => {
    const name = "Café & co — final v2.png";
    const got = carriedFile(`#open=${base64url(new Uint8Array([137, 80, 78, 71]))}&name=${encodeURIComponent(name)}`);
    expect(got?.name).toBe(name);
  });

  it("is null without a file, or with one that does not decode", () => {
    expect(carriedFile("")).toBeNull();
    expect(carriedFile("#")).toBeNull();
    expect(carriedFile("#section")).toBeNull();
    expect(carriedFile("#open=")).toBeNull();
    expect(carriedFile("#open=%%%")).toBeNull();
    expect(carriedFile("open=AAAA")).toBeNull();
  });
});

describe("handOffHash", () => {
  it("writes what carriedFile reads back, byte for byte, with the name", () => {
    const all = Uint8Array.from({ length: 70_000 }, (_, i) => (i * 131 + 7) & 255);
    for (const n of [1, 2, 3, 4, 0x8000, 0x8000 + 1, all.length]) {
      const bytes = all.subarray(0, n);
      const hash = handOffHash("Café & co.png", bytes);
      // The presentation page writes the same fragment for the same file.
      expect(hash).toBe(`#open=${base64url(bytes)}&name=${encodeURIComponent("Café & co.png")}`);
      const got = carriedFile(hash ?? "");
      expect(got?.name).toBe("Café & co.png");
      expect([...(got?.bytes ?? [])]).toEqual([...bytes]);
    }
  });

  it("carries a file of exactly HANDOFF_MAX bytes and refuses one byte more, or an empty one", () => {
    expect(HANDOFF_MAX).toBe(1_500_000);
    expect(handOffHash("a.png", new Uint8Array(HANDOFF_MAX))).not.toBeNull();
    expect(handOffHash("a.png", new Uint8Array(HANDOFF_MAX + 1))).toBeNull();
    expect(handOffHash("a.png", new Uint8Array(0))).toBeNull();
  });
});
