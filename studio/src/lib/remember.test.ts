import { afterEach, describe, expect, it, vi } from "vitest";

import { freshStore, prefs, settings } from "../testing/fixtures";
import type { Formats } from "./ipc";
import {
  applyRemembered,
  currentInterface,
  defaultInterface,
  remember,
  remembered,
  rememberedFormats,
  traceForKeeping,
  watchRemembered,
} from "./remember";

const TABS = ["vectorize", "minify", "fabricate", "batch"] as const;
const FORMATS: Formats = { svg: true, svgMinified: false, pngSizes: [512], favicon: false, assetPack: false };

afterEach(() => {
  vi.useRealTimers();
});

describe("remembered interface", () => {
  it("round-trips: what is saved from one session is what the next one shows", () => {
    const first = freshStore();
    first.set({
      preset: "icon",
      tab: "minify",
      railTab: "tune",
      groupsOpen: { ...first.state.groupsOpen, Output: true },
      view: "wipe",
      wipe: 0.25,
      show: { ...first.state.show, anchors: true },
      detail: true,
    });
    first.state.fab.unit = "in";
    first.state.batch.skipExisting = false;
    const ui = currentInterface(first.state);

    const next = freshStore();
    applyRemembered(next, prefs({ ui }), TABS, () => true);
    expect(currentInterface(next.state)).toEqual(ui);
    expect(next.state.tab).toBe("minify");
    expect(next.state.show.anchors).toBe(true);
  });

  it("keeps no colours of the drawing that was open in the Fabricate request", () => {
    const st = freshStore().state;
    st.fab.options.include = ["#112233"];
    st.fab.options.order = ["#112233"];
    const ui = currentInterface(st);
    expect(ui.fab).not.toHaveProperty("include");
    expect(ui.fab).not.toHaveProperty("order");

    const next = freshStore();
    applyRemembered(next, prefs({ ui: { ...ui, fab: { ...ui.fab, include: ["#445566"] } } }), TABS, () => true);
    expect(next.state.fab.options.include).toEqual([]);
  });

  it("drops a preset that no longer exists, and a tab this build does not have", () => {
    const store = freshStore();
    const ui = { ...defaultInterface(), preset: "saved:gone", tab: "batch" as const, batchPreset: "saved:gone" };
    applyRemembered(store, prefs({ ui }), ["vectorize", "minify", "fabricate"], (id) => id !== "saved:gone");
    expect(store.state.preset).toBeNull();
    expect(store.state.tab).toBe("vectorize");
    expect(store.state.batch.preset).toBe("logo");
  });

  it("uses the defaults for preferences written before the interface was remembered", () => {
    const store = freshStore();
    store.set({ tab: "batch", view: "ab" });
    applyRemembered(store, prefs(), TABS, () => true);
    expect(currentInterface(store.state)).toMatchObject({ preset: "logo", tab: "vectorize", view: "side", export: null, minifyBackdrop: "auto" });
  });

  it("hands components their own remembered choices, with defaults filled in", () => {
    const store = freshStore();
    applyRemembered(store, prefs({ ui: { ...defaultInterface(), export: { ...FORMATS, favicon: true }, minifyBackdrop: "dark" } }), TABS, () => true);
    expect(remembered().minifyBackdrop).toBe("dark");
    const formats = rememberedFormats({ ...FORMATS, pngSizes: [16, 32] });
    expect(formats).toEqual({ ...FORMATS, favicon: true });
    // A copy: changing it does not change what is remembered.
    formats.pngSizes.push(64);
    expect(rememberedFormats(FORMATS).pngSizes).toEqual([512]);
  });
});

describe("traceForKeeping", () => {
  it("keeps the controls but never this image's colour groups", () => {
    const withGroups = settings({ colourGroups: [{ members: ["#000000", "#111111"] }] as never });
    const kept = traceForKeeping(withGroups);
    expect(kept).not.toHaveProperty("colourGroups");
    expect(kept.precision).toBe(withGroups.precision);
    expect(withGroups.colourGroups).toBeDefined();
  });
});

describe("watchRemembered", () => {
  it("saves a moment after a remembered choice changes, once, with the trace controls", () => {
    vi.useFakeTimers();
    const store = freshStore();
    store.set({ prefs: prefs() });
    const save = vi.fn();
    const stop = watchRemembered(store, save, 500);
    store.set({ tab: "minify" });
    store.set({ railTab: "tune" });
    expect(save).not.toHaveBeenCalled();
    vi.advanceTimersByTime(500);
    expect(save).toHaveBeenCalledTimes(1);
    expect(save.mock.calls[0][0].ui).toMatchObject({ tab: "minify", railTab: "tune" });
    expect(save.mock.calls[0][0].trace).toEqual(traceForKeeping(store.state.settings));
    stop();
  });

  it("does not save when the choices are back where they were saved", () => {
    vi.useFakeTimers();
    const store = freshStore();
    store.set({ prefs: prefs() });
    const save = vi.fn();
    const stop = watchRemembered(store, save, 500);
    store.set({ tab: "minify" });
    store.set({ tab: "vectorize" });
    vi.advanceTimersByTime(1000);
    expect(save).not.toHaveBeenCalled();
    stop();
  });

  it("saves at once when the page is hidden, and not before preferences have loaded", () => {
    vi.useFakeTimers();
    const store = freshStore();
    const save = vi.fn();
    const stop = watchRemembered(store, save, 500);
    store.set({ view: "ab" });
    window.dispatchEvent(new Event("pagehide"));
    expect(save).not.toHaveBeenCalled();
    store.set({ prefs: prefs(), view: "wipe" });
    window.dispatchEvent(new Event("pagehide"));
    expect(save).toHaveBeenCalledTimes(1);
    stop();
  });

  it("saves a component's own choice, which the store does not hold", () => {
    vi.useFakeTimers();
    const store = freshStore();
    store.set({ prefs: prefs() });
    const save = vi.fn();
    const stop = watchRemembered(store, save, 500);
    remember({ minifyView: "ab" });
    vi.advanceTimersByTime(500);
    expect(save.mock.calls[0][0].ui.minifyView).toBe("ab");
    stop();
  });
});
