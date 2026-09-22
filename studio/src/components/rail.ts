/**
 * The right rail.
 *
 * Two halves, chosen by a switch at the top: **Result** is what came out — the quality
 * report, how editable it is, what could not be recovered, the palette — and **Tune** is
 * what makes it: the presets and every control. They are separate because they are used
 * at different moments; a single column of both put the controls a thousand pixels from
 * the number they move. What they share is pinned at the foot: a live readout of the
 * three figures that matter, with the change since the last full trace beside each, and
 * Export, which stays reachable at every window width.
 */

import { writeText } from "@tauri-apps/plugin-clipboard-manager";

import { fill, h, icon, s } from "../lib/dom";
import { bytes, count, de00, modKey, percent, plannedTracePx, seconds, type Store } from "../lib/state";
import type { Control, Ink, Loss, Report, Settings, Stage } from "../lib/ipc";
import { api } from "../lib/ipc";
import { closeOverlay, modal, openModal, openPopover, tip, toast } from "./overlays";

export interface RailActions {
  setPreset(id: string): void;
  savePreset(name: string): void;
  deletePreset(id: string): void;
  changeSetting(key: keyof Settings, value: Settings[keyof Settings]): void;
  resetGroup(group: string): void;
  /** The settings the controls are measured against: the selected preset's, else the defaults. */
  baseSettings(): Settings;
  traceNow(): void;
  cancel(): void;
  snap(from: string, to: string): void;
  openExport(): void;
  copySvg(): void;
  saveCard(): void;
  jumpToWorst(): void;
  /** Open the denoiser's download dialog. */
  openDenoiser(): void;
}

/**
 * The two settings that are promoted out of the control groups into the modes block at the
 * top of the rail. They are the two things people most often come for, so they are not left
 * three folds down a list; and each is drawn once, so a setting never has two controls.
 */
const PROMOTED: ReadonlySet<string> = new Set(["cleanUpDamage", "editability"]);

/** What hand-drawn files do, from the 1,544 artist-drawn SVGs in the evaluation corpus. */
const ARTIST = { axisHandles: 0.34, smoothJoins: 0.89, alignedNodes: 0.86 } as const;

export function createRail(store: Store, act: RailActions): HTMLElement {
  const tabs = h("div.railtabs");
  const modes = h("div.railmodes");
  const scroll = h("div.railscroll");
  const foot = h("div.railfoot");
  const rail = h("aside.rail", { "aria-label": "Controls" }, tabs, modes, scroll, foot);

  // Each half keeps its own place: switching to Tune and back must not lose where the
  // palette was scrolled to.
  const scrolled: Record<string, number> = { result: 0, tune: 0 };
  let shown: string = store.state.railTab;

  /** The half that is showing, rebuilt. Keeps the scroll place and the keyboard focus. */
  const renderScroll = () => {
    const st = store.state;
    // A control's element is replaced when it changes, which would drop keyboard focus from
    // the very control someone is adjusting; put it back on its twin afterwards.
    const focused =
      document.activeElement instanceof HTMLElement && scroll.contains(document.activeElement)
        ? document.activeElement.getAttribute("data-ctl")
        : null;
    scrolled[shown] = scroll.scrollTop;

    fill(
      scroll,
      ...(st.railTab === "result"
        ? [st.tracing ? stageCard(store) : null, ...resultPane(store, act)]
        : tunePane(store, act)),
    );
    shown = st.railTab;
    scroll.scrollTop = scrolled[shown] ?? 0;
    if (focused) scroll.querySelector<HTMLElement>(`[data-ctl="${focused}"]`)?.focus({ preventScroll: true });
  };

  /**
   * The pinned foot. While a trace runs it is updated in place rather than rebuilt: a stage
   * finishes every few tenths of a second, and a Cancel button that is replaced between the
   * press and the release never hears the click.
   */
  const renderFoot = () => {
    const st = store.state;
    const busy = foot.querySelector<HTMLElement>(".readout.busy");
    if (st.tracing && busy) {
      const last = st.liveStages[st.liveStages.length - 1];
      busy.querySelector(".stage")!.textContent = last?.name ?? "starting";
      busy.querySelector(".elapsed")!.textContent = seconds(st.liveStages.reduce((a: number, x: Stage) => a + x.ms, 0) / 1000);
      return;
    }
    fill(foot, ...footer(store, act));
  };

  const renderModes = () => {
    const focused =
      document.activeElement instanceof HTMLElement && modes.contains(document.activeElement)
        ? document.activeElement.getAttribute("data-ctl")
        : null;
    fill(modes, ...modesBlock(store, act));
    if (focused) modes.querySelector<HTMLElement>(`[data-ctl="${focused}"]`)?.focus({ preventScroll: true });
  };

  const renderAll = () => {
    fill(tabs, ...railTabs(store, act));
    renderModes();
    renderScroll();
    renderFoot();
  };

  // What the Tune half shows depends on the controls and the presets and on nothing a
  // running trace changes, so a trace's progress must not rebuild it: a slider that is
  // replaced under the pointer cannot be dragged.
  store.on(["caps", "prefs", "preset", "settings", "railTab", "groupsOpen"], renderAll);
  store.on(["tracing", "liveStages", "result", "report", "palette", "losses", "source", "worstCorner", "previous"], () => {
    if (store.state.railTab === "result") renderScroll();
    renderFoot();
    // The tab strip is not rebuilt here — a button replaced between the press and the
    // release never hears the click — only its note is rewritten.
    const note = tabs.querySelector<HTMLElement>('[data-note="result"]');
    if (note) note.textContent = resultNote(store);
  });
  renderAll();
  return rail;
}

