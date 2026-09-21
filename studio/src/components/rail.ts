/**
 * The right rail.
 *
 * The order is deliberate and fixed: preset → trace/cancel and status → quality report →
 * what-was-lost → palette → advanced → Export, pinned at the bottom and reachable without
 * scrolling at every window width.
 */

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import { fill, h, icon, s } from "../lib/dom";
import { bytes, count, de00, modKey, percent, seconds, type Store } from "../lib/state";
import type { Control, Ink, Loss, Settings, Stage } from "../lib/ipc";
import { api } from "../lib/ipc";
import { closeOverlay, modal, openModal, openPopover, tip, toast } from "./overlays";

export interface RailActions {
  setPreset(id: string): void;
  savePreset(name: string): void;
  deletePreset(id: string): void;
  changeSetting(key: keyof Settings, value: Settings[keyof Settings]): void;
  resetGroup(group: string): void;
  traceNow(): void;
  cancel(): void;
  snap(from: string, to: string): void;
  openExport(): void;
  copySvg(): void;
  saveCard(): void;
  jumpToWorst(): void;
}

export function createRail(store: Store, act: RailActions): HTMLElement {
  const scroll = h("div.railscroll");
  const foot = h("div.railfoot");
  const rail = h("aside.rail", { "aria-label": "Controls" }, scroll, foot);

  const render = () => {
    const st = store.state;
    fill(
      scroll,
      presets(store, act),
      traceRow(store, act),
      st.tracing ? stageCard(store) : null,
      reportCard(store, act),
      lostCard(store),
      paletteCard(store, act),
      advancedCard(store, act),
    );
    fill(foot, ...exportFooter(store, act));
  };

  store.on(
    [
      "caps", "prefs", "preset", "settings", "tracing", "liveStages", "result", "report",
      "palette", "losses", "advancedOpen", "source", "worstCorner", "stagesOpen",
    ],
    render,
  );
  render();
  return rail;
}

// ------------------------------------------------------------------- presets ---

function presets(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  const list = st.caps?.presets ?? [];
  const saved = st.prefs?.saved ?? [];
  const mod = modKey(st.caps?.platform);
  const tile = (id: string, name: string, sub: string, remove: (() => void) | null) => {
    const el = h(
      "button.preset",
      {
        "aria-pressed": String(st.preset === id),
        title: `${name} — ${sub}`,
        onclick: () => act.setPreset(id),
      },
      h("span.name", null, name),
      h("span.sub", null, sub),
      remove
        ? h("span.forget", {
            role: "button",
            tabindex: "0",
            "aria-label": `Forget ${name}`,
            title: `Forget ${name}`,
            onclick: (e: Event) => {
              // The tile is a button; without this the click would also select the preset
              // it is on its way to deleting.
              e.stopPropagation();
              remove();
            },
          })
        : null,
    );
    return el;
  };

  return h(
    "div",
    { style: { display: "flex", flexDirection: "column", gap: "8px" } },
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, "Preset"),
      h("span.faint", { style: { fontSize: "11px" } }, `${list.length + saved.length} total`),
    ),
    h(
      "div.presets",
      null,
      ...list.map((p) => tile(p.id, p.name, p.subtitle, null)),
      ...saved.map((p) => tile(p.id, p.name, "Saved", () => act.deletePreset(p.id))),
    ),
    h(
      "div.traybar",
      null,
      h("span.faint", null, `${mod}+1–${mod}+7 switch presets`),
      h(
        "button.reset",
        {
          disabled: !st.caps,
          title: "Remember the eighteen controls exactly as they stand",
          onclick: () => saveCurrentAsPreset(store, act),
        },
        "Save current as preset",
      ),
    ),
  );
}

/**
 * Name the controls as they stand and keep them.
 *
 * The name is asked for rather than generated because a preset called "Custom 3" is a
 * preset nobody presses. The field starts on the preset the settings came from, if they
 * still match one, so the common case — a built-in nudged twice — types two words.
 */
