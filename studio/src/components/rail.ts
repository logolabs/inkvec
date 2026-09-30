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

import { fill, h, icon } from "../lib/dom";
import { appliesTo, bytes, count, de00, modKey, seconds, type Store } from "../lib/state";
import type { Report, Settings } from "../lib/ipc";
import { WEB } from "../lib/platform";
import { closeOverlay, modal, openModal, tip } from "./overlays";
import { paletteCard, wirePaletteHover, type PaletteActions } from "./palette";
import { autoChose, hasAutoNews, type AutoChoseActions } from "./autochose";
import { fetchBar, fetchLine, sentence } from "./denoiserfetch";
import { controlRow } from "./controlrow";
import { benchmarkNote, emptyResult, lostCard, reportCard, stageCard, structureCard } from "./report";
import { elapsedText, runningLabel } from "../lib/live";

/** Everything the rail's controls ask the app to do; `main.ts` provides them. */
export interface RailActions extends PaletteActions, AutoChoseActions {
  setPreset(id: string): void;
  savePreset(name: string): void;
  deletePreset(id: string): void;
  changeSetting(key: keyof Settings, value: Settings[keyof Settings]): void;
  resetGroup(group: string): void;
  /** The settings the controls are measured against: the selected preset's, else the defaults. */
  baseSettings(): Settings;
  traceNow(): void;
  cancel(): void;
  openExport(): void;
  copySvg(): void;
  saveCard(): void;
  jumpToWorst(): void;
  /** Open the denoiser's download dialog. */
  openDenoiser(): void;
  /** Inkvec Studio Lite: try the denoiser's download again after it failed. */
  retryDenoiser(): void;
  /** Close the "Auto chose" note for this image. */
  hideAutoNote(): void;
  /** Open the Custom wizard over the rail. */
  openWizard(): void;
}

/**
 * The three settings that are promoted out of the control groups into the modes block at the
 * top of the rail. They are the things people most often come for, so they are not left
 * three folds down a list; and each is drawn once, so a setting never has two controls.
 */
export const PROMOTED: ReadonlySet<string> = new Set(["mode", "cleanUpDamage", "editability"]);

