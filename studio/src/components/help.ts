/**
 * Help: the user guide, opened inside the app.
 *
 * The guide is the documentation site's Studio section, built into the app's own files
 * (`public/guide/`, written by `tools/build_site.py --studio-guide`). It is read from there,
 * in a full-window screen like Settings, so it works with no network and asks for nothing:
 * the app promises that nothing leaves this computer, and a help button that fetched a web
 * page would be the first thing to break that. "Open in browser" takes the same page from
 * the published site, in the system browser, for anyone who wants it beside the app.
 *
 * Self-contained on purpose: the app bar's button and F1 are the only ways in, and nothing
 * else in the app needs to know the screen exists. In a browser build without the bundled
 * copy it says so and offers the published guide instead.
 */

import { isTauri } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

import { fill, h } from "../lib/dom";
import type { State, Store } from "../lib/state";
import { toast } from "./overlays";
import { windowControls } from "./wincontrols";

/** The published guide; each bundled page has the same path under it. */
export const GUIDE_ONLINE = "https://logolabs.github.io/inkvec/studio/";

/** Where the bundled guide is served from, under whatever base the app is served at. */
const GUIDE_LOCAL = `${import.meta.env.BASE_URL}guide/`;

/** The page about what is on screen, so F1 lands somewhere useful. */
export function helpPageFor(st: State): string {
  if (st.screen === "settings") return "settings.html";
  switch (st.tab) {
    case "minify":
      return "minify.html";
    case "fabricate":
      return "fabricate.html";
    case "batch":
      return "batch.html";
    default:
      if (!st.source) return "getting-started.html";
      if (st.wizard) return "getting-started.html#the-custom-wizard";
      return st.railTab === "tune" ? "tune.html" : "result.html";
  }
}

/** A link that leaves the app: the system browser in the desktop app, a new tab in a browser. */
function openExternal(url: string): void {
  if (isTauri()) {
    openUrl(url).catch(() => toast("Could not open the link in your browser.", { kind: "bad" }));
  } else {
    window.open(url, "_blank", "noopener");
  }
}

let screen: HTMLElement | null = null;

/** Open the guide at `page` (a path under the guide, with an optional #fragment). */
export function openHelp(page = "index.html"): void {
  if (screen) {
    screen.querySelector("iframe")?.setAttribute("src", GUIDE_LOCAL + page);
    return;
  }
  const frame = h("iframe", {
    title: "Inkvec Studio user guide",
    src: GUIDE_LOCAL + page,
    style: { flex: "1", width: "100%", border: "0", background: "var(--stage)" },
  }) as HTMLIFrameElement;
  const body = h("div.screenbody", { style: { display: "flex", overflow: "hidden" } }, frame);

  /** The page on screen, as a path under the guide, for "Open in browser". */
  const shown = (): string => {
    try {
      const loc = frame.contentWindow?.location;
      const at = loc ? loc.pathname.indexOf("/guide/") : -1;
      if (loc && at >= 0) return loc.pathname.slice(at + "/guide/".length) + loc.hash;
    } catch {
      // Not the bundled guide (a page the frame could not show): fall back to the start.
    }
    return "";
  };

  screen = h(
    "div.screen",
    { role: "dialog", "aria-label": "User guide" },
    h(
      "div.screenbar",
      { "data-tauri-drag-region": "" },
      h("span", { style: { fontSize: "12.5px", fontWeight: "600" } }, "User guide"),
      h("div.spacer"),
      h("button.btn.compact.ghost", { onclick: () => frame.setAttribute("src", GUIDE_LOCAL + "index.html") }, "Contents"),
      h(
        "button.btn.compact.ghost",
        { title: "The same page on logolabs.github.io, in your browser", onclick: () => openExternal(GUIDE_ONLINE + shown()) },
        "Open in browser",
      ),
      h("button.btn.compact", { onclick: closeHelp }, "Done"),
      // This screen covers the app bar, so the window's own controls come with it.
      h("div.sep"),
      windowControls(),
    ),
    body,
  );

  frame.addEventListener("load", () => {
    const doc = frame.contentDocument;
    // Anything but a page of the guide means this build has no bundled copy (a dev server
    // answers a missing file with the app's own page): offer the published one instead.
    if (!doc || !doc.querySelector('meta[name="inkvec-guide"]')) {
      fill(
        body,
        h(
          "div.firstrun",
          null,
          h("span.serif", { style: { fontSize: "22px" } }, "The guide is not bundled with this build"),
          h("span.faint", { style: { maxWidth: "46ch", textAlign: "center" } }, "The same guide is published online."),
          h("button.btn.primary", { onclick: () => openExternal(GUIDE_ONLINE) }, "Open the online guide"),
        ),
      );
      return;
    }
    // Keys pressed inside the page never reach the app's window, so Escape is heard here too.
    doc.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeHelp();
      if (e.key === "F1") e.preventDefault();
    });
    // A link off the guide opens outside the app, never inside the frame.
    doc.addEventListener("click", (e) => {
      const a = (e.target as Element | null)?.closest?.("a[href]");
      const href = a?.getAttribute("href") ?? "";
      if (/^(https?:|mailto:)/i.test(href)) {
        e.preventDefault();
        openExternal(href);
      }
    });
  });

  window.addEventListener("keydown", guard, true);
  (document.getElementById("app") ?? document.body).append(screen);
}

export function closeHelp(): void {
  window.removeEventListener("keydown", guard, true);
  screen?.remove();
  screen = null;
}

/**
 * While the guide is open the app's own shortcuts are not: Space would flick the hidden
 * viewer and Escape would cancel a trace. Escape closes the guide instead. The keys still
 * do what they do in the screen itself (Tab, Enter), since nothing here prevents that.
 */
function guard(e: KeyboardEvent): void {
  e.stopImmediatePropagation();
  if (e.key === "Escape" || e.key === "F1") {
    e.preventDefault();
    if (e.key === "Escape") closeHelp();
  }
}

/** F1 opens the guide at the page about what is on screen. */
export function installHelp(store: Store): void {
  window.addEventListener("keydown", (e) => {
    if (e.key !== "F1" || screen) return;
    e.preventDefault();
    openHelp(helpPageFor(store.state));
  });
}