function saveCurrentAsPreset(store: Store, act: RailActions): void {
  const from = store.state.caps?.presets.find((p) => p.id === store.state.preset);
  const field = h("input.numberfield", {
    type: "text",
    maxlength: "40",
    spellcheck: "false",
    placeholder: from ? `${from.name}, adjusted` : "Northwind, flat",
    style: { width: "100%", textAlign: "left", height: "32px", padding: "0 10px" },
  }) as HTMLInputElement;

  // Whether it was kept is the backend's answer, not this modal's, so the confirmation
  // is raised there. All this does is name it and get out of the way.
  const commit = () => {
    act.savePreset((field.value.trim() || field.placeholder).slice(0, 40));
    closeOverlay();
  };

  openModal(
    modal(
      "Save current as preset",
      [
        field,
        h(
          "span.muted",
          { style: { fontSize: "11.5px", lineHeight: "1.5" } },
          "All eighteen controls, as they stand. A saved preset is a snapshot rather than a set of differences from the defaults, so it will not drift when those move.",
        ),
      ],
      [
        h("button.btn", { onclick: closeOverlay }, "Cancel"),
        h("button.btn.primary", { onclick: commit }, "Save"),
      ],
    ),
  );
  field.focus();
  field.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") commit();
  });
}

// ------------------------------------------------------- trace / cancel row ---

function traceRow(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  const busy = st.tracing;
  return h(
    "div",
    { style: { display: "flex", gap: "8px", alignItems: "center" } },
    h(
      "button.btn",
      {
        style: { flex: "1" },
        disabled: !st.source,
        onclick: () => (busy ? act.cancel() : act.traceNow()),
      },
      busy ? "Cancel" : "Trace again",
    ),
    h(
      "button.btn",
      {
        disabled: !st.caps || !st.preset,
        onclick: () => st.preset && act.setPreset(st.preset),
        title: "Put every control back to this preset's values",
      },
      "Reset",
    ),
  );
}

// -------------------------------------------------------------- stage list ---

/**
 * The stage list, shown while a trace runs.
 *
 * Every row is a stage the engine has actually finished, with the time it took. It
 * expands automatically once a trace passes three seconds, which is the point at which a
 * single status line stops being enough information.
 */
