/**
 * One control of the Tune tab (and of the wizard's steps): a switch, an Off/Auto/On choice,
 * a named choice, or a number with a slider on a perceptual scale.
 *
 * The control's description (`Control`, from the core's `options.rs`) says which it is, its
 * range, units and named stops; the value is read from the store and every change goes
 * through `changeSetting`, which traces.
 */

import { h } from "../lib/dom";
import type { Store } from "../lib/state";
import type { Control, Settings } from "../lib/ipc";
import { tip } from "./overlays";

/**
 * One row: a plain-words label, its unit, the value shown numerically, and a slider on a
 * perceptual scale with named stops rather than a bare track.
 */
export function controlRow(store: Store, c: Control, act: { changeSetting(key: keyof Settings, value: Settings[keyof Settings]): void }, base: Settings): HTMLElement {
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
    return row(
      h(
        "div.controlhead",
        null,
        tip(h("span.label", { tabindex: "0" }, c.label), c.help),
        sw,
      ),
    );
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

  if (c.kind === "choice") {
    const options = c.stops.map((s) => [s.label.toLowerCase(), s.label]);
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
