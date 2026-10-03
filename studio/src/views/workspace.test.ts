import { describe, expect, it } from "vitest";

import type { Traced } from "../lib/ipc";
import { freshStore, report, source } from "../testing/fixtures";
import { firstFailure, OPENS, plainMessage, stripWords } from "./workspace";

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
    expect(firstFailure({ kind: "undecodable", message: core })?.formats).toBe(true);
    expect(firstFailure({ kind: "failed", message: "unreachable" })).toEqual({ title: "The trace stopped", body: "unreachable", formats: false });
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

describe("the status strip names the engine that drew what is shown", () => {
  const traced = (tier: "draft" | "final") =>
    ({ tier, svg: "<svg/>", report: report(), palette: [], losses: [], worstCorner: null, stages: [], engineLog: [], bands: null, tracedPx: 512, oversized: false, sourcePx: [2048, 2048] }) as Traced;

  it("calls the Fast draft of an image just opened a Fast draft while the controls say Quality", () => {
    const st = freshStore();
    st.set({ source: source({ width: 2048, height: 2048 }), report: report(), result: traced("draft"), resultEngine: "fast" });
    expect(st.state.settings.mode).toBe("quality");
    expect(stripWords(st.state).lead).toBe("Fast draft");
  });

  it("names the Quality final that follows it, and the engine of a trace in flight", () => {
    const st = freshStore();
    st.set({ source: source(), report: report({ seconds: 2.5 }), result: traced("final"), resultEngine: "quality" });
    expect(stripWords(st.state).lead).toMatch(/^Quality in /);
    st.set({ tracing: true, tracingEngine: "fast", tracingTier: "draft" });
    expect(stripWords(st.state).rest).toMatch(/^· fast · /);
    st.set({ tracingEngine: "quality", tracingTier: "final" });
    expect(stripWords(st.state).rest).toMatch(/^· quality · /);
  });

  it("says Ready with nothing open, and Opened with an image but no trace", () => {
    const st = freshStore();
    expect(stripWords(st.state).lead).toBe("Ready");
    st.set({ source: source() });
    expect(stripWords(st.state)).toMatchObject({ lead: "Opened", rest: "· 512 × 512 · PNG" });
  });
});