/**
 * The rail, built once: the Result/Tune tab strip, the engine, denoiser and editability
 * block, the scrolling half that is showing, and the pinned foot. Each part subscribes to the
 * keys it reads and is rebuilt only when one of those changes.
 */
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
      // The elapsed time is the live clock's to write (`lib/live.ts`).
      busy.querySelector(".stage")!.textContent = runningLabel(st);
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
  // The browser's denoiser download moves a note under the Denoiser buttons, nothing else.
  store.on(["denoiserFetch"], renderModes);
  store.on(
    [
      "tracing",
      "liveStages",
      "liveNow",
      "result",
      "report",
      "palette",
      "losses",
      "source",
      "worstCorner",
      "previous",
      "colourGroups",
      "groupSuggestions",
      "resultGroups",
      "paletteSelection",
      "simplifyTo",
      "auto",
      "facts",
      "chooser",
      "wizard",
      "autoNoteHidden",
    ],
    () => {
      if (store.state.railTab === "result") renderScroll();
      renderFoot();
      // The tab strip is not rebuilt here — a button replaced between the press and the
      // release never hears the click — only its note is rewritten.
      const note = tabs.querySelector<HTMLElement>('[data-note="result"]');
      if (note) note.textContent = resultNote(store);
    },
  );
  // A swatch, a group or a member hovered or focused singles its fills out in the drawing.
  wirePaletteHover(scroll, store);
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
  const engineMode = st.settings.mode ?? "quality";

  const engine = h(
    "div.mode.mode-engine",
    null,
    h(
      "div.modehead",
      null,
      tip(
        h("span.modetitle", { tabindex: "0" }, "Engine"),
        help("mode") ||
          "Quality places every edge to a fraction of a pixel and fits the fewest curves that match the image: the closest trace, and the default. Fast traces each shape in a single pass, many times quicker, for previews, batches and very large images.",
      ),
      h("span.modestate", null, engineMode === "fast" ? "one pass" : "closest fit"),
    ),
    h(
      "div.seg.big",
      { role: "group", "aria-label": "Tracing Engine" },
      h(
        "button",
        {
          "aria-pressed": String(engineMode === "quality"),
          "data-ctl": "mode:quality",
          title: "Quality: the closest trace, edges placed to a fraction of a pixel",
          onclick: () => act.changeSetting("mode", "quality"),
        },
        "Quality",
      ),
      h(
        "button",
        {
          "aria-pressed": String(engineMode === "fast"),
          "data-ctl": "mode:fast",
          title: "Fast: each shape traced in one pass, many times quicker, a little less exact",
          onclick: () => act.changeSetting("mode", "fast"),
        },
        "Fast",
      ),
    ),
  );

  const den = st.caps?.denoiser;
  const mode = st.settings.cleanUpDamage;
  const supported = den?.supported ?? false;
  // In a browser the denoiser downloads by itself (in the background, or as soon as it is
  // turned on) and a trace waits for nothing: this says how far that has got, under the
  // buttons that asked for it. The desktop's download is one the user starts.
  const dl = WEB && supported && mode !== "off" && st.denoiserFetch?.phase !== "ready" ? st.denoiserFetch : undefined;
  const fetching = dl !== undefined;
  const missing = !fetching && supported && den !== undefined && !den.installed && mode !== "off";

  const captions = { off: "pixels as they are", auto: "only if damaged", on: "always" } as const;
  const denoiser = h(
    "div.mode",
    null,
    h(
      "div.modehead",
      null,
      tip(h("span.modetitle", { tabindex: "0" }, "Denoiser"), help("cleanUpDamage")),
      fetching && dl?.phase === "failed"
        ? h("button.reset", { "data-ctl": "denoiser-retry", onclick: act.retryDenoiser }, "Retry")
        : missing
        ? h("button.reset", { onclick: act.openDenoiser, title: "It runs on this computer; nothing is uploaded" }, "Download it")
        : h(
            "span.modestate",
            // In a browser the denoiser needs a cross-origin isolated page; the Space's own
            // tab is one, its embedding frame may not be.
            null,
            supported ? captions[mode] : WEB ? "needs its own tab" : "not in this build",
          ),
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
    fetching
      ? h(
          "div.fetchnote",
          { "data-ctl": "denoiser-progress", role: "status" },
          fetchBar(dl ?? null),
          h("span.fetchline", null, dl?.phase === "failed" ? `${fetchLine(dl, true)}: ${dl.message ?? "the connection dropped"}.` : sentence(fetchLine(dl ?? null, true) ?? "")),
          h("span.muted", null, dl?.phase === "failed" ? "The trace shown is without it." : "Shown without it until then; it traces again by itself."),
        )
      : null,
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
  return [engine, h("div.railmodes-row", null, denoiser, structure)];
}

// ---------------------------------------------------------------------- the tabs ---

/**
 * How many controls are away from where the selected preset put them, among those that
 * change the drawing in the engine that is on. One hidden in this engine keeps its value,
 * but it is not a change to this drawing, so it is not counted as one.
 */
function changedCount(store: Store, act: RailActions): number {
  const base = act.baseSettings();
  const st = store.state;
  return (st.caps?.controls ?? []).filter((c) => appliesTo(c, st.settings) && st.settings[c.key] !== base[c.key]).length;
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

/**
 * In Fast, the controls that only Quality reads are not shown (they steer stages Fast skips).
 * One quiet line says how many there are and where they went, so a control that vanished is
 * not a control that was lost; their values are kept for when Quality is back on.
 */
function qualityOnlyLine(store: Store, act: RailActions): HTMLElement | null {
  const st = store.state;
  if (st.settings.mode !== "fast") return null;
  const hidden = (st.caps?.controls ?? []).filter((c) => !PROMOTED.has(c.key) && !appliesTo(c, st.settings)).length;
  if (!hidden) return null;
  return h(
    "div.guideline",
    { "data-note": "quality-only" },
    h("span.faint", null, `${hidden} more control${hidden === 1 ? " is" : "s are"} for Quality: switch to see ${hidden === 1 ? "it" : "them"}.`),
    h(
      "button.reset",
      { "data-ctl": "show-quality", title: "Switch the engine to Quality", onclick: () => act.changeSetting("mode", "quality") },
      "Quality",
    ),
  );
}

/** The Tune half: the way into the wizard, the Fast note, the presets, and the control groups. */
function tunePane(store: Store, act: RailActions): (HTMLElement | null)[] {
  return [guideLine(store, act), qualityOnlyLine(store, act), presets(store, act), ...controlGroups(store, act)];
}

/** The way back into the wizard, for an image that is already open. */
function guideLine(store: Store, act: RailActions): HTMLElement | null {
  if (!store.state.source) return null;
  return h(
    "div.guideline",
    null,
    h("span.faint", null, "Not sure which controls matter here?"),
    h(
      "button.reset",
      { "data-ctl": "open-wizard", title: "The main choices one step at a time, each explained, with the result beside Auto's", onclick: act.openWizard },
      "Walk me through it",
    ),
  );
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
  return [autoCard(store, act), reportCard(store, act), structureCard(store, act), lostCard(store), paletteCard(store, act), benchmarkNote()];
}

/**
 * "Auto chose", once the chooser over the stage has gone: what the automatic trace noticed,
 * with the alternatives and the colour-group suggestions a click away. Only while it has
 * something to offer, and closable for the image.
 */
function autoCard(store: Store, act: RailActions): HTMLElement | null {
  const st = store.state;
  if (st.chooser || st.wizard || st.autoNoteHidden || !hasAutoNews(store)) return null;
  const body = autoChose(store, act, true);
  if (!body) return null;
  return h(
    "div.card.autocard",
    null,
    h(
      "div.cardhead",
      null,
      h("span.eyebrow", null, "Auto chose"),
      h(
        "div",
        { style: { display: "flex", gap: "10px" } },
        h("button.reset", { "data-ctl": "auto-customise", onclick: act.openWizard }, "Customise"),
        h("button.reset", { "data-ctl": "auto-hide", title: "Hide this for the image", onclick: act.hideAutoNote }, "Hide"),
      ),
    ),
    body,
  );
}

// -------------------------------------------------------- control groups ---

/**
 * The controls, four groups of them, each folded open or shut.
 *
 * A group says how many of its controls are away from the preset, and can put just those
 * back — "what did I change?" is the question a wall of eighteen sliders makes hard, and
 * the one somebody comparing two traces keeps asking.
 *
 * Only the controls that change the drawing in the selected engine are listed, and a group
 * with none of those left is not drawn at all.
 */
function controlGroups(store: Store, act: RailActions): HTMLElement[] {
  const st = store.state;
  const controls = (st.caps?.controls ?? []).filter((c) => !PROMOTED.has(c.key) && appliesTo(c, st.settings));
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

/**
 * The foot's readout: the running stage and its clock with Cancel while a trace runs; else
 * the engine, the time taken, and dE00, coordinates and file size, each with its change
 * since the previous full trace.
 */
function readout(store: Store, act: RailActions): HTMLElement {
  const st = store.state;
  const r = st.report;

  if (st.tracing) {
    return h(
      "div.readout.busy",
      null,
      h("span.pulse"),
      h("span.stage", null, runningLabel(st)),
      h("span.muted.num.elapsed", { "data-live": "elapsed" }, elapsedText(st)),
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

  const isFast = (st.settings.mode ?? "quality") === "fast";
  const badgeRow = h(
    "div.readout-badge-row",
    null,
    h(
      "button.engine-pill" + (isFast ? ".fast" : ".quality"),
      {
        type: "button",
        title: isFast ? "Fast vectorizer active. Click to switch to Quality mode." : "Quality vectorizer active. Click to switch to Fast mode.",
        onclick: () => act.changeSetting("mode", isFast ? "quality" : "fast"),
      },
      h("span.engine-name", null, isFast ? "Fast" : "Quality"),
    ),
    r.seconds != null
      ? h(
          "span.time-pill.num",
          { title: `Traced in ${seconds(r.seconds)}` },
          seconds(r.seconds),
        )
      : null,
  );

  return h(
    "div.readout",
    { "aria-live": "polite" },
    badgeRow,
    h(
      "div.readout-grid",
      null,
      cell(de00(r.meanDe00), "dE00", p ? change(r.meanDe00, p.meanDe00, (d) => d.toFixed(2), 0.005, true) : null),
      cell(count(r.coordinates), "coordinates", p ? change(r.coordinates, p.coordinates, count, 0, false) : null),
      cell(bytes(r.bytes), "file", p ? change(r.bytes, p.bytes, bytes, 16, false) : null),
    ),
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
