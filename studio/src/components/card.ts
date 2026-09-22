/**
 * The share card: the composer that configures it and the canvas that draws it.
 *
 * The card is drawn on a canvas rather than rendered from SVG in Rust. resvg cannot shape
 * text without a font database and does not read woff2, so drawing it in the webview is
 * what makes the card use the app's own fonts and look like the app — which is the whole
 * point of a poster somebody will post.
 */

import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { h, icon } from "../lib/dom";
import { api } from "../lib/ipc";
import { count, de00, type Store } from "../lib/state";
import { closeOverlay, openModal, toast } from "./overlays";

interface CardOptions {
  size: "1600x900" | "1080x1080";
  theme: "dark" | "light";
  fileName: boolean;
  detail: boolean;
  numbers: boolean;
  /** How far the detail crop is magnified, in source pixels. */
  zoom: number;
}

const ZOOM_MIN = 4;
const ZOOM_MAX = 24;

type Toggle = "fileName" | "detail" | "numbers";

/**
 * The composer: a live preview beside the choices that shape it.
 *
 * Built once and updated in place. Rebuilding the dialog on every choice would drop
 * keyboard focus each time and make the preview flash, and the preview is the thing being
 * looked at while the choices are made.
 */
export function openCardComposer(store: Store): void {
  const opts: CardOptions = {
    size: "1600x900",
    theme: "dark",
    fileName: false,
    detail: true,
    numbers: true,
    zoom: 12,
  };

  // ------------------------------------------------------------- preview ---

  const stageImg = h("img", { alt: "Preview of the comparison card", draggable: "false" }) as HTMLImageElement;
  const stage = h("div.cardstage", null, stageImg, h("div.cardbusy", { "aria-hidden": "true" }, h("span.cardspin")));
  const caption = h("span.faint", { style: { fontSize: "11.5px" } });

  /** Only the newest draw may put its picture on screen; older ones finish and are dropped. */
  let latest = 0;
  const refresh = async (): Promise<void> => {
    const mine = ++latest;
    stage.classList.add("busy");
    try {
      const canvas = await drawCard(store, opts);
      if (mine !== latest) return;
      stageImg.src = canvas.toDataURL("image/png");
    } catch {
      if (mine === latest) toast("The card could not be drawn.", { kind: "bad" });
    } finally {
      if (mine === latest) stage.classList.remove("busy");
    }
  };

  const syncPreviewFrame = () => {
    const [w, hh] = dimensions(opts.size);
    stage.style.setProperty("--ar", String(w / hh));
    caption.textContent = `${w} × ${hh} px · PNG · ${opts.theme}`;
  };

  // ------------------------------------------------------------ controls ---

  const syncers: (() => void)[] = [];
  const changed = () => {
    syncers.forEach((f) => f());
    syncPreviewFrame();
    void refresh();
  };

  const segmented = (
    key: "size" | "theme",
    label: string,
    choices: { value: string; label: string; sub?: string }[],
  ) => {
    const buttons = choices.map((c) =>
      h(
        "button",
        {
          type: "button",
          role: "radio",
          onclick: () => {
            (opts[key] as string) = c.value;
            changed();
          },
        },
        h("span", null, c.label),
        c.sub ? h("small", null, c.sub) : null,
      ),
    );
    syncers.push(() =>
      buttons.forEach((b, i) => b.setAttribute("aria-checked", String(opts[key] === choices[i].value))),
    );
    return group(label, h("div.cardseg", { role: "radiogroup", "aria-label": label }, ...buttons));
  };

  const toggle = (key: Toggle, label: string, note: string) => {
    const sw = h("span.switch");
    const row = h(
      "button.optrow",
      {
        type: "button",
        role: "switch",
        onclick: () => {
          opts[key] = !opts[key];
          changed();
        },
      },
      h("span.optext", null, h("span.optlabel", null, label), h("span.optnote", null, note)),
      sw,
    );
    syncers.push(() => {
      row.setAttribute("aria-checked", String(opts[key]));
      sw.setAttribute("aria-checked", String(opts[key]));
    });
    return row;
  };

  // A slider, so the redraw happens once when it is let go rather than on every pixel of
  // the drag; the readout follows the thumb in the meantime.
  const zoomReadout = h("span.num", null, `${opts.zoom}×`);
  const zoomSlider = h("input.slider", {
    type: "range",
    min: String(ZOOM_MIN),
    max: String(ZOOM_MAX),
    step: "1",
    value: String(opts.zoom),
    "aria-label": "Detail crop magnification",
    oninput: (e: Event) => {
      zoomReadout.textContent = `${(e.target as HTMLInputElement).value}×`;
    },
    onchange: (e: Event) => {
      opts.zoom = Number((e.target as HTMLInputElement).value);
      changed();
    },
  }) as HTMLInputElement;
  const zoomRow = h(
    "div.optslider",
    null,
    h("div.optsliderhead", null, h("span", null, "Detail magnification"), zoomReadout),
    zoomSlider,
    h("div.stops", null, h("span", null, `${ZOOM_MIN}×`), h("span", null, `${ZOOM_MAX}×`)),
  );
  syncers.push(() => {
    zoomSlider.disabled = !opts.detail;
    zoomRow.classList.toggle("off", !opts.detail);
  });

  // ------------------------------------------------------------- actions ---

  /** Disable a button and relabel it while `work` runs, so a slow save cannot be double-clicked. */
  const busyButton = (btn: HTMLButtonElement, working: string, work: () => Promise<void>) => async () => {
    const label = btn.lastChild?.textContent ?? "";
    btn.disabled = true;
    if (btn.lastChild) btn.lastChild.textContent = working;
    try {
      await work();
    } finally {
      btn.disabled = false;
      if (btn.lastChild) btn.lastChild.textContent = label;
    }
  };

  const toBlob = async (): Promise<Blob | null> => {
    const canvas = await drawCard(store, opts);
    return new Promise((r) => canvas.toBlob(r, "image/png"));
  };

  const copyBtn = h("button.btn", { type: "button" }, icon("copy", 15), h("span", null, "Copy image")) as HTMLButtonElement;
  copyBtn.onclick = busyButton(copyBtn, "Copying…", async () => {
    const blob = await toBlob();
    if (!blob) return;
    try {
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      toast("Card copied.", { kind: "good" });
    } catch {
      toast("This system would not take an image on the clipboard; save it instead.", { kind: "bad" });
    }
  });

  const saveBtn = h("button.btn.primary", { type: "button" }, icon("download", 15), h("span", null, "Save PNG")) as HTMLButtonElement;
  saveBtn.onclick = busyButton(saveBtn, "Saving…", async () => {
    const stem = store.state.source?.name.replace(/\.[^.]+$/, "") ?? "inkvec";
    const path = await save({
      defaultPath: `${stem}-card.png`,
      filters: [{ name: "PNG", extensions: ["png"] }],
    });
    if (!path) return;
    const blob = await toBlob();
    if (!blob) return;
    await api.saveBytes(path, [...new Uint8Array(await blob.arrayBuffer())]);
    closeOverlay();
    toast(`Card saved to ${path}`, {
      kind: "good",
      action: { label: "Show in folder", run: () => void revealItemInDir(path) },
    });
  });

  // ------------------------------------------------------------- the box ---

  const content = h(
    "div.modal.composer",
    { tabindex: "-1" },
    h(
      "div.composerhead",
      null,
      h(
        "div",
        null,
        h("h2", null, "Save comparison card"),
        h("p", null, "The before and after on one poster, with the measurements behind it."),
      ),
      h("button.btn.icon.ghost", { type: "button", "aria-label": "Close", onclick: closeOverlay }, icon("x", 16)),
    ),
    h(
      "div.composerbody",
      null,
      h("div.composerpreview", null, stage, caption),
      h(
        "div.composerside",
        null,
        segmented("size", "Format", [
          { value: "1600x900", label: "Landscape", sub: "1600 × 900" },
          { value: "1080x1080", label: "Square", sub: "1080 × 1080" },
        ]),
        segmented("theme", "Appearance", [
          { value: "dark", label: "Dark" },
          { value: "light", label: "Light" },
        ]),
        group(
          "On the card",
          h(
            "div.optlist",
            null,
            toggle("numbers", "The numbers", "Colour difference, coordinates and file size."),
            toggle("detail", "A zoomed detail", "The vector edge cutting through the source's pixels."),
            zoomRow,
            // Off by default: agencies trace unreleased client logos and must not be forced
            // to publish the name to use the card.
            toggle("fileName", "The file name", "Off by default, so unreleased work stays private."),
          ),
        ),
        h("div.composeractions", null, saveBtn, copyBtn),
      ),
    ),
  );

  openModal(content);
  changed();
}

