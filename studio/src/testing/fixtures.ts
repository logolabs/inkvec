/**
 * Small builders for the unit tests (`*.test.ts`): a store in its starting state, and the
 * backend's records (inks, sources, reports, controls, presets) with only the fields a test
 * cares about spelt out. Never imported by the app itself.
 */

import type { Capabilities, Control, Ink, Loss, PresetInfo, Prefs, Report, Settings, SourceFacts, SourceInfo } from "../lib/ipc";
import { DEFAULT_SETTINGS } from "../lib/presets";
import { initial, Store } from "../lib/state";

/** The Minify tab's settings the app starts with (`main.ts`). */
export const MINIFY = { tolerancePx: 0.1, judgePx: 1024, cornerDegrees: 30, documentCleanup: true };

/** The default controls, with `patch` over them. */
export function settings(patch: Partial<Settings> = {}): Settings {
  return { ...DEFAULT_SETTINGS, ...patch };
}

/** A store in the state the app starts in. */
export function freshStore(): Store {
  return new Store(initial(settings(), MINIFY));
}

/** A flat ink of colour `hex` covering `share` of the canvas. */
export function ink(hex: string, share = 0.4, kind: Ink["kind"] = "flat"): Ink {
  return { traced: hex, hex, share, snappedDe00: null, kind, keys: [hex], stops: [hex] };
}

/** An opened image: a 512 px square PNG unless `patch` says otherwise. */
export function source(patch: Partial<SourceInfo> = {}): SourceInfo {
  return { name: "logo.png", path: null, width: 512, height: 512, container: "PNG", lossy: false, preview: "", ...patch };
}

/** What the noise estimator says about an image: clean and opaque unless told otherwise. */
export function facts(patch: Partial<SourceFacts> = {}): SourceFacts {
  return { noiseLevels: 0.5, hasAlpha: false, clearShare: 0, ...patch };
}

/** A trace's report; the figures are placeholders except where `patch` sets them. */
export function report(patch: Partial<Report> = {}): Report {
  return {
    meanDe00: 0.1,
    medianDe00: 0.05,
    worstDe00: 2,
    coordinates: 100,
    paths: 3,
    segments: 40,
    colours: 2,
    bytes: 2048,
    minifiedBytes: null,
    structure: {} as Report["structure"],
    seconds: 0.4,
    tracedPx: 512,
    ...patch,
  };
}

/** A loss of the given kind, as the report's "could not recover" list carries it. */
export function loss(kind: string): Loss {
  return { kind, text: kind, why: "", link: null };
}

/** A slider control for `key`, read by the engines `modes` says. */
export function control(key: keyof Settings, modes: Control["modes"] = "both", group: Control["group"] = "Detail"): Control {
  return { group, key, label: String(key), unit: "", kind: "range", min: 0, max: 1, curve: 1, decimals: 2, stops: [], help: "", modes };
}

/** A built-in preset with the default controls and `patch` over them. */
export function preset(id: PresetInfo["id"], patch: Partial<Settings> = {}, wantsDenoiser = false): PresetInfo {
  return { id, name: id, subtitle: "", settings: settings(patch), wantsDenoiser };
}

/** Capabilities with the given presets and controls, and nothing else of note. */
export function caps(presets: PresetInfo[] = [], controls: Control[] = []): Capabilities {
  return {
    version: "0",
    engineVersion: "0",
    buildTarget: "test",
    platform: "windows",
    controls,
    presets,
    stages: [],
    denoiser: { supported: false, installed: false, path: null, bytes: null } as Capabilities["denoiser"],
  };
}

/** Preferences as the backend would send them, with `patch` over the defaults. */
export function prefs(patch: Partial<Prefs> = {}): Prefs {
  return {
    schema: 1,
    outputFolder: null,
    theme: "system",
    threads: null,
    draftPx: 512,
    draftSeconds: 1,
    settleMs: 800,
    checkUpdates: false,
    channel: "stable",
    trace: settings(),
    recent: [],
    seenFirstRun: true,
    onOpen: "ask",
    saved: [],
    ...patch,
  };
}
