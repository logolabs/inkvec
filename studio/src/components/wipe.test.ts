import { describe, expect, it, vi } from "vitest";

import { clampWipe, tapToWipe, WIPE_MAX, WIPE_MIN, wipeAfterKey, wipeSlider, type WipeModel } from "./wipe";

describe("the wipe slider's keys (WAI-ARIA APG slider pattern)", () => {
  it("steps by 2 points, 10 with Shift or Page Up/Down, and goes to the ends on Home and End", () => {
    expect(wipeAfterKey(0.5, "ArrowRight", false)).toBeCloseTo(0.52);
    expect(wipeAfterKey(0.5, "ArrowUp", false)).toBeCloseTo(0.52);
    expect(wipeAfterKey(0.5, "ArrowLeft", false)).toBeCloseTo(0.48);
    expect(wipeAfterKey(0.5, "ArrowDown", true)).toBeCloseTo(0.4);
    expect(wipeAfterKey(0.5, "PageUp", false)).toBeCloseTo(0.6);
    expect(wipeAfterKey(0.5, "PageDown", false)).toBeCloseTo(0.4);
    expect(wipeAfterKey(0.5, "Home", false)).toBe(WIPE_MIN);
    expect(wipeAfterKey(0.5, "End", false)).toBe(WIPE_MAX);
  });

  it("never leaves the range the handle can be grabbed back from", () => {
    expect(wipeAfterKey(0.03, "ArrowLeft", true)).toBe(WIPE_MIN);
    expect(wipeAfterKey(0.97, "PageUp", false)).toBe(WIPE_MAX);
    expect(clampWipe(-1)).toBe(WIPE_MIN);
    expect(clampWipe(2)).toBe(WIPE_MAX);
    expect(clampWipe(0.3)).toBe(0.3);
  });

  it("ignores keys a slider does not use", () => {
    expect(wipeAfterKey(0.5, "a", false)).toBeNull();
    expect(wipeAfterKey(0.5, " ", false)).toBeNull();
    expect(wipeAfterKey(0.5, "Tab", false)).toBeNull();
  });
});

/** A two-pane strip 200 px wide at x = 100, with its divider, over a model held in a variable. */
function rig(start = 0.5): { divider: HTMLElement; panes: HTMLElement; model: WipeModel; value: () => number; sync: () => void } {
  const panes = document.createElement("div");
  const divider = document.createElement("div");
  panes.append(divider);
  document.body.append(panes);
  panes.getBoundingClientRect = () => ({ left: 100, top: 0, right: 300, bottom: 100, width: 200, height: 100, x: 100, y: 0, toJSON: () => ({}) }) as DOMRect;
  let v = start;
  const model: WipeModel = { get: () => v, set: (n) => (v = n), names: () => ["source", "vector"] };
  const sync = wipeSlider(divider, panes, model);
  return { divider, panes, model, value: () => v, sync };
}

describe("wipeSlider", () => {
  it("is a focusable slider with its range, value and a value text naming both panes", () => {
    const { divider } = rig(0.62);
    expect(divider.getAttribute("role")).toBe("slider");
    expect(divider.getAttribute("tabindex")).toBe("0");
    expect(divider.getAttribute("aria-valuemin")).toBe("2");
    expect(divider.getAttribute("aria-valuemax")).toBe("98");
    expect(divider.getAttribute("aria-valuenow")).toBe("62");
    expect(divider.getAttribute("aria-valuetext")).toBe("62% source, 38% vector");
    expect(divider.getAttribute("aria-label")).toBe("Wipe between source and vector");
  });

  it("moves on the arrow keys, and keeps them from the window's own shortcuts", () => {
    const { divider, value, sync } = rig(0.5);
    const behind = vi.fn();
    window.addEventListener("keydown", behind);
    const e = new KeyboardEvent("keydown", { key: "ArrowLeft", shiftKey: true, bubbles: true, cancelable: true });
    divider.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
    expect(value()).toBeCloseTo(0.4);
    expect(behind).not.toHaveBeenCalled();
    sync();
    expect(divider.getAttribute("aria-valuenow")).toBe("40");
    // A key it does not use goes on to the window as before.
    divider.dispatchEvent(new KeyboardEvent("keydown", { key: "x", bubbles: true }));
    expect(behind).toHaveBeenCalledTimes(1);
    window.removeEventListener("keydown", behind);
  });

  it("moves to a tap, as a share of the panes' width, clamped", () => {
    const { panes, model, value } = rig(0.5);
    tapToWipe(panes, model, 150);
    expect(value()).toBeCloseTo(0.25);
    tapToWipe(panes, model, 90);
    expect(value()).toBe(WIPE_MIN);
  });
});
