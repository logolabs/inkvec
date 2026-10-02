/**
 * The export sheet.
 *
 * The sheet is a rail takeover rather than a full screen, and every size beside a format
 * is real: the backend builds the whole set in memory and reports what it actually
 * produced, so nothing here is an estimate. The share card lives in `card.ts`.
 *
 * It is a modal dialog in the WAI-ARIA sense (`dialog.ts`): `role="dialog"` with
 * `aria-modal="true"`, named by its heading; focus moves to the first format on open, Tab
 * and Shift+Tab stay inside, Escape or a press outside closes it, and focus goes back to
 * whatever opened it (the Export button, or wherever Ctrl+E was pressed). Each format is a
 * `role="checkbox"` button carrying `aria-checked`; the tick box drawn inside it is
 * decoration. Before this the tick was a `<span>` with `aria-checked` and no role, which
 * ARIA does not allow (axe-core `aria-allowed-attr`, five nodes), focus stayed behind in
 * the window (18 of 25 Tab presses landed outside the sheet) and Escape did nothing
 * (r2-product, 2026-10-02).
 */

import { fill, h, icon } from "../lib/dom";
import { api, type ExportRequest, type Formats, type PlannedFile } from "../lib/ipc";
import { CAN_PICK_FOLDER, pickFolder, revealAction } from "../lib/platform";
import { bytes, type Store } from "../lib/state";
import { holdModal } from "./dialog";
import { toast } from "./overlays";
import { remember, rememberedFormats } from "../lib/remember";

const PNG_SIZES = [512, 1024, 2048];

/** The sheet on screen, if one is: Ctrl+E while it is open goes back to it, not to a second. */
let openSheet: HTMLElement | null = null;

/** Build the export request for the drawing as it currently stands. */
export function requestFor(store: Store, formats: Formats): ExportRequest | null {
  const st = store.state;
  if (!st.svg || !st.report) return null;
  return {
    svg: st.svg,
    report: {
      meanDe00: st.report.meanDe00,
      coordinates: st.report.coordinates,
      paths: st.report.paths,
      tracedPx: st.report.tracedPx,
    },
    palette: st.palette.map((i) =>
      i.kind === "gradient" ? { hex: i.hex, traced: i.traced, share: i.share, stops: i.stops } : { hex: i.hex, traced: i.traced, share: i.share },
    ),
    losses: st.losses.map((l) => ({ text: l.text })),
    formats,
  };
}

/**
 * Open the export sheet inside `host` (the rail).
 *
 * `onBeforeExport` is the fresh full trace: export always re-traces, because what is on
 * screen may be a draft and nobody should ship a draft by accident.
 */
