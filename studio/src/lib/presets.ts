/**
 * The trace controls' starting values, and the presets that set them.
 *
 * A preset is either one of the ten built into the engine (`Capabilities.presets`, from the
 * core's `options::Preset`) or one the user saved (`Prefs.saved`, a whole snapshot of the
 * controls). Both are named by an id string; these helpers turn an id, or a keyboard
 * shortcut, into the preset it means. Used by `main.ts`; pure, so tested on their own
 * (`presets.test.ts`).
 */

import type { Capabilities, Prefs, Settings } from "./ipc";

/**
 * The controls before the preferences have loaded (the remembered ones then replace them),
 * and what a reset returns to when no preset is selected.
 */
export const DEFAULT_SETTINGS: Settings = {
  mode: "quality",
  precision: 0.1,
  speckleFloor: 2,
  traceSize: 2048,
  timeLimit: 0,
  maxColours: 64,
  colourMerging: 0.035,
  flatFills: false,
  blackAndWhite: false,
  monochrome: false,
  cleanUpDamage: "off",
  matchRepeatedShapes: true,
  matchThreshold: 0.92,
  fewerPaths: false,
  lineArt: false,
  repairRings: true,
  editability: false,
  bezierCost: 6,
  cornerAngle: 10,
  minify: false,
  transparentBackground: false,
  margin: 0,
  holesAsCutouts: false,
  traceTransparency: true,
};

/**
 * A preset id to the settings it means, whether it is one of the built-in presets or one
 * the user saved. Returns `null` for an id that no longer exists: a saved preset can be
 * deleted while it is the selected one. Only a built-in preset can want the denoiser.
 */
export function resolvePreset(
  caps: Pick<Capabilities, "presets"> | null,
  prefs: Pick<Prefs, "saved"> | null,
  id: string,
): { settings: Settings; wantsDenoiser: boolean } | null {
  const builtin = caps?.presets.find((p) => p.id === id);
  if (builtin) return { settings: builtin.settings, wantsDenoiser: builtin.wantsDenoiser };
  const saved = prefs?.saved.find((p) => p.id === id);
  return saved ? { settings: saved.settings, wantsDenoiser: false } : null;
}

/**
 * Which built-in preset Ctrl (⌘ on a Mac) plus `key` selects, as an index into
 * `Capabilities.presets`: the keys 1 to 9 the first nine, in the order the rail shows them,
 * and 0 the tenth, as on a keyboard's number row. `null` for a key that is not a digit.
 */
export function presetIndexForKey(key: string): number | null {
  return /^[0-9]$/.test(key) ? (Number(key) + 9) % 10 : null;
}
