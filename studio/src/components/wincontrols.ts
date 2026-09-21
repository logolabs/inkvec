/**
 * Minimise, maximise and close.
 *
 * The window has `decorations: false`, so these three buttons are the only way to work
 * the window — which is why they are a component rather than three lines inside the app
 * bar. Settings and About cover the app bar completely, and a full-window screen that
 * takes the chrome away without giving it back leaves somebody with no way to close the
 * app but the keyboard.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";

import { h, icon } from "../lib/dom";

export function windowControls(): HTMLElement {
  const win = getCurrentWindow();
  return h(
    "div.wincontrols",
    null,
    h("button.min", { "aria-label": "Minimise", onclick: () => void win.minimize() }, h("i")),
    h("button.max", { "aria-label": "Maximise", onclick: () => void win.toggleMaximize() }, h("i")),
    h("button.close", { "aria-label": "Close", onclick: () => void win.close() }, icon("x", 13)),
  );
}