function group(label: string, body: HTMLElement): HTMLElement {
  return h("div.optgroup", null, h("span.eyebrow", null, label), body);
}

function dimensions(size: CardOptions["size"]): [number, number] {
  return size === "1600x900" ? [1600, 900] : [1080, 1080];
}

// ------------------------------------------------------------------ drawing ---

/**
 * Draw the card.
 *
 * A designed poster, not a screenshot with a border: the before/after pair, one zoomed
 * detail crop where the accuracy is actually visible, the headline numbers, and a small
 * mark. People post it because it looks good, and every one of those posts is a download
 * we did not pay for.
 */
async function drawCard(store: Store, opts: CardOptions): Promise<HTMLCanvasElement> {
  const [W, H] = dimensions(opts.size);
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

  // `fonts.ready` resolves as soon as nothing is loading, which is before a face nobody
  // has drawn with yet has started to; asking for the faces by name is what loads them.
  await Promise.all([
    document.fonts.load('500 20px "Playfair Studio"'),
    document.fonts.load('400 20px "Inter Studio"'),
    document.fonts.load('500 20px "Inter Studio"'),
  ]).catch(() => undefined);

  g.fillStyle = paper;
  g.fillRect(0, 0, W, H);
  const glow = g.createRadialGradient(W * 0.2, -H * 0.1, 0, W * 0.2, -H * 0.1, W * 0.6);
  glow.addColorStop(0, "rgba(201,117,74,.16)");
  glow.addColorStop(1, "rgba(201,117,74,0)");
  g.fillStyle = glow;
  g.fillRect(0, 0, W, H);

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
  const pairH = Math.round(square ? H * 0.3 : H * 0.44);

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
  const sized = svg ? withIntrinsicSize(svg) : null;

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
  if (sized) await drawInto(sized.url, rightX + half + 12, "SVG", accent);

  // The detail crop: where the accuracy is actually visible.
  if (opts.detail && source && sized) {
    const cropY = topY + pairH + 14;
    const cropH = H - cropY - pad - 40;
    if (cropH > 60) {
      const corner = store.state.worstCorner;
      const zoom = opts.zoom;
      const cx = corner?.x ?? sized.w / 2;
      const cy = corner?.y ?? sized.h / 2;
      // Both layers are laid out in the drawing's own coordinates, exactly as the viewer
      // does, so the edge lands where the source's anti-aliasing says it should.
      const ox = rightX + rightW / 2 - cx * zoom;
      const oy = cropY + cropH / 2 - cy * zoom;
      g.save();
      g.beginPath();
      g.rect(rightX, cropY, rightW, cropH);
      g.clip();
      g.fillStyle = dark ? "#141210" : "#f0ece6";
      g.fillRect(rightX, cropY, rightW, cropH);
      const img = await loadImage(source.preview);
      g.imageSmoothingEnabled = false;
      g.drawImage(img, ox, oy, sized.w * zoom, sized.h * zoom);
      g.imageSmoothingEnabled = true;
      // The pixel grid over the source, and the vector edge cutting through it.
      g.strokeStyle = dark ? "rgba(250,248,245,.09)" : "rgba(26,24,22,.12)";
      g.lineWidth = 1;
      const offX = ((ox % zoom) + zoom) % zoom;
      const offY = ((oy % zoom) + zoom) % zoom;
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
      const vec = await loadImage(sized.url);
      g.drawImage(vec, ox, oy, sized.w * zoom, sized.h * zoom);
      g.restore();
      g.fillStyle = faint;
      g.font = `400 ${Math.round(W * 0.0082)}px "Inter Studio", Inter, sans-serif`;
      const worst = corner ? `edge within ${de00(corner.de00)} dE00 · ${zoom}×` : `${zoom}× detail`;
      g.fillText(worst, rightX + rightW - g.measureText(worst).width - 10, cropY + cropH - 10);
    }
  }

  return canvas;
}

/**
 * The traced document as an image URL, with an explicit size.
 *
 * The tracer writes a `viewBox` and, depending on the options, no `width` or `height`. An
 * SVG with only a `viewBox` has no intrinsic size of its own, and what a webview reports
 * for one differs by engine — so a card laid out from `img.width` could place the drawing
 * at the wrong scale. Stating the size from the `viewBox` removes the question.
 */
function withIntrinsicSize(svg: string): { url: string; w: number; h: number } {
  let w = 512;
  let hh = 512;
  let text = svg;
  const root = new DOMParser().parseFromString(svg, "image/svg+xml").documentElement;
  if (root && root.nodeName === "svg" && !root.querySelector("parsererror")) {
    const vb = (root.getAttribute("viewBox") ?? "").trim().split(/[\s,]+/).map(Number);
    if (vb.length === 4 && vb.every(Number.isFinite) && vb[2] > 0 && vb[3] > 0) {
      w = vb[2];
      hh = vb[3];
    }
    root.setAttribute("width", String(w));
    root.setAttribute("height", String(hh));
    text = new XMLSerializer().serializeToString(root);
  }
  return { url: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(text)}`, w, h: hh };
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = src;
  });
}
