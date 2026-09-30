/**
 * The Result half of the rail, card by card: the stage list while a trace runs, the quality
 * report, how editable the drawing is, what the trace could not recover, and the notes shown
 * before any image is open. The palette card has its own module (`palette.ts`), and the
 * "Auto chose" card is `autochose.ts`.
 *
 * Each card is a function of the store, rebuilt whole by `rail.ts` when a key it reads
 * changes; none keeps state of its own.
 */

import { h, icon } from "../lib/dom";
import { bytes, count, de00, percent, plannedTracePx, seconds, type Store } from "../lib/state";
import type { Loss, Settings, Stage } from "../lib/ipc";
import { elapsedText, stepText } from "../lib/live";

/** What the cards ask the app to do: the rail passes its own actions. */
export interface CardActions {
  jumpToWorst(): void;
  changeSetting(key: keyof Settings, value: Settings[keyof Settings]): void;
}

/** What hand-drawn files do, from the 1,544 artist-drawn SVGs in the evaluation corpus. */
const ARTIST = { axisHandles: 0.34, smoothJoins: 0.89, alignedNodes: 0.86 } as const;

/** Before any image is open there is nothing to report, and a card of dashes says so badly. */
export function emptyResult(): HTMLElement {
  return h(
    "div.card.emptycard",
    null,
    h("span.eyebrow", null, "Quality report"),
    h(
      "p",
      null,
      "Open an image and its report lands here: the measured colour difference, what the drawing cost in coordinates, how editable it is, what could not be recovered, and its palette.",
    ),
    h("span.faint", null, "The controls are under Tune; they apply to whatever you open next."),
  );
}

/** The one line that puts our numbers beside someone else's, kept apart from the measurements. */
export function benchmarkNote(): HTMLElement {
  // Our published 21-case average, labelled as such. It must never read as a
  // measurement of the user's own file, because we did not run VTracer on it.
  return h(
    "div.benchmark",
    null,
    h("span.eyebrow.label", null, "Benchmark"),
    h(
      "p",
      null,
      "Across our published 21-case set, VTracer's defaults average 4.4× the coordinates at 10× the colour error. ",
      h("span.dim", null, "Not a measurement of this file."),
    ),
  );
}

// -------------------------------------------------------------- stage list ---

/**
 * The stage list, shown at the top of the Result half while a trace runs.
 *
 * One row per stage the engine has (`Capabilities.stages`): a tick and the time it took for
 * each stage the engine has finished, the running one with its loop count and its own
 * clock, and a dot for those still to come. The clocks are written in place by
 * `startClock` (`lib/live.ts`), not by rebuilding the card.
 */
export function stageCard(store: Store): HTMLElement {
  const st = store.state;
  const all = st.caps?.stages ?? [];
  const done = new Map(st.liveStages.map((x: Stage) => [x.name, x.ms]));
  const now = st.liveNow;

  return h(
    "div.card",
    null,
    h(
      "div.cardhead",
      null,
      h("span", { style: { fontSize: "12px", fontWeight: "500" } }, `Tracing at ${plannedTracePx(st, st.tracingTier) ?? "—"} px`),
      h("span.muted.num", { style: { fontSize: "11px" }, "data-live": "elapsed" }, elapsedText(st)),
    ),
    h("div.sweep", null, h("i")),
    h(
      "div.stagelist",
      null,
      ...all.map((name) => {
        const ms = done.get(name);
        // Running now: shown the moment the engine starts it, with its own clock.
        if (now?.stage === name) {
          return h(
            "div.stagerow.running",
            null,
            h("span.mark", null, "›"),
            h("span.name", null, name, h("span.sub", null, now.step ? stepText(now.step) : now.what)),
            h("span.num", { "data-live": "stage" }),
          );
        }
        return h(
          `div.stagerow${ms === undefined ? "" : ".done"}`,
          null,
          h("span.mark", null, ms === undefined ? "·" : "✓"),
          h("span.name", null, name),
          h("span.muted", null, ms === undefined ? "—" : `${ms.toFixed(0)} ms`),
        );
      }),
    ),
  );
}

// ----------------------------------------------------------- quality report ---

/**
 * The quality report.
 *
 * An instrument readout: monospace figures, units, restrained colour, and a scale beside
 * the headline number showing where "invisible" sits. Never a score badge — the
 * credibility is the point.
 */
