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
 * tab", which is the same page at its own address, with the image that is open (`ownTab`).
 */

import { getCurrentWindow } from "@tauri-apps/api/window";

import { h, icon } from "../lib/dom";
import { DESKTOP_URL, DESKTOP_WHY, WEB } from "../lib/platform";
import { HANDOFF_MAX, handOffHash } from "../lib/web/chrome";
import { webBackend } from "../lib/web/engine";
import { toast } from "./overlays";

/**
 * The window's controls for this build: minimise, maximise and close on the desktop; in a
 * browser, Full screen or "Open in its own tab" (`webControls`).
 */
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

/**
 * "Open in its own tab": this page at its own address, carrying the image that is open. From
 * inside the Space's frame nothing but the address reaches the new tab: the frame's storage is
 * partitioned under the embedding site, and the Studio's isolation (COOP) cuts the tab off
 * from its opener, so a message is dropped. The file therefore rides in the URL fragment, the
 * way the presentation page hands a dropped file over (`handOffHash`; `takeLaunch` reads it
 * and clears the address). A file over `HANDOFF_MAX` cannot fit in an address: the user is
 * told so before the tab opens without it, and the toast's button opens it.
 */
function ownTab(): void {
  const page = window.location.href.split("#")[0];
  const open = webBackend().openImage();
  if (!open) {
    window.open(page, "_blank", "noopener");
    return;
  }
  const name = open.name ?? "image";
  const hash = handOffHash(name, open.bytes);
  if (hash) {
    window.open(page + hash, "_blank", "noopener");
    return;
  }
  const mb = (open.bytes.length / 1e6).toFixed(1);
  toast(
    `${name} is ${mb} MB. A page address carries a file of up to ${(HANDOFF_MAX / 1e6).toFixed(1)} MB, so the new tab opens without it: open the file again there.`,
    { action: { label: "Open the tab", run: () => window.open(page, "_blank", "noopener") } },
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

  const tab = embedded()
    ? h(
        "button.btn.ghost.compact",
        {
          "data-ctl": "own-tab",
          title: `The same app at its own address, with the whole window and every core. The open image comes along, up to ${(HANDOFF_MAX / 1e6).toFixed(1)} MB.`,
          onclick: ownTab,
        },
        icon("external", 14),
        h("span", null, "Open in its own tab"),
      )
    : null;
  // Back to the Space's front page. Named as a file: Hugging Face's static host has no index
  // for a folder below its root, so "../" would land on one of its own pages instead.
  const home = h(
    "a.btn.ghost.compact",
    { "data-ctl": "home", href: "../index.html", title: "The Inkvec front page: what's new, before and after, results" },
    icon("globe", 14),
    h("span", null, "Inkvec home"),
  );
  // The desktop app, recommended once and quietly: it opens in a new tab.
  const desktop = h(
    "a.btn.ghost.compact.desktopapp",
    {
      "data-ctl": "desktop-app",
      href: DESKTOP_URL,
      target: "_blank",
      rel: "noopener noreferrer",
      title: DESKTOP_WHY,
    },
    icon("download", 14),
    h("span", null, "Desktop app"),
  );
  // A frame that is not allowed to go full screen gets only the way out of the frame.
  return h("div.webcontrols", null, home, desktop, document.fullscreenEnabled ? full : null, tab);
}
