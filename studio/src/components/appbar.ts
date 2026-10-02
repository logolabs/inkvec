/**
 * The app bar across the top of the window: the mark and the app's name, the open file's
 * name and size, the tab switcher, and the buttons that are not about any one tab (Open,
 * Recent, Settings, About, Showcase, Help) beside the window controls.
 *
 * Drawn by `main.ts`, which redraws it whenever the tab, the open image, the preferences or
 * the capabilities change. The bar is also the window's drag region on the desktop, where
 * the window has no system title bar.
 */

import { appMark, fill, h } from "../lib/dom";
import { APP_NAME, WEB } from "../lib/platform";
import { modKey, type Store } from "../lib/state";
import { helpPageFor, openHelp } from "./help";
import { closeOverlay, openPopover } from "./overlays";
import { windowControls } from "./wincontrols";

/** What the bar's buttons ask the app to do. */
export interface AppBarActions {
  /** Ask for a file to open (the Open… button and the Recent menu's last item). */
  openFile(): void;
  /** Open one of the recently opened files, by its path on this computer. */
  openPath(path: string): void;
}

/** Draw the app bar into `appbar` from the state as it is now, replacing what was there. */
export function renderAppBar(appbar: HTMLElement, store: Store, act: AppBarActions): void {
  const st = store.state;

  fill(
    appbar,
    // The page's level-one heading: what a screen reader's heading list starts from.
    h("h1.brand", null, appMark(18), APP_NAME),
    st.tab === "vectorize" && st.source
      ? h(
          "div.filechip",
          null,
          h("span.name", null, st.source.name),
          h("span.muted.num", null, `${st.source.width} × ${st.source.height} · ${st.source.container}`),
        )
      : null,
    h(
      "div.seg.apptabs",
      { role: "group", "aria-label": "Tabs" },
      ...(
        [
          ["vectorize", "Vectorize"],
          ["minify", "Minify SVG"],
          ["fabricate", "Fabricate"],
          // A folder of images in, a folder of SVGs out: nothing a browser tab can do.
          ...(WEB ? [] : ([["batch", "Batch"]] as const)),
        ] as const
      ).map(([id, label]) =>
        h("button", { "aria-pressed": String(st.tab === id), onclick: () => store.set({ tab: id }) }, label),
      ),
    ),
    // On a phone this row scrolls sideways on its own (app.css, "compact"), under the tabs.
    h(
      "div.appactions",
      null,
      h("button.btn.ghost.compact", { onclick: () => act.openFile() }, `Open…`),
      // Recent files are paths on this computer; a browser never learns them.
      WEB
        ? null
        : h(
            "button.btn.ghost.compact",
            {
              disabled: !(st.prefs?.recent.length),
              onclick: (e: Event) => openRecent(e.currentTarget as HTMLElement, store, act),
            },
            "Recent",
          ),
      h("button.btn.ghost.compact", { onclick: () => store.set({ screen: "settings" }) }, "Settings"),
      h("button.btn.ghost.compact", { onclick: () => store.set({ screen: "about" }) }, "About"),
      h("button.btn.ghost.compact", { title: "Before and after on real logos, and how Inkvec compares", onclick: () => store.set({ screen: "showcase" }) }, "Showcase"),
      h("button.btn.ghost.compact", { title: "The user guide (F1)", onclick: () => openHelp(helpPageFor(store.state)) }, "Help"),
      h("div.sep"),
      windowControls(),
    ),
  );
  appbar.setAttribute("data-tauri-drag-region", "");
}

/** The Recent menu, under its button: each remembered file by name, then Open…. */
function openRecent(anchor: HTMLElement, store: Store, act: AppBarActions): void {
  const recent = store.state.prefs?.recent ?? [];
  openPopover(
    anchor,
    h(
      "div.menu",
      null,
      ...recent.map((path) =>
        h(
          "button.item",
          {
            onclick: () => {
              closeOverlay();
              act.openPath(path);
            },
          },
          h("span", { style: { flex: "1", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" } }, path.split(/[\\/]/).pop()),
        ),
      ),
      h("div.rule"),
      h(
        "button.item",
        {
          onclick: () => {
            closeOverlay();
            act.openFile();
          },
        },
        h("span", { style: { flex: "1" } }, "Open…"),
        h("span.when", null, `${modKey(store.state.caps?.platform)}+O`),
      ),
    ),
  );
}
