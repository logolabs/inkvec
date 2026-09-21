/**
 * The export sheet, and the share card.
 *
 * The sheet is a rail takeover rather than a full screen, and every size beside a format
 * is real: the backend builds the whole set in memory and reports what it actually
 * produced, so nothing here is an estimate.
 *
 * The card is drawn on a canvas rather than rendered from SVG in Rust. resvg cannot shape
 * text without a font database and does not read woff2, so drawing it in the webview is
 * what makes the card use the app's own fonts and look like the app — which is the whole
 * point of a poster somebody will post.
 */

import { open, save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { fill, h, icon } from "../lib/dom";
import { api, type ExportRequest, type Formats, type PlannedFile } from "../lib/ipc";
import { bytes, count, de00, type Store } from "../lib/state";
import { closeOverlay, openModal, toast } from "./overlays";

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
    palette: st.palette.map((i) => ({ hex: i.hex, traced: i.traced, share: i.share })),
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
      h(
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
            if (!target) {
              target = await chooseFolder(null);
              if (!target) return;
              destination = target;
            }
            close();
            await onBeforeExport();
            const request = requestFor(store, formats);
            if (!request) return;
            try {
              const written = await api.writeExport(request, target);
              toast(`${written.length} file${written.length === 1 ? "" : "s"} written to ${target}`, {
                kind: "good",
                action: { label: "Show in folder", run: () => void revealItemInDir(written[0]) },
              });
            } catch (e) {
              toast(String(e), { kind: "bad" });
            }
          },
        },
        planned.length
          ? `Export ${planned.length} file${planned.length === 1 ? "" : "s"} · ${bytes(total)}`
          : "Nothing selected",
      ),
    );
  }

  store.set({ exportOpen: true });
  host.append(sheet);
  render();
  void replan();
}

async function chooseFolder(current: string | null): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, defaultPath: current ?? undefined });
  return typeof picked === "string" ? picked : null;
}

// --------------------------------------------------------------- share card ---

interface CardOptions {
  size: "1600x900" | "1080x1080";
  theme: "dark" | "light";
  fileName: boolean;
  detail: boolean;
  numbers: boolean;
}

/** The composer. One button in the workspace; the card costs almost nothing to build. */
export function openCardComposer(store: Store): void {
  const opts: CardOptions = {
    size: "1600x900",
    theme: "dark",
    fileName: false,
    detail: true,
    numbers: true,
  };
  const previewBox = h("div", {
    style: {
      border: "1px solid var(--rule2)",
      borderRadius: "var(--radius)",
      overflow: "hidden",
      background: "var(--stage)",
      display: "grid",
      placeItems: "center",
      padding: "8px",
    },
  });

  const refresh = async () => {
    const canvas = await drawCard(store, opts);
    const img = h("img", { src: canvas.toDataURL("image/png"), style: { width: "100%", display: "block" } });
    fill(previewBox, img);
  };

  const pick = <K extends keyof CardOptions>(key: K, value: CardOptions[K], label: string) =>
    h(
      "button",
      {
        style: {
          flex: "1",
          height: "30px",
          display: "grid",
          placeItems: "center",
          borderRadius: "7px",
          border: `1px solid ${opts[key] === value ? "var(--accent)" : "var(--rule2)"}`,
          background: opts[key] === value ? "var(--accent-soft)" : "transparent",
          fontSize: "12px",
          color: opts[key] === value ? "var(--ink)" : "var(--faint)",
        },
        onclick: () => {
          opts[key] = value;
          rebuild();
        },
      },
      label,
    );

  const toggle = (key: "fileName" | "detail" | "numbers", label: string) =>
    h(
      "button",
      {
        style: {
          display: "flex",
          alignItems: "center",
          gap: "10px",
          padding: "8px 10px",
          border: "1px solid var(--rule2)",
          borderRadius: "var(--radius)",
          background: "var(--paper)",
          width: "100%",
        },
        onclick: () => {
          opts[key] = !opts[key];
          rebuild();
        },
      },
      h("span.dim", { style: { flex: "1", textAlign: "left", fontSize: "12.5px" } }, label),
      h("span.switch", { "aria-checked": String(opts[key]) }),
    );

  const content = h("div.modal");
  function rebuild() {
    fill(
      content,
      h("h2", null, "Save comparison card"),
      previewBox,
      h("div", { style: { display: "flex", gap: "8px" } }, pick("size", "1600x900", "1600 × 900"), pick("size", "1080x1080", "1080 × 1080")),
      h("div", { style: { display: "flex", gap: "8px" } }, pick("theme", "dark", "Dark"), pick("theme", "light", "Light")),
      // The file name is off by default: agencies trace unreleased client logos and
      // must not be forced to publish the name to use the card.
      toggle("fileName", "Include the file name"),
      toggle("detail", "Include the detail crop"),
      toggle("numbers", "Include the numbers"),
      h(
        "div.actions",
        null,
        h(
          "button.btn",
          {
            onclick: async () => {
              const canvas = await drawCard(store, opts);
              canvas.toBlob(async (blob) => {
                if (!blob) return;
                try {
                  await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
                  toast("Card copied.", { kind: "good" });
                } catch {
                  toast("This system would not take an image on the clipboard; save it instead.", { kind: "bad" });
                }
              }, "image/png");
            },
          },
          "Copy to clipboard",
        ),
        h(
          "button.btn.primary",
          {
            onclick: async () => {
              const stem = store.state.source?.name.replace(/\.[^.]+$/, "") ?? "inkvec";
              const path = await save({
                defaultPath: `${stem}-card.png`,
                filters: [{ name: "PNG", extensions: ["png"] }],
              });
              if (!path) return;
              const canvas = await drawCard(store, opts);
              const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/png"));
              if (!blob) return;
              await api.saveBytes(path, [...new Uint8Array(await blob.arrayBuffer())]);
              closeOverlay();
              toast(`Card saved to ${path}`, {
                kind: "good",
                action: { label: "Show in folder", run: () => void revealItemInDir(path) },
              });
            },
          },
          "Save PNG",
        ),
      ),
    );
    void refresh();
  }

  openModal(content);
  rebuild();
}