// --------------------------------------------------------------------- the modes ---

/**
 * The denoiser and editable structure, big, above both halves of the rail.
 *
 * The denoiser is the difference between tracing a JPEG's damage faithfully and tracing what
 * the picture was meant to be, and editable structure is the difference between an SVG to
 * look at and an SVG to open in a vector editor. Both used to be rows in the Tune tab's
 * groups, which is a fine place for a precision slider and a poor one for these. They are
 * here whichever half of the rail is showing, and set the same settings the groups did.
 */
function modesBlock(store: Store, act: RailActions): HTMLElement[] {
  const st = store.state;
  const help = (key: string) => st.caps?.controls.find((c) => c.key === key)?.help ?? "";
  const den = st.caps?.denoiser;
  const mode = st.settings.cleanUpDamage;
  const supported = den?.supported ?? false;
  const missing = supported && den !== undefined && !den.installed && mode !== "off";

  const captions = { off: "pixels as they are", auto: "only if damaged", on: "always" } as const;
  const denoiser = h(
    "div.mode",
    null,
    h(
      "div.modehead",
      null,
      tip(h("span.modetitle", { tabindex: "0" }, "Denoiser"), help("cleanUpDamage")),
      missing
        ? h("button.reset", { onclick: act.openDenoiser, title: "It runs on this computer; nothing is uploaded" }, "Download it")
        : h("span.modestate", null, supported ? captions[mode] : "not in this build"),
    ),
    h(
      "div.seg.big",
      { role: "group", "aria-label": "Denoiser" },
      ...(["off", "auto", "on"] as const).map((id) =>
        h(
          "button",
          {
            "aria-pressed": String(mode === id),
            disabled: !supported,
            "data-ctl": `cleanUpDamage:${id}`,
            onclick: () => act.changeSetting("cleanUpDamage", id),
          },
          id === "off" ? "Off" : id === "auto" ? "Auto" : "On",
        ),
      ),
    ),
  );

  const editable = st.settings.editability;
  const structure = h(
    "div.mode",
    null,
    h(
      "div.modehead",
      null,
      tip(h("span.modetitle", { tabindex: "0" }, "Editable"), help("editability")),
    ),
    h(
      "button.bigswitch",
      {
        role: "switch",
        "aria-checked": String(editable),
        "aria-label": "Editable structure",
        "data-ctl": "editability:switch",
        onclick: () => act.changeSetting("editability", !editable),
      },
      h("span.knob"),
      h("span.state", null, editable ? "On" : "Off"),
    ),
  );
  return [denoiser, structure];
}

// ---------------------------------------------------------------------- the tabs ---

/** How many controls are away from where the selected preset put them. */
function changedCount(store: Store, act: RailActions): number {
  const base = act.baseSettings();
  const st = store.state;
  return (st.caps?.controls ?? []).filter((c) => st.settings[c.key] !== base[c.key]).length;
}

/** What the Result tab says about itself: the colour difference of the drawing on screen. */
function resultNote(store: Store): string {
  const r = store.state.report;
  return r ? `${de00(r.meanDe00)} dE00` : "";
}