export function reportCard(store: Store, act: Pick<CardActions, "jumpToWorst">): HTMLElement {
  const st = store.state;
  const r = st.report;
  const scope = !r ? "—" : st.result?.tier === "draft" ? `draft · ${r.tracedPx} px` : `full trace · ${r.tracedPx} px`;

  // The meter runs 0–5 dE00 on a square-root scale: a good trace lands near 0.1 and a
  // linear axis would put every real result in the first two percent of the track.
  const position = (v: number) => `${(Math.sqrt(Math.min(v, 5) / 5) * 100).toFixed(1)}%`;

  return h(
    "div.card",
    { style: { opacity: st.tracing ? "0.45" : "1", transition: "opacity var(--d-fast)" } },
    h("div.cardhead", null, h("span.eyebrow", null, "Quality report"), h("span.muted", { style: { fontSize: "11px" } }, scope)),
    h(
      "div.headline",
      null,
      h("span.figure", null, de00(r?.meanDe00)),
      h(
        "div.of",
        null,
        h("span.dim", { style: { fontSize: "11.5px" } }, "mean colour difference"),
        h("span.muted.num", { style: { fontSize: "11px" } }, `median ${de00(r?.medianDe00)} · dE00`),
      ),
    ),
    h(
      "div",
      { style: { display: "flex", flexDirection: "column", gap: "5px" } },
      h(
        "div.meter",
        { role: "img", "aria-label": `Colour difference ${de00(r?.meanDe00)} of a 0 to 5 scale` },
        h("span.threshold", { style: { left: position(1) } }),
        r?.meanDe00 != null ? h("span.needle", { style: { left: position(r.meanDe00) } }) : null,
      ),
      h(
        "div.meterscale",
        null,
        h("span", null, "0 invisible"),
        h("span", null, "1.0 just visible"),
        h("span", null, "5.0"),
      ),
    ),
    h(
      "div.stats",
      null,
      ...statPairs(store).map(([k, v]) => h("div", null, h("span.k", null, k), h("span.v", null, v))),
    ),
    r && st.worstCorner
      ? h(
          "button.reset",
          { style: { textAlign: "left" }, onclick: act.jumpToWorst },
          `Find the worst corner · ${de00(st.worstCorner.de00)} dE00 →`,
        )
      : null,
  );
}

/** The report's figures as label and value pairs, dashes before there is a report. */
function statPairs(store: Store): [string, string][] {
  const r = store.state.report;
  if (!r) return [["coordinates", "—"], ["paths", "—"], ["colours found", "—"], ["segments", "—"], ["file size", "—"], ["minified", "—"]];
  return [
    ["coordinates", count(r.coordinates)],
    ["paths", count(r.paths)],
    ["colours found", count(r.colours)],
    ["segments", count(r.segments)],
    ["file size", bytes(r.bytes)],
    ["minified", r.minifiedBytes === null ? "already" : bytes(r.minifiedBytes)],
    ["traced at", `${r.tracedPx} px`],
    ["time taken", seconds(r.seconds)],
  ];
}

// ----------------------------------------------------------- editability ---

/**
 * How editable the drawing is: the three habits of hand-drawn vector files, counted on
 * this one, each beside what artists' own files do.
 *
 * A traced file is fitted for pixels alone, so it starts near zero on all three; the
 * editable-structure control moves them, and this is where that is shown rather than
 * claimed. The reference tick is the median of 1,544 artist-drawn SVGs — measured with
 * the same function, `inkvec_svgmin::structure`, so the two are the same kind of number.
 */
