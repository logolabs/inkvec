/**
 * The export sheet.
 *
 * The sheet is a rail takeover rather than a full screen, and every size beside a format
 * is real: the backend builds the whole set in memory and reports what it actually
 * produced, so nothing here is an estimate. The share card lives in `card.ts`.
 */

import { fill, h, icon } from "../lib/dom";
import { api, type ExportRequest, type Formats, type PlannedFile } from "../lib/ipc";
import { CAN_PICK_FOLDER, pickFolder, revealAction } from "../lib/platform";
import { bytes, type Store } from "../lib/state";
import { toast } from "./overlays";

const PNG_SIZES = [512, 1024, 2048];

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
  const formats: Formats = {
    svg: true,
    svgMinified: true,
    pngSizes: [...PNG_SIZES],
    favicon: false,
    assetPack: true,
  };
  let planned: PlannedFile[] = [];
  let destination = store.state.prefs?.outputFolder ?? null;

  const body = h("div.sheetbody");
  const sheet = h("div.sheet", {
    onmousedown: (e: MouseEvent) => {
      if (e.target === sheet) close();
    },
  });
  sheet.append(body);

  const close = () => {
    sheet.remove();
    store.set({ exportOpen: false });
  };

  const sizeOf = (group: string) =>
    planned.filter((p) => p.group === group).reduce((a, p) => a + p.bytes, 0);

  const replan = async () => {
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

  const row = (label: string, group: string, on: boolean, toggle: () => void) =>
    h(
      "button.format",
      { onclick: toggle, "aria-pressed": String(on) },
      h(`span.checkbox`, { "aria-checked": String(on) }, icon("check", 11)),
      h("span", null, label),
      h("span.size", null, on ? bytes(sizeOf(group)) : "—"),
    );

  function render() {
    const total = planned.reduce((a, p) => a + p.bytes, 0);
    fill(
      body,
      h(
        "div.cardhead",
        null,
        h("span.serif", { style: { fontSize: "19px" } }, "Export"),
        h("button.reset", { onclick: close }, "Close"),
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
  }

  store.set({ exportOpen: true });
  host.append(sheet);
  render();
  void replan();
}

function chooseFolder(current: string | null): Promise<string | null> {
  return pickFolder(current);
}
