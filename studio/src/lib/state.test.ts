import { describe, expect, it, vi } from "vitest";

import { control, freshStore, settings, source } from "../testing/fixtures";
import { appliesTo, bytes, count, de00, duration, engineName, fastEngine, modKey, percent, plannedTracePx, seconds } from "./state";

describe("appliesTo", () => {
  const both = control("speckleFloor", "both");
  const qualityOnly = control("precision", "qualityOnly");
  const fastOnly = control("cornerAngle", "fastOnly");

  it("shows every control but the Fast-only ones in Quality", () => {
    const quality = settings({ mode: "quality" });
    expect([both, qualityOnly, fastOnly].map((c) => appliesTo(c, quality))).toEqual([true, true, false]);
  });

  it("hides the Quality-only controls in Fast", () => {
    const fast = settings({ mode: "fast" });
    expect([both, qualityOnly, fastOnly].map((c) => appliesTo(c, fast))).toEqual([true, false, true]);
  });

  it("reads Fast as Quality under black & white or line art, which fit with Quality's fitter", () => {
    for (const route of [{ blackAndWhite: true }, { lineArt: true }]) {
      const s = settings({ mode: "fast", ...route });
      expect([both, qualityOnly, fastOnly].map((c) => appliesTo(c, s))).toEqual([true, true, false]);
    }
  });

  it("shows Balanced what Fast shows: it runs the Fast engine", () => {
    const balanced = settings({ mode: "balanced" });
    expect([both, qualityOnly, fastOnly].map((c) => appliesTo(c, balanced))).toEqual([true, false, true]);
    expect([fastEngine("balanced"), fastEngine("fast"), fastEngine("quality")]).toEqual([true, true, false]);
    expect(["quality", "balanced", "fast"].map((m) => engineName(m as "quality"))).toEqual(["Quality", "Balanced", "Fast"]);
  });

  it("does not treat monochrome as its own route", () => {
    const s = settings({ mode: "fast", monochrome: true });
    expect(appliesTo(qualityOnly, s)).toBe(false);
  });
});

describe("Store", () => {
  it("merges a patch and tells a subscriber only about keys that changed", () => {
    const store = freshStore();
    const onTab = vi.fn();
    const onZoom = vi.fn();
    store.on(["tab"], onTab);
    store.on(["zoom"], onZoom);
    store.set({ tab: "minify", zoom: store.state.zoom });
    expect(store.state.tab).toBe("minify");
    expect(onTab).toHaveBeenCalledTimes(1);
    expect(onZoom).not.toHaveBeenCalled();
  });

  it("stays silent when nothing changed, and compares by identity", () => {
    const store = freshStore();
    const fn = vi.fn();
    store.on(["tab", "pan"], fn);
    store.set({ tab: store.state.tab });
    expect(fn).not.toHaveBeenCalled();
    // An equal but new object is a change: the store never compares contents.
    store.set({ pan: { ...store.state.pan } });
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("calls a subscriber once per change, however many of its keys moved", () => {
    const store = freshStore();
    const fn = vi.fn();
    store.on(["tab", "zoom", "railTab"], fn);
    store.set({ tab: "batch", zoom: 3, railTab: "tune" });
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("announces a key mutated in place when touched", () => {
    const store = freshStore();
    const fn = vi.fn();
    store.on(["settings"], fn);
    store.state.settings.precision = 0.3;
    store.touch("settings");
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("stops calling a subscriber once it unsubscribes, even from inside a notification", () => {
    const store = freshStore();
    const later = vi.fn();
    const once = vi.fn(() => stop());
    const stop = store.on(["tab"], once);
    store.on(["tab"], later);
    store.set({ tab: "minify" });
    store.set({ tab: "fabricate" });
    expect(once).toHaveBeenCalledTimes(1);
    expect(later).toHaveBeenCalledTimes(2);
  });
});

describe("plannedTracePx", () => {
  it("is the smaller of the cap and the image's longer side", () => {
    const st = freshStore().state;
    expect(plannedTracePx(st, "final")).toBeNull();
    st.source = source({ width: 622, height: 300 });
    expect(plannedTracePx(st, "final")).toBe(622);
    st.source = source({ width: 4000, height: 3000 });
    expect(plannedTracePx(st, "final")).toBe(2048);
  });

  it("caps a draft at the draft size too", () => {
    const st = freshStore().state;
    st.source = source({ width: 4000, height: 3000 });
    expect(plannedTracePx(st, "draft")).toBe(512);
    st.settings.traceSize = 300;
    expect(plannedTracePx(st, "draft")).toBe(300);
  });
});

describe("formatting", () => {
  it("writes counts, bytes and shares the one way every panel shows them", () => {
    expect(count(1382)).toBe("1,382");
    expect([bytes(1023), bytes(1536), bytes(3 * 1024 * 1024)]).toEqual(["1023 B", "1.5 KB", "3.0 MB"]);
    expect([percent(0.256), percent(0.256, 1)]).toEqual(["26%", "25.6%"]);
  });

  it("gives a colour difference two decimals, and a dash for none", () => {
    expect([de00(0.1), de00(0.126), de00(null), de00(undefined)]).toEqual(["0.10", "0.13", "—", "—"]);
  });

  it("writes times in the unit a reader can act on", () => {
    expect([seconds(0.4321), seconds(2.5)]).toEqual(["432 ms", "2.50 s"]);
    expect([duration(-3), duration(59.4), duration(59.6), duration(252)]).toEqual(["0 s", "59 s", "1 min 0 s", "4 min 12 s"]);
  });

  it("names the modifier key the platform uses", () => {
    expect([modKey("macos"), modKey("windows"), modKey(undefined)]).toEqual(["⌘", "Ctrl", "Ctrl"]);
  });
});
