import { describe, expect, it } from "vitest";

import { toneOfPixels } from "./tone";

/** A 64 × 64 RGBA buffer, transparent, with the first `covered` pixels painted `rgba`. */
function canvas(covered: number, rgba: [number, number, number, number], size = 64): Uint8ClampedArray {
  const px = new Uint8ClampedArray(size * size * 4);
  for (let i = 0; i < covered; i++) px.set(rgba, i * 4);
  return px;
}

const HALF = 64 * 32;

describe("toneOfPixels", () => {
  it("asks for a light backdrop behind dark artwork on a clear ground", () => {
    expect(toneOfPixels(canvas(HALF, [0, 0, 0, 255]))).toBe("light");
  });

  it("asks for a dark backdrop behind light artwork on a clear ground", () => {
    expect(toneOfPixels(canvas(HALF, [255, 255, 255, 255]))).toBe("dark");
  });

  it("leaves a mid-tone drawing on the default backdrop", () => {
    expect(toneOfPixels(canvas(HALF, [128, 128, 128, 255]))).toBeNull();
  });

  it("reads lightness with the luma weights: pure green is light, pure blue is dark", () => {
    // 0.7152 of full scale is between the two thresholds; 0.0722 is below the dark one.
    expect(toneOfPixels(canvas(HALF, [0, 255, 0, 255]))).toBeNull();
    expect(toneOfPixels(canvas(HALF, [0, 0, 255, 255]))).toBe("light");
    expect(toneOfPixels(canvas(HALF, [255, 255, 200, 255]))).toBe("dark");
  });

  it("returns null for an empty picture, or one with too little paint to judge", () => {
    expect(toneOfPixels(canvas(0, [0, 0, 0, 255]))).toBeNull();
    expect(toneOfPixels(canvas(19, [0, 0, 0, 255]))).toBeNull();
    expect(toneOfPixels(canvas(21, [0, 0, 0, 255]))).toBe("light");
    expect(toneOfPixels(new Uint8ClampedArray(0))).toBeNull();
  });

  it("does not count nearly clear pixels, however dark", () => {
    // Alpha 12/255 is under 0.05: all of it is backdrop.
    expect(toneOfPixels(canvas(64 * 64, [0, 0, 0, 12]))).toBeNull();
  });

  it("returns null for an opaque drawing that covers its own backdrop", () => {
    expect(toneOfPixels(canvas(64 * 64, [0, 0, 0, 255]))).toBeNull();
    expect(toneOfPixels(canvas(64 * 64, [255, 255, 255, 255]))).toBeNull();
    // Just under 97% coverage still shows some backdrop, so it is still judged.
    expect(toneOfPixels(canvas(Math.floor(0.96 * 64 * 64), [0, 0, 0, 255]))).toBe("light");
    expect(toneOfPixels(canvas(Math.ceil(0.98 * 64 * 64), [0, 0, 0, 255]))).toBeNull();
  });

  it("weights each pixel by its alpha, so a soft edge counts for less than a fill", () => {
    const px = canvas(HALF, [0, 0, 0, 255]);
    // A white fringe at a quarter alpha, as large as the black fill: mean 0.2, still dark.
    for (let i = HALF; i < 2 * HALF - 64; i++) px.set([255, 255, 255, 64], i * 4);
    expect(toneOfPixels(px)).toBe("light");
  });
});
