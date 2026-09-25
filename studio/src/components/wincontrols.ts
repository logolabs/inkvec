/**
 * The window's own controls.
 *
 * On the desktop: minimise, maximise and close. The window has `decorations: false`, so
 * these three buttons are the only way to work the window — which is why they are a
 * component rather than three lines inside the app bar. Settings and About cover the app bar
 * completely, and a full-window screen that takes the chrome away without giving it back
 * leaves somebody with no way to close the app but the keyboard.
 *
 * In a browser tab (Inkvec Studio Lite) there is no window to work, but there is a screen to
 * fill: Full screen, through the Fullscreen API, and — when the page is embedded, as it is on
 * the Hugging Face Space, whose frame may not be allowed to go full screen — "Open in its own
 * tab", which is the same page at its own address.
 */

import { getCurrentWindow } from "@tauri-apps/api/window";

import { h, icon } from "../lib/dom";
import { WEB } from "../lib/platform";

export function windowControls(): HTMLElement {
  if (WEB) return webControls();
  const win = getCurrentWindow();
  return h(
    "div.wincontrols",
    null,
    h("button.min", { "aria-label": "Minimise", onclick: () => void win.minimize() }, h("i")),
    h("button.max", { "aria-label": "Maximise", onclick: () => void win.toggleMaximize() }, h("i")),
    h("button.close", { "aria-label": "Close", onclick: () => void win.close() }, icon("x", 13)),
  );
}

/** Whether this page is inside someone else's frame (the Space embeds it). */
function embedded(): boolean {
  try {
    return window.self !== window.top;
  } catch {
    // A cross-origin parent refuses even the comparison: that is an embed.
    return true;
  }
}

function webControls(): HTMLElement {
  const full = h(
    "button.btn.ghost.compact.fullscreen",
    {
      "data-ctl": "fullscreen",
      title: "Fill the screen (Esc to leave)",
      onclick: () => {
        if (document.fullscreenElement) void document.exitFullscreen();
        else void document.documentElement.requestFullscreen().catch(() => ownTab());
      },
    },
    icon("maximize", 14),
    h("span", null, "Full screen"),
  );
  const label = full.querySelector("span");
  const sync = () => {
    if (label) label.textContent = document.fullscreenElement ? "Exit full screen" : "Full screen";
  };
  document.addEventListener("fullscreenchange", sync);

  const ownTab = () => window.open(window.location.href, "_blank", "noopener");
  const tab = embedded()
    ? h(
        "button.btn.ghost.compact",
        { "data-ctl": "own-tab", title: "The same app at its own address, with the whole window", onclick: ownTab },
        icon("external", 14),
        h("span", null, "Open in its own tab"),
      )
    : null;
  // A frame that is not allowed to go full screen gets only the way out of the frame.
  return h("div.webcontrols", null, document.fullscreenEnabled ? full : null, tab);
}
