import { describe, expect, it } from "vitest";

import { firstFailure, OPENS, plainMessage } from "./workspace";

describe("a file that did not open, as the empty state says it", () => {
  const core = "Error: This file is not an image format the tracer reads: The image format could not be determined. PNG, JPEG, WebP, BMP, GIF and TIFF are supported.";

  it("says why without the thrown error's prefix, and without listing the formats twice", () => {
    expect(plainMessage(core, true)).toBe("This file is not an image format the tracer reads: The image format could not be determined.");
    // Where the screen does not list the formats itself, the core's sentence stays.
    expect(plainMessage(core)).toBe(core.slice("Error: ".length));
    expect(plainMessage("Error: Error: boom")).toBe("boom");
  });

  it("leads the empty state for an undecodable file, a failed trace and an out-of-memory one", () => {
    expect(firstFailure({ kind: "undecodable", message: core })?.title).toBe("That file did not open");
    expect(firstFailure({ kind: "failed", message: "unreachable" })).toEqual({ title: "The trace stopped", body: "unreachable" });
    expect(firstFailure({ kind: "outOfMemory", neededGb: 5.25, suggestPx: 2048 })?.body).toContain("5.3 GB");
  });

  it("says nothing for the states that are not a failure", () => {
    for (const kind of ["empty", "decoding", "drawing", "cancelled", "flat", "denoiserMissing"] as const) {
      expect(firstFailure({ kind } as never)).toBeNull();
    }
  });

  it("names every format the open dialog accepts", () => {
    for (const f of ["PNG", "JPEG", "WebP", "GIF", "BMP", "TIFF", "SVG"]) expect(OPENS).toContain(f);
  });
});