/**
 * The switch between the two halves, drawn as a tab strip rather than another segmented
 * control: on its own recessed bar with a rule under it, an underline on the tab that is
 * showing, and a line saying what that half is. Each tab also says something true about
 * itself — the colour difference of what came out, how many controls have moved — so it
 * is a summary as well as a switch.
 *
 * Styled through `.railseg`, not `.seg`: that class is shared with the viewer toolbar and
 * the app bar.
 */
function railTabs(store: Store, act: RailActions): HTMLElement[] {
  const st = store.state;
  const changed = changedCount(store, act);
  const tab = (id: "result" | "tune", label: string, note: string, hint: string) =>
    h(
      "button",
      {
        role: "tab",
        "aria-selected": String(st.railTab === id),
        title: hint,
        onclick: () => store.set({ railTab: id }),
      },
      h("span.tablabel", null, label),
      h("span.tabnote", { "data-note": id }, note),
    );
  return [
    h(
      "div.railseg",
      { role: "tablist", "aria-label": "What the rail shows" },
      tab("result", "Result", resultNote(store), "What this trace produced"),
      tab("tune", "Tune", changed ? `${changed} changed` : "", "The settings that make the next trace"),
    ),
    h(
      "p.tabcaption",
      null,
      st.railTab === "result" ? "What this trace produced." : "The settings for the next trace.",
    ),
  ];
}

// ------------------------------------------------------------------------- tune ---

function tunePane(store: Store, act: RailActions): (HTMLElement | null)[] {
  return [presets(store, act), ...controlGroups(store, act)];
}

// ------------------------------------------------------------------- presets ---

