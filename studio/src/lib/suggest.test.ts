import { describe, expect, it } from "vitest";

import { caps, control, facts, ink, loss, preset, report, settings, source } from "../testing/fixtures";
import type { Settings } from "./ipc";
import { autoSuggestions, changedControls, describeAuto, looksDamaged, NOISY_LEVELS, type SuggestInput } from "./suggest";

/** A clean, 1024 px, multi-colour PNG that suggests nothing, with `patch` over it. */
function input(patch: Partial<SuggestInput> = {}): SuggestInput {
  return {
    source: source({ width: 1024, height: 1024 }),
    report: report(),
    palette: [ink("#d04020"), ink("#2040d0"), ink("#20a040")],
    losses: [],
    facts: facts(),
    settings: settings(),
    preset: "logo",
    ...patch,
  };
}

const ids = (i: SuggestInput) => autoSuggestions(i).map((s) => s.id);
const BLACK_ON_WHITE = [ink("#000000", 0.3), ink("#ffffff", 0.7)];

describe("looksDamaged", () => {
  it("reads a lossy container or measurable noise as damage", () => {
    expect(looksDamaged(source({ lossy: true }), null)).toBe(true);
    expect(looksDamaged(source(), facts({ noiseLevels: NOISY_LEVELS }))).toBe(true);
    expect(looksDamaged(source(), facts({ noiseLevels: NOISY_LEVELS - 0.1 }))).toBe(false);
    expect(looksDamaged(null, null)).toBe(false);
  });
});

describe("autoSuggestions", () => {
  it("says nothing about a clean logo, or before there is a trace", () => {
    expect(ids(input())).toEqual([]);
    expect(ids(input({ report: null }))).toEqual([]);
    expect(ids(input({ source: null }))).toEqual([]);
  });

  it("suggests the denoiser for a JPEG, with the evidence, unless it is already on", () => {
    const jpeg = source({ width: 1024, height: 1024, lossy: true, container: "JPEG" });
    const [photo] = autoSuggestions(input({ source: jpeg, facts: facts({ noiseLevels: 3.4 }) }));
    expect(photo).toMatchObject({ id: "photo", preset: "photo-or-scan", evidence: "JPEG · noise 3.4 levels" });
    expect(ids(input({ source: jpeg, settings: settings({ cleanUpDamage: "auto" }) }))).toEqual([]);
  });

  it("suggests the denoiser for a noisy PNG", () => {
    expect(ids(input({ facts: facts({ noiseLevels: 3 }) }))).toEqual(["photo"]);
  });

  it("suggests two tones for a black and white drawing", () => {
    expect(ids(input({ palette: BLACK_ON_WHITE }))).toEqual(["bw"]);
  });

  it("does not suggest two tones when monochrome or black & white is already on", () => {
    expect(ids(input({ palette: BLACK_ON_WHITE, settings: settings({ monochrome: true }) }))).toEqual([]);
    expect(ids(input({ palette: BLACK_ON_WHITE, settings: settings({ blackAndWhite: true }) }))).toEqual([]);
  });

  it("only calls a pair of neutrals, one dark and one light, black and white", () => {
    expect(ids(input({ palette: [ink("#000000"), ink("#ff0000")] }))).toEqual([]);
    expect(ids(input({ palette: [ink("#000000"), ink("#777777")] }))).toEqual([]);
    expect(ids(input({ palette: [ink("#000000"), ink("#ffffff", 0.5, "gradient")] }))).toEqual([]);
    expect(ids(input({ palette: [ink("#000000"), ink("#ffffff"), ink("#808080")] }))).toEqual([]);
  });

  it("counts only inks that cover a visible share of the canvas", () => {
    // A stray third colour on 0.1% of the canvas is noise, not a third ink.
    expect(ids(input({ palette: [...BLACK_ON_WHITE, ink("#ff0000", 0.001)] }))).toEqual(["bw"]);
  });

  it("suggests line art when strokes came back as filled outlines", () => {
    expect(ids(input({ losses: [loss("strokes")] }))).toEqual(["lineart"]);
    expect(ids(input({ losses: [loss("strokes")], settings: settings({ lineArt: true }) }))).toEqual([]);
    expect(ids(input({ losses: [loss("strokes")], settings: settings({ blackAndWhite: true }) }))).toEqual([]);
    expect(ids(input({ losses: [loss("lettering")] }))).toEqual([]);
  });

  it("suggests Icon for a small flat mark, but only when nothing else is suggested", () => {
    const small = source({ width: 200, height: 120 });
    const [icon] = autoSuggestions(input({ source: small }));
    expect(icon).toMatchObject({ id: "icon", evidence: "200 px, 3 colours" });
    expect(ids(input({ source: small, losses: [loss("strokes")] }))).toEqual(["lineart"]);
    expect(ids(input({ source: source({ width: 257, height: 100 }) }))).toEqual([]);
    expect(ids(input({ source: small, palette: [ink("#000000"), ink("#ff0000", 0.3, "gradient")] }))).toEqual([]);
    // Settings that already keep one-pixel details are Icon's already.
    expect(ids(input({ source: small, settings: settings({ speckleFloor: 1, maxColours: 16 }) }))).toEqual([]);
  });

  it("offers at most two, strongest first", () => {
    const busy = input({
      source: source({ width: 1024, height: 1024, lossy: true }),
      palette: BLACK_ON_WHITE,
      losses: [loss("strokes")],
    });
    expect(ids(busy)).toEqual(["photo", "bw"]);
  });
});

describe("describeAuto and changedControls", () => {
  const list = caps(
    [preset("logo"), preset("icon", { speckleFloor: 1 })],
    [control("speckleFloor"), control("precision"), control("maxColours")],
  );

  it("counts the controls that differ between two settings", () => {
    const moved: Settings = settings({ speckleFloor: 5, precision: 0.2 });
    expect(changedControls(list, moved, settings())).toEqual(["speckleFloor", "precision"]);
    expect(changedControls(null, moved, settings())).toEqual([]);
  });

  it("names the preset Auto used, and how many controls were moved from it", () => {
    expect(describeAuto(list, settings(), "logo")).toBe("the Logo preset, the default");
    expect(describeAuto(list, settings({ speckleFloor: 1 }), "icon")).toBe("the icon preset");
    expect(describeAuto(list, settings({ precision: 0.3 }), "logo")).toBe(
      "the Logo preset, the default, with 1 control changed last time",
    );
    expect(describeAuto(list, settings(), null)).toBe("your settings from last time");
  });
});