export function openExportSheet(
  store: Store,
  host: HTMLElement,
  onBeforeExport: () => Promise<void>,
): void {
  if (openSheet?.isConnected) {
    // Already open (Ctrl+E pressed again): the keyboard goes back into it.
    openSheet.querySelector<HTMLElement>(".format")?.focus();
    return;
  }
  // The formats ticked last time (lib/remember.ts), or these.
  const formats: Formats = rememberedFormats({
    svg: true,
    svgMinified: true,
    pngSizes: [...PNG_SIZES],
    favicon: false,
    assetPack: true,
  });
  let planned: PlannedFile[] = [];
  let destination = store.state.prefs?.outputFolder ?? null;

  const body = h("div.sheetbody", { role: "dialog", "aria-modal": "true", "aria-labelledby": "export-title" });
  // The backdrop over the rest of the rail. A press on it is a press outside the dialog,
  // which `holdModal` turns into a close (`onOutside`), like a press on the inert stage.
  const sheet = h("div.sheet");
  sheet.append(body);

  /** Undoes the modal hold; set once the sheet is on the page. */
  let release: (returnFocus?: boolean) => void = () => {};
  const close = () => {
    release();
    sheet.remove();
    if (openSheet === sheet) openSheet = null;
    store.set({ exportOpen: false });
  };

  const sizeOf = (group: string) =>
    planned.filter((p) => p.group === group).reduce((a, p) => a + p.bytes, 0);

  const replan = async () => {
    remember({ export: { ...formats, pngSizes: [...formats.pngSizes] } });
    const request = requestFor(store, formats);
    if (!request) return;
    try {
      planned = await api.planExport(request);
    } catch (e) {
      planned = [];
      toast(String(e), { kind: "bad" });
    }
    render();
  };

  // One format: a checkbox button. Its state is on the button (`aria-checked`); the tick box
  // inside is drawn from it by the stylesheet (`[aria-checked="true"] > .checkbox`).
  const row = (label: string, group: string, on: boolean, toggle: () => void) =>
    h(
      "button.format",
      { onclick: toggle, role: "checkbox", "aria-checked": String(on), "data-ctl": `format:${group}` },
      h("span.checkbox", { "aria-hidden": "true" }, icon("check", 11)),
      h("span", null, label),
      h("span.size", null, on ? bytes(sizeOf(group)) : "—"),
    );

  function render() {
    const total = planned.reduce((a, p) => a + p.bytes, 0);
    // Every replan rebuilds the sheet, which would drop the keyboard from the box just ticked;
    // focus goes back to its twin by `data-ctl`, as the rail does for its controls.
    const focused =
      document.activeElement instanceof HTMLElement && body.contains(document.activeElement)
        ? document.activeElement.getAttribute("data-ctl")
        : null;
    fill(
      body,
      h(
        "div.cardhead",
        null,
        h("h2.serif", { id: "export-title", style: { fontSize: "19px", fontWeight: "400", margin: "0" } }, "Export"),
        h("button.reset", { onclick: close, "data-ctl": "export-close" }, "Close"),
      ),
      row("SVG", "svg", formats.svg, () => {
        formats.svg = !formats.svg;
        void replan();
      }),
      row("SVG (minified)", "svgMinified", formats.svgMinified, () => {
        formats.svgMinified = !formats.svgMinified;
        void replan();
      }),
      row("PNG · 512 / 1024 / 2048", "png", formats.pngSizes.length > 0, () => {
        formats.pngSizes = formats.pngSizes.length ? [] : [...PNG_SIZES];
        void replan();
      }),
      row("ICO and favicon set", "favicon", formats.favicon, () => {
        formats.favicon = !formats.favicon;
        void replan();
      }),
      row("Asset pack (.zip)", "assetPack", formats.assetPack, () => {
        formats.assetPack = !formats.assetPack;
        void replan();
      }),
      // A browser has no folder to choose: the export downloads (one .zip when it is
      // several files), into wherever the browser keeps downloads.
      !CAN_PICK_FOLDER
        ? h(
            "div.format",
            { style: { cursor: "default" } },
            h("span.muted", { style: { fontSize: "11px" } }, "To"),
            h("span", { style: { flex: "1" } }, planned.length > 1 ? "Your downloads, as one .zip" : "Your downloads"),
          )
        : h(
        "div.format",
        { style: { cursor: "default" } },
        h("span.muted", { style: { fontSize: "11px" } }, "To"),
        h(
          "span",
          { style: { flex: "1", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" } },
          destination ?? "Choose a folder…",
        ),
        h(
          "button.reset",
          {
            "data-ctl": "export-folder",
            onclick: async () => {
              const picked = await chooseFolder(destination);
              if (picked) {
                destination = picked;
                render();
              }
            },
          },
          destination ? "Change" : "Choose",
        ),
      ),
      h(
        "button.btn.primary",
        {
          disabled: !planned.length,
          "data-ctl": "export-go",
          onclick: async () => {
            let target = destination;
            if (!CAN_PICK_FOLDER) target = "";
            else if (!target) {
              target = await chooseFolder(null);
              if (!target) return;
              destination = target;
            }
            close();
            await onBeforeExport();
            const request = requestFor(store, formats);
            if (!request) return;
            try {
              const written = await api.writeExport(request, target ?? "");
              toast(
                CAN_PICK_FOLDER
                  ? `${written.length} file${written.length === 1 ? "" : "s"} written to ${target}`
                  : `Downloaded ${written[0] ?? "the export"}`,
                { kind: "good", action: revealAction(CAN_PICK_FOLDER ? (written[0] ?? null) : null) },
              );
            } catch (e) {
              toast(String(e), { kind: "bad" });
            }
          },
        },
        planned.length
          ? `${CAN_PICK_FOLDER ? "Export" : "Download"} ${planned.length} file${planned.length === 1 ? "" : "s"} · ${bytes(total)}`
          : "Nothing selected",
      ),
    );
    if (focused) body.querySelector<HTMLElement>(`[data-ctl="${focused}"]`)?.focus({ preventScroll: true });
  }

  store.set({ exportOpen: true });
  host.append(sheet);
  openSheet = sheet;
  render();
  release = holdModal(body, {
    initial: () => body.querySelector<HTMLElement>(".format"),
    onEscape: close,
    onOutside: close,
    // The opener can be gone by the time the sheet closes (the rail foot is rebuilt when a
    // trace lands); the rail's own Export button is where the keyboard belongs then.
    fallback: () => host.querySelector<HTMLElement>(".railfoot .btn.primary"),
  });
  void replan();
}

function chooseFolder(current: string | null): Promise<string | null> {
  return pickFolder(current);
}
