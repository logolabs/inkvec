/**
 * The wipe divider of a two-pane comparison (the Vectorize viewer and the Minify tab), as a
 * slider that works with a pointer, a single tap and the keyboard.
 *
 * The divider used to be a `role="separator"` that only a drag could move: no Tab stop, no
 * value, nothing for a keyboard or a switch user to operate (WCAG 2.2 SC 2.1.1 Keyboard,
 * and SC 2.5.7 Dragging Movements, which asks for a single-pointer way to do anything a drag
 * does; r2-product, 2026-10-02). It now follows the WAI-ARIA Authoring Practices "Slider
 * Pattern" (https://www.w3.org/WAI/ARIA/apg/patterns/slider/), which this implements as
 * described:
 *
 * - `role="slider"`, `tabindex="0"`, `aria-valuemin` 2, `aria-valuemax` 98, `aria-valuenow`
 *   the share of the width the left pane shows, in percent, and `aria-valuetext` saying
 *   which pane is which ("62% source, 38% vector"). 2 and 98 are the wipe's own clamp: at 0
 *   or 100 the handle would sit under the pane's edge where it cannot be grabbed back.
 * - Arrow Left/Down and Right/Up step by 2 percentage points, with Shift (and Page Down/Up)
 *   by 10; Home and End go to the ends.
 * - A drag still moves it 1:1 with the pointer, as before.
 *
 * The single-pointer alternative for SC 2.5.7 is `tapToWipe`: a press and release on the
 * panes without moving moves the divider to that point, the way comparison sliders on the web
 * usually behave. A press that moves is still a pan. Not from the literature: the movement
 * threshold that tells a tap from a drag (`TAP_SLOP`), because the APG and WCAG define the
 * behaviour, not the threshold. See also: the Pointer Events "click" vs drag distinction in
 * UI toolkits, which use a few CSS pixels of slop the same way.
 */

/** The wipe's range, as a share of the panes' width: the handle is never lost under an edge. */
export const WIPE_MIN = 0.02;
export const WIPE_MAX = 0.98;
/** A keyboard step, and a big one (Shift, Page Up/Down), as a share of the width. */
const STEP = 0.02;
const BIG_STEP = 0.1;
/** How far, in CSS pixels, a press may travel and still be a tap rather than a drag. */
export const TAP_SLOP = 4;

/** `v` clamped to the wipe's range. */
export function clampWipe(v: number): number {
  return Math.min(WIPE_MAX, Math.max(WIPE_MIN, v));
}

/**
 * The wipe after `key` is pressed on the slider at `now` (shares of the width), or null for a
 * key the slider does not use. Pure, for the tests: the arithmetic of the APG's key table.
 */
export function wipeAfterKey(now: number, key: string, shift: boolean): number | null {
  const step = shift ? BIG_STEP : STEP;
  switch (key) {
    case "ArrowLeft":
    case "ArrowDown":
      return clampWipe(now - step);
    case "ArrowRight":
    case "ArrowUp":
      return clampWipe(now + step);
    case "PageDown":
      return clampWipe(now - BIG_STEP);
    case "PageUp":
      return clampWipe(now + BIG_STEP);
    case "Home":
      return WIPE_MIN;
    case "End":
      return WIPE_MAX;
    default:
      return null;
  }
}

/** What a wipe slider reads and writes, and what it calls its two sides. */
export interface WipeModel {
  /** The wipe now, as a share of the width (0.02 to 0.98). */
  get(): number;
  /** Move the wipe; the owner redraws and calls `sync` (directly or through its store). */
  set(v: number): void;
  /** What the left and right panes are, for the value text: "source" and "vector". */
  names(): [string, string];
}

/**
 * Make `divider` (inside `panes`) a slider over `model`. Returns `sync`, which writes the
 * current value into the ARIA attributes; call it whenever the wipe or the pane names change.
 */
export function wipeSlider(divider: HTMLElement, panes: HTMLElement, model: WipeModel): () => void {
  divider.setAttribute("role", "slider");
  divider.setAttribute("tabindex", "0");
  divider.setAttribute("aria-orientation", "horizontal");
  divider.setAttribute("aria-valuemin", String(Math.round(WIPE_MIN * 100)));
  divider.setAttribute("aria-valuemax", String(Math.round(WIPE_MAX * 100)));

  const sync = () => {
    const pct = Math.round(model.get() * 100);
    const [left, right] = model.names();
    divider.setAttribute("aria-valuenow", String(pct));
    divider.setAttribute("aria-valuetext", `${pct}% ${left}, ${100 - pct}% ${right}`);
    divider.setAttribute("aria-label", `Wipe between ${left} and ${right}`);
  };

  divider.addEventListener("keydown", (e: KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const next = wipeAfterKey(model.get(), e.key, e.shiftKey);
    if (next === null) return;
    e.preventDefault();
    // The window's own keys (+/- zoom, Space to flick) are not meant for a focused slider.
    e.stopPropagation();
    model.set(next);
  });

  // A drag follows the pointer 1:1, and releasing does not animate.
  divider.addEventListener("pointerdown", (e: PointerEvent) => {
    e.stopPropagation();
    divider.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      const r = panes.getBoundingClientRect();
      model.set(clampWipe((m.clientX - r.left) / r.width));
    };
    const up = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", up);
      divider.removeEventListener("pointercancel", up);
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", up);
    divider.addEventListener("pointercancel", up);
  });

  sync();
  return sync;
}

/** Move the wipe of `panes` to the horizontal position of a tap at `clientX`. */
export function tapToWipe(panes: HTMLElement, model: WipeModel, clientX: number): void {
  const r = panes.getBoundingClientRect();
  if (r.width > 0) model.set(clampWipe((clientX - r.left) / r.width));
}
