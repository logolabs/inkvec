import { describe, expect, it } from "vitest";

import { caps, preset, prefs, settings } from "../testing/fixtures";
import type { PresetId } from "./ipc";
import { DEFAULT_SETTINGS, presetIndexForKey, resolvePreset } from "./presets";

/** The ten built-in presets, in the order the engine lists them. */
const TEN: PresetId[] = [
  "logo",
  "icon",
  "fine-detail",
  "fewer-paths",
  "photo-or-scan",
  "monochrome",
  "monochrome-transparent",
  "black-and-white",
  "line-art",
  "editable",
];

describe("presetIndexForKey", () => {
  it("maps 1 to 9 onto the first nine presets and 0 onto the tenth", () => {
    expect([..."1234567890"].map(presetIndexForKey)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it("reaches every one of the ten presets exactly once", () => {
    const list = TEN.map((id) => preset(id));
    const reached = [..."1234567890"].map((k) => list[presetIndexForKey(k)!].id);
    expect(new Set(reached).size).toBe(10);
    expect(reached[0]).toBe("logo");
    expect(reached[9]).toBe("editable");
  });

  it("ignores every key that is not a single digit", () => {
    for (const key of ["", "a", "10", "O", "e", ",", "Escape", " 1", "١"]) expect(presetIndexForKey(key)).toBeNull();
  });
});

describe("resolvePreset", () => {
  const builtIn = caps([preset("logo"), preset("photo-or-scan", { cleanUpDamage: "auto" }, true)]);
  const saved = prefs({ saved: [{ id: "saved:abc", name: "Mine", settings: settings({ precision: 0.3 }) }] });

  it("finds a built-in preset, with whether it wants the denoiser", () => {
    expect(resolvePreset(builtIn, saved, "photo-or-scan")).toEqual({
      settings: settings({ cleanUpDamage: "auto" }),
      wantsDenoiser: true,
    });
  });

  it("finds a saved preset, which never asks for the denoiser", () => {
    expect(resolvePreset(builtIn, saved, "saved:abc")).toEqual({ settings: settings({ precision: 0.3 }), wantsDenoiser: false });
  });

  it("returns null for an id that no longer names a preset, or before anything has loaded", () => {
    expect(resolvePreset(builtIn, saved, "saved:deleted")).toBeNull();
    expect(resolvePreset(null, null, "logo")).toBeNull();
  });

  it("hands back the preset's own settings object, which callers copy before changing", () => {
    expect(resolvePreset(builtIn, null, "logo")?.settings).toBe(builtIn.presets[0].settings);
  });
});

describe("DEFAULT_SETTINGS", () => {
  it("starts in Quality with every special route off", () => {
    expect(DEFAULT_SETTINGS.mode).toBe("quality");
    expect(DEFAULT_SETTINGS.blackAndWhite || DEFAULT_SETTINGS.monochrome || DEFAULT_SETTINGS.lineArt).toBe(false);
    expect(DEFAULT_SETTINGS.cleanUpDamage).toBe("off");
    expect("colourGroups" in DEFAULT_SETTINGS).toBe(false);
  });
});