/**
 * Draw the card.
 *
 * A designed poster, not a screenshot with a border: the before/after pair, one zoomed
 * detail crop where the accuracy is actually visible, the headline numbers, and a small
 * mark. People post it because it looks good, and every one of those posts is a download
 * we did not pay for.
 */
async function drawCard(store: Store, opts: CardOptions): Promise<HTMLCanvasElement> {
  const [W, H] = opts.size === "1600x900" ? [1600, 900] : [1080, 1080];
  const canvas = document.createElement("canvas");
  canvas.width = W;
  canvas.height = H;
  const g = canvas.getContext("2d");
  if (!g) return canvas;

  const dark = opts.theme === "dark";
  const paper = dark ? "#1a1816" : "#faf8f5";
  const ink = dark ? "#faf8f5" : "#1a1816";
  const muted = dark ? "rgba(250,248,245,.4)" : "rgba(26,24,22,.46)";
  const faint = dark ? "rgba(250,248,245,.55)" : "rgba(26,24,22,.58)";
  const accent = dark ? "#c9754a" : "#9c5730";
  const checkerA = dark ? "#26231f" : "#ffffff";
  const checkerB = dark ? "#211f1c" : "#e8e3db";

  g.fillStyle = paper;
  g.fillRect(0, 0, W, H);
  const glow = g.createRadialGradient(W * 0.2, -H * 0.1, 0, W * 0.2, -H * 0.1, W * 0.6);
  glow.addColorStop(0, "rgba(201,117,74,.16)");
  glow.addColorStop(1, "rgba(201,117,74,0)");
  g.fillStyle = glow;
  g.fillRect(0, 0, W, H);

  await document.fonts.ready;
  const pad = Math.round(W * 0.042);
  const square = opts.size === "1080x1080";
  const leftW = square ? W - pad * 2 : Math.round(W * 0.46);

  // ---- left: the claim and the numbers ----
  g.fillStyle = muted;
  g.font = `500 ${Math.round(W * 0.0105)}px "Inter Studio", Inter, sans-serif`;
  g.letterSpacing = "3px";
  g.fillText("PNG TRACED TO SVG", pad, pad + 14);
  g.letterSpacing = "0px";

  g.fillStyle = ink;
  const titleSize = Math.round(W * (square ? 0.05 : 0.039));
  g.font = `500 ${titleSize}px "Playfair Studio", Georgia, serif`;
  g.fillText("A logo, measured", pad, pad + titleSize + 26);
  g.fillText("rather than guessed.", pad, pad + titleSize * 2 + 30);

  let y = pad + titleSize * 2 + 96;
  if (opts.fileName && store.state.source) {
    g.fillStyle = faint;
    g.font = `400 ${Math.round(W * 0.013)}px "Inter Studio", Inter, sans-serif`;
    g.fillText(store.state.source.name, pad, y);
    y += 40;
  }

  const r = store.state.report;
  if (opts.numbers && r) {
    const figures: [string, string][] = [
      [de00(r.meanDe00), "mean dE00"],
      [count(r.coordinates), "coordinates"],
      [`${(r.bytes / 1024).toFixed(1)} KB`, "SVG"],
    ];
    let x = pad;
    for (const [value, label] of figures.slice(0, square ? 2 : 3)) {
      g.fillStyle = value === de00(r.meanDe00) ? accent : ink;
      const size = Math.round(W * 0.026);
      g.font = `500 ${size}px "Playfair Studio", Georgia, serif`;
      g.fillText(value, x, y + size);
      g.fillStyle = muted;
      g.font = `400 ${Math.round(W * 0.0085)}px "Inter Studio", Inter, sans-serif`;
      g.fillText(label, x, y + size + 20);
      x += Math.max(g.measureText(label).width, 120) + Math.round(W * 0.05);
    }
  }

  // ---- the mark ----
  g.fillStyle = faint;
  g.font = `400 ${Math.round(W * 0.0095)}px "Inter Studio", Inter, sans-serif`;
  g.fillText("traced with Inkvec Studio · by LogoLabs", pad, H - pad);

  // ---- right: the pair, and the detail crop ----
  const rightX = square ? pad : pad + leftW;
  const rightW = W - rightX - pad;
  const topY = square ? Math.round(H * 0.42) : pad;
  const pairH = Math.round((square ? H * 0.3 : H * 0.44));

  const checker = (x: number, yy: number, w: number, hh: number) => {
    const cell = 14;
    for (let i = 0; i * cell < w; i++) {
      for (let j = 0; j * cell < hh; j++) {
        g.fillStyle = (i + j) % 2 ? checkerA : checkerB;
        g.fillRect(x + i * cell, yy + j * cell, Math.min(cell, w - i * cell), Math.min(cell, hh - j * cell));
      }
    }
  };

  const half = Math.round((rightW - 12) / 2);
  const source = store.state.source;
  const svg = store.state.svg;

  const drawInto = async (src: string, x: number, label: string, labelColour: string) => {
    g.save();
    g.beginPath();
    g.rect(x, topY, half, pairH);
    g.clip();
    checker(x, topY, half, pairH);
    const img = await loadImage(src);
    const scale = Math.min((half - 40) / img.width, (pairH - 40) / img.height);
    const w = img.width * scale;
    const hh = img.height * scale;
    g.drawImage(img, x + (half - w) / 2, topY + (pairH - hh) / 2, w, hh);
    g.restore();
    g.fillStyle = labelColour;
    g.font = `500 ${Math.round(W * 0.0085)}px "Inter Studio", Inter, sans-serif`;
    g.letterSpacing = "2px";
    g.fillText(label, x + 12, topY + 22);
    g.letterSpacing = "0px";
  };

  if (source) await drawInto(source.preview, rightX, source.container.toUpperCase(), muted);
  if (svg) await drawInto(svgDataUrl(svg), rightX + half + 12, "SVG", accent);

  // The detail crop: 12x, where the accuracy is actually visible.
  if (opts.detail && source && svg) {
    const cropY = topY + pairH + 14;
    const cropH = H - cropY - pad - 40;
    if (cropH > 60) {
      const corner = store.state.worstCorner;
      const zoom = 12;
      const cx = corner?.x ?? (store.state.report?.tracedPx ?? 512) / 2;
      const cy = corner?.y ?? (store.state.report?.tracedPx ?? 512) / 2;
      g.save();
      g.beginPath();
      g.rect(rightX, cropY, rightW, cropH);
      g.clip();
      g.fillStyle = dark ? "#141210" : "#f0ece6";
      g.fillRect(rightX, cropY, rightW, cropH);
      const img = await loadImage(source.preview);
      g.imageSmoothingEnabled = false;
      g.drawImage(img, rightX + rightW / 2 - cx * zoom, cropY + cropH / 2 - cy * zoom, img.width * zoom, img.height * zoom);
      g.imageSmoothingEnabled = true;
      const vec = await loadImage(svgDataUrl(svg));
      g.globalAlpha = 0.0;
      g.drawImage(vec, 0, 0, 1, 1);
      g.globalAlpha = 1;
      // The pixel grid over the source, and the vector edge cutting through it.
      g.strokeStyle = dark ? "rgba(250,248,245,.09)" : "rgba(26,24,22,.12)";
      g.lineWidth = 1;
      const offX = (rightX + rightW / 2 - cx * zoom) % zoom;
      const offY = (cropY + cropH / 2 - cy * zoom) % zoom;
      for (let x = rightX + offX; x < rightX + rightW; x += zoom) {
        g.beginPath();
        g.moveTo(Math.round(x) + 0.5, cropY);
        g.lineTo(Math.round(x) + 0.5, cropY + cropH);
        g.stroke();
      }
      for (let yy = cropY + offY; yy < cropY + cropH; yy += zoom) {
        g.beginPath();
        g.moveTo(rightX, Math.round(yy) + 0.5);
        g.lineTo(rightX + rightW, Math.round(yy) + 0.5);
        g.stroke();
      }
      g.drawImage(vec, rightX + rightW / 2 - cx * zoom, cropY + cropH / 2 - cy * zoom, vec.width * zoom, vec.height * zoom);
      g.restore();
      g.fillStyle = faint;
      g.font = `400 ${Math.round(W * 0.0082)}px "Inter Studio", Inter, sans-serif`;
      const worst = corner ? `edge within ${de00(corner.de00)} dE00 · 12×` : "12× detail";
      g.fillText(worst, rightX + rightW - g.measureText(worst).width - 10, cropY + cropH - 10);
    }
  }

  return canvas;
}

function svgDataUrl(svg: string): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = src;
  });
}