export function structureCard(store: Store, act: Pick<CardActions, "changeSetting">): HTMLElement | null {
  const st = store.state;
  const m = st.report?.structure;
  if (st.report && !m) return null;

  const on = st.settings.editability;
  const rows: { label: string; help: string; part: number; whole: number; artist: number }[] = m
    ? [
        {
          label: "Handles on an axis",
          help: "Curve handles that point exactly along x or y, as an artist places them with the keyboard. Counted over every cubic handle in the drawing.",
          part: m.axisHandles,
          whole: m.handles,
          artist: ARTIST.axisHandles,
        },
        {
          label: "Smooth joins",
          help: "Places where two curves meet with their tangents within a degree of each other, so the outline has no kink for a hand to trip on.",
          part: m.smoothJoins,
          whole: m.joins,
          artist: ARTIST.smoothJoins,
        },
        {
          label: "Nodes sharing a coordinate",
          help: "Nodes with the same x or the same y as another node, so a group of them can be selected and aligned in one move.",
          part: m.alignedNodes,
          whole: m.nodes,
          artist: ARTIST.alignedNodes,
        },
      ]
    : [];

  // A drawing of lines and arcs has no curve handles to be on an axis and no two curves
  // to join, and an empty bar for each would read as a failure rather than as not applying.
  const measured = rows.filter((r) => r.whole > 0);
  const curveless = m !== undefined && m.cubics === 0;

  return h(
    "div.card.structure",
    { style: { opacity: st.tracing ? "0.45" : "1", transition: "opacity var(--d-fast)" } },
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, "Editability"),
      h(
        "button.reset",
        {
          title: on
            ? "Stop moving nodes and handles onto what an artist would draw"
            : "Move nodes and handles onto what an artist would draw. Costs about 0.04 dE00 on icons, and the report above shows the price on this image.",
          onclick: () => act.changeSetting("editability", !on),
        },
        on ? "Editable structure on · turn off" : "Make it editable",
      ),
    ),
    ...measured.map((r) => {
      const share = r.part / r.whole;
      return h(
        "div.srow",
        { title: r.help },
        h("span.k", null, r.label),
        h("span.v", null, percent(share), h("span.of", null, `${r.part} of ${r.whole}`)),
        h(
          "div.sbar",
          { role: "img", "aria-label": `${r.label}: ${percent(share)}, ${r.part} of ${r.whole}; hand-drawn files ${percent(r.artist)}` },
          h("i", { style: { width: `${(share * 100).toFixed(1)}%` } }),
          h("b", { style: { left: `${(r.artist * 100).toFixed(1)}%` } }),
        ),
      );
    }),
    curveless
      ? h(
          "span.muted",
          { style: { fontSize: "11px", lineHeight: "1.45" } },
          "This drawing is lines and arcs, so there are no curve handles to tidy; only its nodes can line up.",
        )
      : null,
    h(
      "span.muted",
      { style: { fontSize: "11px", lineHeight: "1.45" } },
      "The tick is where hand-drawn files sit (median of 1,544 artist SVGs). ",
      on
        ? "Nothing moves further than the trace's own tolerance."
        : "A trace fitted for pixels alone starts near zero on all three.",
    ),
  );
}

// ------------------------------------------------------------ what was lost ---

/**
 * "What this trace could not recover".
 *
 * Always visible, because a panel that only ever appears to deliver bad news trains people
 * to read it as an advertisement. When nothing is detected it says so, calmly.
 */
export function lostCard(store: Store): HTMLElement {
  const st = store.state;
  const glyphFor = (kind: string) =>
    ({ lettering: "type", lossy: "image", strokes: "wand", ramp: "droplet", detail: "layers" })[kind] ?? "info";

  return h(
    "div.card",
    null,
    h("span.eyebrow", null, "What this trace could not recover"),
    ...st.losses.map((l: Loss) => lossRow(l, glyphFor(l.kind))),
    h(
      "div.nothinglost",
      null,
      h("span.glyph", null, icon("checkCircle", 15)),
      h("span", null, st.losses.length ? "Nothing else we can detect was lost." : "Nothing we can detect was lost."),
    ),
  );
}

/** One thing the trace could not recover: its icon, the sentence, a Why that unfolds the reason, and at most one link. */
function lossRow(l: Loss, glyph: string): HTMLElement {
  const why = h("div.why", { hidden: true }, l.why);
  const toggle = h(
    "button.expand",
    {
      "aria-expanded": "false",
      onclick: () => {
        const open = why.hasAttribute("hidden");
        why.toggleAttribute("hidden", !open);
        toggle.setAttribute("aria-expanded", String(open));
        toggle.textContent = open ? "Hide" : "Why";
      },
    },
    "Why",
  );
  return h(
    "div.loss",
    null,
    h("span.glyph", null, icon(glyph, 15)),
    h(
      "div",
      { style: { display: "flex", flexDirection: "column", gap: "3px", minWidth: "0" } },
      h("span.text", null, l.text),
      h(
        "div.meta",
        null,
        toggle,
        // At most one text link, at the same weight as everything else. Never a
        // button, never accent-filled: the moment one looks like an ad, the honesty
        // that makes this panel work is gone.
        l.link ? h("a", { href: l.link.href, "data-external": l.link.href.startsWith("http") ? "1" : null }, l.link.label) : null,
      ),
      why,
    ),
  );
}