function presets(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  const list = st.caps?.presets ?? [];
  const saved = st.prefs?.saved ?? [];
  const mod = modKey(st.caps?.platform);
  const chip = (id: string, name: string, sub: string, hotkey: number | null, remove: (() => void) | null) =>
    h(
      remove ? "button.preset.saved" : "button.preset",
      {
        "aria-pressed": String(st.preset === id),
        title: `${name} — ${sub}${hotkey ? ` (${mod}+${hotkey})` : ""}`,
        onclick: () => act.setPreset(id),
      },
      h("span.name", null, name),
      remove
        ? h("span.forget", {
            role: "button",
            tabindex: "0",
            "aria-label": `Forget ${name}`,
            title: `Forget ${name}`,
            onclick: (e: Event) => {
              // The chip is a button; without this the click would also select the preset
              // it is on its way to deleting.
              e.stopPropagation();
              remove();
            },
          })
        : null,
    );

  const current = list.find((p) => p.id === st.preset);
  const caption = current
    ? `${current.subtitle}.`
    : saved.find((p) => p.id === st.preset)
      ? "One of your saved presets."
      : "Controls moved from the preset they started at.";
  const changed = changedCount(store, act);

  return h(
    "div.presetblock",
    null,
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, "Preset"),
      h(
        "button.reset",
        {
          disabled: !st.caps || !changed,
          title: "Put every control back to where the preset had it",
          onclick: () => st.preset && act.setPreset(st.preset),
        },
        changed ? `Reset ${changed} changed` : "Nothing changed",
      ),
    ),
    h(
      "div.presets",
      { role: "group", "aria-label": "Presets" },
      ...list.map((p, i) => chip(p.id, p.name, p.subtitle, i < 9 ? i + 1 : null, null)),
      ...saved.map((p) => chip(p.id, p.name, "Saved preset", null, () => act.deletePreset(p.id))),
    ),
    h(
      "div.traybar",
      null,
      h("span.faint", null, caption),
      h(
        "button.reset",
        {
          disabled: !st.caps,
          title: `Remember all ${st.caps?.controls.length ?? ""} controls exactly as they stand`,
          onclick: () => saveCurrentAsPreset(store, act),
        },
        "Save as preset",
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
          `All ${store.state.caps?.controls.length ?? ""} controls, as they stand. A saved preset is a snapshot rather than a set of differences from the defaults, so it will not drift when those move.`,
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

// ---------------------------------------------------------------------- result ---

function resultPane(store: Store, act: RailActions): (HTMLElement | null)[] {
  if (!store.state.source) return [emptyResult()];
  return [reportCard(store, act), structureCard(store, act), lostCard(store), paletteCard(store, act), benchmarkNote()];
}

/** Before any image is open there is nothing to report, and a card of dashes says so badly. */
function emptyResult(): HTMLElement {
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
function benchmarkNote(): HTMLElement {
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
      h("span", { style: { fontSize: "12px", fontWeight: "500" } }, `Tracing at ${plannedTracePx(st, st.tracingTier) ?? "—"} px`),
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
function structureCard(store: Store, act: RailActions): HTMLElement | null {
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

// -------------------------------------------------------- control groups ---

/**
 * The controls, four groups of them, each folded open or shut.
 *
 * A group says how many of its controls are away from the preset, and can put just those
 * back — "what did I change?" is the question a wall of eighteen sliders makes hard, and
 * the one somebody comparing two traces keeps asking.
 */
function controlGroups(store: Store, act: RailActions): HTMLElement[] {
  const st = store.state;
  const controls = (st.caps?.controls ?? []).filter((c) => !PROMOTED.has(c.key));
  const base = act.baseSettings();
  const groups = [...new Set(controls.map((c) => c.group))];

  return groups.map((g) => {
    const rows = controls.filter((c) => c.group === g);
    const open = st.groupsOpen[g] !== false;
    const changed = rows.filter((c) => st.settings[c.key] !== base[c.key]).length;
    return h(
      "section.group",
      { class: open ? "open" : undefined },
      h(
        "div.grouphead",
        null,
        h(
          "button.groupbtn",
          {
            "aria-expanded": String(open),
            "data-ctl": `group:${g}`,
            onclick: () => {
              st.groupsOpen[g] = !open;
              store.touch("groupsOpen");
            },
          },
          icon(open ? "chevronDown" : "chevronRight", 13),
          h("span.eyebrow", null, g),
          changed ? h("span.changed", null, `${changed} changed`) : null,
        ),
        changed ? h("button.reset", { onclick: () => act.resetGroup(g) }, "Reset") : null,
      ),
      open ? h("div.groupbody", null, ...rows.map((c) => controlRow(store, c, act, base))) : null,
    );
  });
}

/**
 * One row: a plain-words label, its unit, the value shown numerically, and a slider on a
 * perceptual scale with named stops rather than a bare track.
 */
function controlRow(store: Store, c: Control, act: RailActions, base: Settings): HTMLElement {
  const value = store.state.settings[c.key];
  const changed = value !== base[c.key];
  const row = (...kids: (HTMLElement | null)[]) => h(changed ? "div.control.changed" : "div.control", null, ...kids);

  if (c.kind === "switch") {
    const sw = h("button.switch", {
      role: "switch",
      "aria-checked": String(Boolean(value)),
      "aria-label": c.label,
      "data-ctl": `${c.key}:switch`,
      onclick: () => act.changeSetting(c.key, !value as never),
    });
    return row(h("div.controlhead", null, tip(h("span.label", { tabindex: "0" }, c.label), c.help), sw));
  }

  if (c.kind === "tri") {
    const options: [string, string][] = [["off", "Off"], ["auto", "Auto"], ["on", "On"]];
    return row(
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
              {
                "aria-pressed": String(value === id),
                "data-ctl": `${c.key}:${id}`,
                onclick: () => act.changeSetting(c.key, id as never),
              },
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
    "data-ctl": `${c.key}:field`,
    onchange: (e: Event) => {
      const n = Number((e.target as HTMLInputElement).value);
      if (Number.isFinite(n)) act.changeSetting(c.key, Math.min(c.max, Math.max(c.min, n)) as never);
    },
  }) as HTMLInputElement;

  const valueAt = (el: HTMLInputElement) => {
    const v = fromSlider(Number(el.value));
    return c.decimals === 0 ? Math.round(v) : Number(v.toFixed(c.decimals));
  };

  // Dragging only moves the readout. A trace is Rust work and the rail rebuilds itself
  // when a setting changes — either one on every pixel of a drag is what made the
  // controls stutter — so the setting is committed once, when the thumb is let go. The
  // `change` event also fires once per key press, so the arrow keys still work, and the
  // rebuild hands focus back to the slider so the next press lands too.
  const slider = h("input.slider", {
    type: "range",
    min: "0",
    max: "1000",
    step: "1",
    value: String(Math.round(toSlider(Number(value)))),
    "aria-label": c.label,
    "aria-valuetext": `${show(Number(value))} ${c.unit}`.trim(),
    "data-ctl": `${c.key}:slider`,
    oninput: (e: Event) => {
      const el = e.target as HTMLInputElement;
      const rounded = valueAt(el);
      field.value = show(rounded);
      el.setAttribute("aria-valuetext", `${show(rounded)} ${c.unit}`.trim());
    },
    onchange: (e: Event) => act.changeSetting(c.key, valueAt(e.target as HTMLInputElement) as never),
  });

  return row(
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

// ------------------------------------------------------------------ footer ---

/**
 * The pinned foot: the live readout, Export, and the one honest note about Export.
 *
 * The readout is the loop that makes the controls useful. Moving one starts a trace, and
 * the figures that matter — the measured colour difference, what the drawing cost in
 * coordinates, the file it makes — land here beside the change since the last full trace,
 * wherever the rail is scrolled and whichever half of it is showing.
 */
function footer(store: Store, act: RailActions): HTMLElement[] {
  const st = store.state;
  const ready = Boolean(st.svg) && !st.tracing;
  return [
    readout(store, act),
    h(
      "div.row",
      null,
      h("button.btn.primary", { style: { flex: "1" }, disabled: !ready, onclick: act.openExport }, "Export"),
      h("button.btn", { disabled: !ready, onclick: act.copySvg, title: "Paste straight into Figma or Illustrator" }, "Copy SVG"),
      h("button.btn.icon", { disabled: !ready, onclick: act.saveCard, "aria-label": "Save comparison card", title: "Save comparison card" }, icon("share", 16)),
    ),
    h(
      "div.note",
      null,
      h("span", null, "Export runs a fresh full trace."),
      h("button.reset", { disabled: !st.source || st.tracing, onclick: act.traceNow }, "Trace again"),
    ),
  ];
}

function readout(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  const r = st.report;

  if (st.tracing) {
    const last = st.liveStages[st.liveStages.length - 1];
    const elapsed = st.liveStages.reduce((a: number, x: Stage) => a + x.ms, 0) / 1000;
    return h(
      "div.readout.busy",
      null,
      h("span.pulse"),
      h("span.stage", null, last?.name ?? "starting"),
      h("span.muted.num.elapsed", null, seconds(elapsed)),
      h("button.reset", { onclick: act.cancel, title: "Esc" }, "Cancel"),
    );
  }
  if (!r) {
    return h("div.readout.idle", null, h("span.faint", null, st.source ? "Nothing traced yet." : "Open an image to see what a setting does."));
  }

  // Only a full trace is compared with a full trace: a draft is smaller, so its counts
  // would read as a change nobody made.
  const p: Report | null = st.result?.tier === "final" ? st.previous : null;
  const cell = (value: string, label: string, change: HTMLElement | null) =>
    h("div.cell", null, h("span.v.num", null, value), h("span.k", null, label), change);

  return h(
    "div.readout",
    { "aria-live": "polite" },
    cell(de00(r.meanDe00), "dE00", p ? change(r.meanDe00, p.meanDe00, (d) => d.toFixed(2), 0.005, true) : null),
    cell(count(r.coordinates), "coordinates", p ? change(r.coordinates, p.coordinates, count, 0, false) : null),
    cell(bytes(r.bytes), "file", p ? change(r.bytes, p.bytes, bytes, 16, false) : null),
  );
}

/**
 * What a control moved, in the words of the number it moved.
 *
 * Fewer coordinates and a smaller file are only ever good news, but a bigger colour
 * difference is a cost, so only that one turns amber.
 */
function change(
  now: number | null | undefined,
  before: number | null | undefined,
  fmt: (n: number) => string,
  epsilon: number,
  costWhenHigher: boolean,
): HTMLElement | null {
  if (now == null || before == null) return null;
  const d = now - before;
  if (Math.abs(d) <= epsilon) return h("span.delta.same", null, "no change");
  const cls = d < 0 ? "better" : costWhenHigher ? "worse" : "same";
  return h(`span.delta.${cls}`, null, `${d < 0 ? "−" : "+"}${fmt(Math.abs(d))}`);
}

/** A tiny helper the export sheet and the batch bar both want. */
export function pill(label: string, onclick: () => void, pressed?: boolean): HTMLElement {
  return h("button.btn.compact", { "aria-pressed": pressed === undefined ? null : String(pressed), onclick }, label);
}

/** Let other modules raise a toast without importing the overlay module directly. */
export { toast };

/** Re-exported for the export sheet, which lives in its own module. */
export { s };
