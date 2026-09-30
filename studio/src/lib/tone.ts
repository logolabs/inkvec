/**
 * Which backdrop a drawing can be seen on, decided from its rendered pixels.
 *
 * Black artwork on a transparent ground vanishes on the dark stage, and white artwork on a
 * light one. `toneOf` in `views/minify.ts` renders an SVG small onto a canvas and hands the
 * pixels here; the viewer and the Minify tab then put a light or dark backdrop behind the
 * preview. The SVG itself is never changed. Pure, so tested on its own (`tone.test.ts`).
 */

/** Pixels more transparent than this (alpha, 0..1) are backdrop and are not counted. */
const CLEAR_ALPHA = 0.05;

/** Below this much total coverage (in whole pixels' worth of alpha) there is nothing to judge. */
const MIN_WEIGHT = 20;

/** Coverage above this share of the canvas means the drawing hides its backdrop entirely. */
const OPAQUE_SHARE = 0.97;

/** Mean luminance (0..1) below which the drawing is dark and wants a light backdrop. */
const DARK_BELOW = 0.3;

/** Mean luminance (0..1) above which the drawing is light and wants a dark backdrop. */
const LIGHT_ABOVE = 0.8;

/**
 * The backdrop a drawing needs, from its pixels: `"light"` for dark artwork, `"dark"` for
 * light artwork, `null` when the default backdrop is fine.
 *
 * `rgba` is canvas image data: four bytes (0..255) per pixel, red, green, blue, alpha, not
 * premultiplied. The idea is an alpha-weighted mean of the drawing's luminance, so a soft
 * edge counts for less than a solid fill:
 *
 *     W = Σ_i a_i,     Y = (1 / W) · Σ_i a_i · (0.2126 R_i + 0.7152 G_i + 0.0722 B_i) / 255
 *
 * over the pixels whose alpha `a_i` (0..1) is at least 0.05, with R, G, B the 8-bit channel
 * values. The weights are the Rec. 709 luma coefficients, applied to the gamma-encoded
 * values as they are: a quick lightness reading, not a colorimetric one, which is all a
 * backdrop choice needs. Y below 0.3 asks for a light backdrop, above 0.8 for a dark one.
 *
 * `null` in three cases besides a mid-tone drawing: an empty or nearly empty picture
 * (W under 20 pixels' worth), an empty buffer, and a drawing that covers more than 97% of
 * the canvas with opaque paint. That last one paints its own ground, so whatever is behind
 * it does not show and no backdrop needs choosing.
 */
export function toneOfPixels(rgba: ArrayLike<number>): "light" | "dark" | null {
  const pixels = rgba.length / 4;
  let weight = 0;
  let light = 0;
  for (let i = 0; i + 3 < rgba.length; i += 4) {
    const a = rgba[i + 3] / 255;
    if (a < CLEAR_ALPHA) continue;
    weight += a;
    light += (a * (0.2126 * rgba[i] + 0.7152 * rgba[i + 1] + 0.0722 * rgba[i + 2])) / 255;
  }
  // An opaque drawing covers its own backdrop, so no backdrop needs choosing for it.
  if (weight < MIN_WEIGHT || weight > OPAQUE_SHARE * pixels) return null;
  const mean = light / weight;
  return mean < DARK_BELOW ? "light" : mean > LIGHT_ABOVE ? "dark" : null;
}