function stageCard(store: Store): HTMLElement {
  const st = store.state;
  const all = st.caps?.stages ?? [];
  const done = new Map(st.liveStages.map((x: Stage) => [x.name, x.ms]));
  const elapsed = st.liveStages.reduce((a, x) => a + x.ms, 0) / 1000;

  return h(
    "div.card",
    null,
    h(
      "div.cardhead",
      null,
      h("span", { style: { fontSize: "12px", fontWeight: "500" } }, `Tracing at ${st.settings.traceSize} px`),
      h("span.muted.num", { style: { fontSize: "11px" } }, seconds(elapsed)),
    ),
    h("div.sweep", null, h("i")),
    h(
      "div.stagelist",
      null,
      ...all.map((name) => {
        const ms = done.get(name);
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
function reportCard(store: Store, act: RailActions): HTMLElement {
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
    // Our published 21-case average, labelled as such. It must never read as a
    // measurement of the user's own file, because we did not run VTracer on it.
    h(
      "div.benchmark",
      null,
      h("span.eyebrow.label", null, "Benchmark"),
      h(
        "p",
        null,
        "Across our published 21-case set, VTracer's defaults average 4.4× the coordinates at 10× the colour error. ",
        h("span.dim", null, "Not a measurement of this file."),
      ),
    ),
  );
}

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

// ------------------------------------------------------------ what was lost ---

/**
 * "What this trace could not recover".
 *
 * Always visible, because a panel that only ever appears to deliver bad news trains people
 * to read it as an advertisement. When nothing is detected it says so, calmly.
 */
function lostCard(store: Store): HTMLElement {
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

// ----------------------------------------------------------------- palette ---

function paletteCard(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  return h(
    "div.card",
    null,
    h(
      "div.cardhead",
      null,
      h(
        "span.eyebrow",
        { style: { display: "flex", alignItems: "center", gap: "6px" } },
        icon("palette", 12),
        `Palette · ${st.palette.length} ink${st.palette.length === 1 ? "" : "s"}`,
      ),
      h("button.reset", { disabled: !st.palette.length, onclick: () => pastePalette(store, act) }, "Paste brand palette"),
    ),
    ...st.palette.map((ink) => inkRow(ink, (target) => openSnap(target, ink, act))),
    st.palette.length
      ? h(
          "div",
          { style: { display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: "10px" } },
          h("span.muted", { style: { fontSize: "11px", lineHeight: "1.45" } }, "Snapping rewrites fills only — no re-trace."),
          h(
            "button.reset",
            {
              style: { fontSize: "11px", flex: "none" },
              title: "Custom properties, one per ink, in canvas-share order",
              onclick: () => void copyPaletteCss(st.palette),
            },
            "Copy all as CSS",
          ),
        )
      : h("span.muted", { style: { fontSize: "11px" } }, "Trace an image to see its inks."),
  );
}

function inkRow(ink: Ink, open: (el: HTMLElement) => void): HTMLElement {
  const snapped = ink.snappedDe00 !== null;
  const row = h(
    "button.ink",
    { onclick: () => open(row) },
    h(`span.swatch${snapped ? ".snapped" : ""}`, null, h("i", { style: { background: ink.hex } })),
    h("span.hex", null, ink.hex.toUpperCase()),
    h("span.share", null, percent(ink.share)),
    h(
      `span.snap${snapped ? ".on" : ""}`,
      null,
      snapped ? `snapped · ΔE ${de00(ink.snappedDe00)}` : "snap to…",
    ),
  );
  return row;
}

/** The colour-snap popover: a picker, a "snap to" field, and what it would cost. */
function openSnap(anchor: HTMLElement, ink: Ink, act: RailActions): void {
  const field = h("input.numberfield", {
    type: "text",
    value: ink.hex.toUpperCase(),
    spellcheck: "false",
    style: { width: "100%", textAlign: "left", height: "30px", padding: "0 9px" },
  }) as HTMLInputElement;
  const picker = h("input", {
    type: "color",
    value: ink.hex,
    style: { width: "100%", height: "60px", border: "none", background: "none", padding: "0" },
    oninput: () => {
      field.value = (picker as HTMLInputElement).value.toUpperCase();
    },
  });
  const note = h(
    "span",
    { style: { fontSize: "11.5px", color: "var(--gold)" } },
    "You are overriding a measurement.",
  );

  openPopover(
    anchor,
    h(
      "div",
      { style: { width: "300px", padding: "12px", display: "flex", flexDirection: "column", gap: "11px" } },
      picker,
      h(
        "div",
        { style: { display: "flex", gap: "8px", alignItems: "center" } },
        h("span.swatch", null, h("i", { style: { background: ink.traced } })),
        h(
          "div",
          { style: { display: "flex", flexDirection: "column" } },
          h("span.muted", { style: { fontSize: "11px" } }, "traced"),
          h("span.num", { style: { fontSize: "12.5px" } }, `${ink.traced.toUpperCase()} · ${percent(ink.share)} of canvas`),
        ),
      ),
      h("span.muted", { style: { fontSize: "11px" } }, "Snap to"),
      h(
        "div",
        { style: { display: "flex", gap: "6px" } },
        field,
        h(
          "button.btn.compact",
          {
            style: { background: "var(--accent)", borderColor: "var(--accent)", color: "var(--accent-ink)", fontWeight: "600" },
            onclick: () => {
              act.snap(ink.traced, field.value.trim().toLowerCase());
              closeOverlay();
            },
          },
          "Snap",
        ),
      ),
      note,
    ),
  );
}

/**
 * The palette as CSS custom properties.
 *
 * Named by position rather than by colour, because `--ink-1` survives a re-trace that
 * moves the hue and `--dark-green` does not. The share goes in a comment: it is the
 * reason the order is what it is, and it is the first thing you want when deciding which
 * of four inks is the brand colour.
 */
async function copyPaletteCss(palette: Ink[]): Promise<void> {
  const body = palette
    .map((ink, i) => `  --ink-${i + 1}: ${ink.hex.toLowerCase()}; /* ${percent(ink.share)} of canvas */`)
    .join("\n");
  try {
    await writeText(`:root {\n${body}\n}\n`);
    toast(`${palette.length} ink${palette.length === 1 ? "" : "s"} copied as CSS.`, { kind: "good" });
  } catch (e) {
    toast(String(e), { kind: "bad" });
  }
}

/** Paste a brand palette and see what each match would cost before committing. */
function pastePalette(store: Store, act: RailActions): void {
  const area = h("textarea", {
    rows: "5",
    spellcheck: "false",
    placeholder: "#12443E\n#E9B24C\nrgb(207, 198, 180)",
    style: {
      width: "100%",
      border: "1px solid var(--rule2)",
      borderRadius: "var(--radius)",
      background: "var(--paper)",
      color: "var(--ink)",
      padding: "10px 12px",
      font: "12px/1.8 var(--font-mono)",
      resize: "vertical",
    },
  }) as HTMLTextAreaElement;
  const preview = h("div", { style: { display: "flex", flexDirection: "column", gap: "7px" } });
  let matches: { from: string; to: string; de00: number }[] = [];

  const update = async () => {
    matches = await api.matchPalette(store.state.palette.map((i) => i.traced), area.value);
    fill(
      preview,
      ...matches.map((m) =>
        h(
          "div",
          { style: { display: "flex", alignItems: "center", gap: "9px", fontSize: "12px" }, class: "num" },
          h("span.swatch", { style: { width: "20px", height: "20px" } }, h("i", { style: { background: m.from } })),
          h("span.muted", null, m.from.toUpperCase()),
          h("span.muted", null, "→"),
          h("span.swatch", { style: { width: "20px", height: "20px" } }, h("i", { style: { background: m.to } })),
          h("span.dim", null, m.to.toUpperCase()),
          h(
            "span",
            { style: { marginLeft: "auto", color: m.de00 < 0.5 ? "var(--good)" : "var(--gold)" } },
            m.de00 < 0.05 ? "exact" : `${de00(m.de00)} dE00`,
          ),
        ),
      ),
    );
  };
  area.addEventListener("input", () => void update());

  openModal(
    modal(
      "Paste a brand palette",
      [
        area,
        h("span.muted", { style: { fontSize: "11.5px" } }, "Hex, RGB or CSS variables, one per line. We match each to the nearest traced ink."),
        preview,
      ],
      [
        h("button.btn", { onclick: closeOverlay }, "Cancel"),
        h(
          "button.btn.primary",
          {
            onclick: () => {
              for (const m of matches) act.snap(m.from, m.to);
              closeOverlay();
            },
          },
          `Snap ${store.state.palette.length} inks`,
        ),
      ],
    ),
  );
}

// -------------------------------------------------------- advanced drawer ---

function advancedCard(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  const controls = st.caps?.controls ?? [];
  if (!st.advancedOpen) {
    return h(
      "button.card",
      {
        style: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", padding: "11px 13px" },
        onclick: () => store.set({ advancedOpen: true }),
      },
      h("span.dim", { style: { fontSize: "12.5px" } }, `Advanced · ${controls.length} controls`),
      h("span.muted", { style: { fontSize: "12px" } }, "Show"),
    );
  }

  const groups = [...new Set(controls.map((c) => c.group))];
  return h(
    // `.drawer` is what the narrow-window rule targets: below 1400 px this becomes a
    // bottom sheet over the stage so Export keeps its place at the foot of the rail.
    "div.card.drawer",
    null,
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, `Advanced · ${controls.length} controls`),
      h("button.reset", { onclick: () => store.set({ advancedOpen: false }) }, "Hide"),
    ),
    ...groups.map((g) =>
      h(
        "div.group",
        null,
        h(
          "div.grouphead",
          null,
          h("span.eyebrow", null, g),
          h("button.reset", { onclick: () => act.resetGroup(g) }, "Reset group"),
        ),
        ...controls.filter((c) => c.group === g).map((c) => controlRow(store, c, act)),
      ),
    ),
  );
}

/**
 * One row of the drawer: a plain-words label, its unit, the value shown numerically, and
 * a slider on a perceptual scale with named stops rather than a bare track.
 */
function controlRow(store: Store, c: Control, act: RailActions): HTMLElement {
  const value = store.state.settings[c.key];

  if (c.kind === "switch") {
    const sw = h("button.switch", {
      role: "switch",
      "aria-checked": String(Boolean(value)),
      "aria-label": c.label,
      onclick: () => act.changeSetting(c.key, !value as never),
    });
    return h(
      "div.control",
      null,
      h("div.controlhead", null, tip(h("span.label", { tabindex: "0" }, c.label), c.help), sw),
    );
  }

  if (c.kind === "tri") {
    const options: [string, string][] = [["off", "Off"], ["auto", "Auto"], ["on", "On"]];
    return h(
      "div.control",
      null,
      h(
        "div.controlhead",
        null,
        tip(h("span.label", { tabindex: "0" }, c.label), c.help),
        h(
          "div.seg",
          null,
          ...options.map(([id, label]) =>
            h(
              "button",
              { "aria-pressed": String(value === id), onclick: () => act.changeSetting(c.key, id as never) },
              label,
            ),
          ),
        ),
      ),
    );
  }

  // A power scale: precision runs 0.02–0.5 and is perceptually nowhere near linear,
  // so the slider's position is the value raised to the control's own curve.
  const toSlider = (v: number) => Math.pow((Number(v) - c.min) / (c.max - c.min || 1), 1 / c.curve) * 1000;
  const fromSlider = (p: number) => c.min + Math.pow(p / 1000, c.curve) * (c.max - c.min);
  const show = (v: number) => (c.decimals === 0 ? String(Math.round(v)) : v.toFixed(c.decimals));

  const field = h("input.numberfield", {
    type: "text",
    value: show(Number(value)),
    inputmode: "decimal",
    "aria-label": `${c.label}${c.unit ? ` in ${c.unit}` : ""}`,
    onchange: (e: Event) => {
      const n = Number((e.target as HTMLInputElement).value);
      if (Number.isFinite(n)) act.changeSetting(c.key, Math.min(c.max, Math.max(c.min, n)) as never);
    },
  }) as HTMLInputElement;

  const slider = h("input.slider", {
    type: "range",
    min: "0",
    max: "1000",
    step: "1",
    value: String(Math.round(toSlider(Number(value)))),
    "aria-label": c.label,
    "aria-valuetext": `${show(Number(value))} ${c.unit}`.trim(),
    oninput: (e: Event) => {
      const v = fromSlider(Number((e.target as HTMLInputElement).value));
      const rounded = c.decimals === 0 ? Math.round(v) : Number(v.toFixed(c.decimals));
      field.value = show(rounded);
      act.changeSetting(c.key, rounded as never);
    },
  });

  return h(
    "div.control",
    null,
    h(
      "div.controlhead",
      null,
      tip(h("span.label", { tabindex: "0" }, c.label), c.help),
      c.unit ? h("span.unit", null, c.unit) : null,
      field,
    ),
    slider,
    c.stops.length ? h("div.stops", null, ...c.stops.map((x) => h("span", null, x.label))) : null,
  );
}

// ------------------------------------------------------------------ export ---

function exportFooter(store: Store, act: RailActions): HTMLElement[] {
  const ready = Boolean(store.state.svg) && !store.state.tracing;
  return [
    h(
      "div.row",
      null,
      h("button.btn.primary", { style: { flex: "1" }, disabled: !ready, onclick: act.openExport }, "Export"),
      h("button.btn", { disabled: !ready, onclick: act.copySvg, title: "Paste straight into Figma or Illustrator" }, "Copy SVG"),
      h("button.btn.icon", { disabled: !ready, onclick: act.saveCard, "aria-label": "Save comparison card" }, icon("share", 16)),
    ),
    h(
      "div.note",
      null,
      h("span", null, "Export runs a fresh full trace."),
      h("button.reset", { disabled: !ready, onclick: act.saveCard }, "Save card"),
    ),
  ];
}

/** A tiny helper the export sheet and the batch bar both want. */
export function pill(label: string, onclick: () => void, pressed?: boolean): HTMLElement {
  return h("button.btn.compact", { "aria-pressed": pressed === undefined ? null : String(pressed), onclick }, label);
}

/** Let other modules raise a toast without importing the overlay module directly. */
export { toast };

/** Re-exported for the export sheet, which lives in its own module. */
export { s };
