/**
 * The SVG Minimizer tab.
 *
 * A different emotional register entirely, and worth exploiting: tracing takes a second,
 * minifying takes about thirty milliseconds. This tab is instant and should feel it — the
 * result is the screen, it updates as you move the control, and there is no progress to
 * show because there is nothing to wait for.
 *
 * The tolerance control explains itself in its own label. That sentence *is* the
 * interface; the number is inside it, not in a tooltip hanging off it.
 */

import { open, save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

import { fill, h, icon } from "../lib/dom";
import { api } from "../lib/ipc";
import { bytes, count, de00, percent, seconds, type Store } from "../lib/state";
import { toast } from "../components/overlays";

export function createMinify(store: Store): HTMLElement {
  const before = h("div.pane", null, h("span.eyebrow.panelabel", null, "Before"));
  const after = h("div.pane", null, h("span.eyebrow.panelabel", null, "After"));
  const divider = h("div.wipehandle", { role: "separator", "aria-label": "Wipe" });
  const panes = h("div.panes", null, before, after, divider);
  const tools = h("div.viewertools");
  const stage = h("section.stage", null, tools, panes);

  // The same three arrangements as the Vectorize viewer, for the same reason: this tab's
  // whole claim is that nothing moved, and the only way to believe that is to look.
  let view: "side" | "wipe" | "ab" = "side";
  let wipe = 0.5;
  let flicked = false;

  // What the drawings are shown on. A logo is very often one dark colour, and a dark logo
  // on a dark stage is a blank pane: nothing is wrong with the drawing, it just cannot be
  // seen. So the backdrop follows the artwork unless it is told otherwise.
  let backdrop: "auto" | "light" | "dark" = "auto";
  let autoTone: "light" | "dark" | null = null;
  let measuredFor: string | null = null;
  const applyBackdrop = () => {
    const tone = backdrop === "auto" ? autoTone : backdrop;
    panes.classList.toggle("onlight", tone === "light");
    panes.classList.toggle("ondark", tone === "dark");
  };

  const layout = () => {
    panes.classList.toggle("stacked", view !== "side");
    divider.style.display = view === "wipe" ? "" : "none";
    if (view === "side") {
      before.classList.remove("hidden");
      after.classList.remove("hidden");
      before.style.clipPath = "";
      after.style.clipPath = "";
    } else if (view === "wipe") {
      before.classList.remove("hidden");
      after.classList.remove("hidden");
      const pct = `${(wipe * 100).toFixed(2)}%`;
      divider.style.left = pct;
      before.style.clipPath = `inset(0 ${(100 - wipe * 100).toFixed(2)}% 0 0)`;
      after.style.clipPath = `inset(0 0 0 ${pct})`;
    } else {
      before.style.clipPath = "";
      after.style.clipPath = "";
      before.classList.toggle("hidden", !flicked);
      after.classList.toggle("hidden", flicked);
    }
  };

  divider.addEventListener("pointerdown", (e: PointerEvent) => {
    divider.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      const r = panes.getBoundingClientRect();
      wipe = Math.min(0.98, Math.max(0.02, (m.clientX - r.left) / r.width));
      layout();
    };
    const up = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", up);
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", up);
  });

  window.addEventListener("keydown", (e) => {
    if (store.state.tab !== "minify" || store.state.screen || e.key !== " " || e.repeat) return;
    if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
    e.preventDefault();
    flicked = true;
    layout();
  });
  window.addEventListener("keyup", (e) => {
    if (e.key === " " && flicked) {
      flicked = false;
      layout();
    }
  });

  const renderTools = () => {
    const m = store.state.minify;
    const r = m.result;
    fill(
      tools,
      h(
        "div.seg",
        null,
        ...(["side", "wipe", "ab"] as const).map((v) =>
          h(
            "button",
            {
              "aria-pressed": String(view === v),
              disabled: !m.before,
              onclick: () => {
                view = v;
                layout();
                renderTools();
              },
            },
            v === "side" ? "Side by side" : v === "wipe" ? "Wipe" : "A/B",
          ),
        ),
      ),
      h("span.muted", { style: { fontSize: "11.5px" } }, "hold ", h("kbd", null, "Space"), " to flick"),
      h(
        "div",
        { style: { display: "flex", alignItems: "center", gap: "8px", marginLeft: "12px" } },
        h("span.muted", { style: { fontSize: "11.5px" } }, "Backdrop"),
        h(
          "div.seg",
          { role: "group", "aria-label": "Backdrop" },
          ...(["auto", "light", "dark"] as const).map((b) =>
            h(
              "button",
              {
                "aria-pressed": String(backdrop === b),
                disabled: !m.before,
                onclick: () => {
                  backdrop = b;
                  applyBackdrop();
                  renderTools();
                },
              },
              b === "auto" ? "Auto" : b === "light" ? "Light" : "Dark",
            ),
          ),
        ),
      ),
      // The promise, stated where somebody is looking for a reason to doubt it.
      h(
        "span.muted",
        { style: { marginLeft: "auto", fontSize: "11.5px" } },
        r
          ? `Nothing moved more than ${m.settings.tolerancePx} px${
              r.differenceDe00 === null ? "" : ` · measured ${de00(r.differenceDe00)} dE00`
            }`
          : "",
      ),
    );
  };
  const rail = h("aside.minifyrail");
  const el = h("div", { style: { flex: "1", minHeight: "0", display: "flex" } }, stage, rail);

  let pending = 0;

  const run = async () => {
    const st = store.state.minify;
    if (!st.before) return;
    const token = ++pending;
    try {
      const result = await api.minify(st.before, st.settings);
      if (token !== pending) return;
      store.state.minify.result = result;
      store.state.minify.error = null;
    } catch (e) {
      if (token !== pending) return;
      store.state.minify.result = null;
      store.state.minify.error = String(e);
    }
    store.touch("minify");
  };

  const openSvg = async () => {
    const picked = await open({
      multiple: false,
      filters: [{ name: "SVG", extensions: ["svg"] }],
    });
    if (typeof picked !== "string") return;
    try {
      const text = await api.readTextFile(picked);
      store.state.minify.name = picked.split(/[\\/]/).pop() ?? picked;
      store.state.minify.before = text;
      store.state.minify.result = null;
      store.state.minify.error = null;
      store.touch("minify");
      await run();
    } catch (e) {
      toast(String(e), { kind: "bad" });
    }
  };

  /** Accept an SVG dropped anywhere in the window while this tab is showing. */
  const acceptDropped = async (path: string) => {
    if (!path.toLowerCase().endsWith(".svg")) return false;
    const text = await api.readTextFile(path);
    store.state.minify.name = path.split(/[\\/]/).pop() ?? path;
    store.state.minify.before = text;
    store.touch("minify");
    await run();
    return true;
  };
  (el as HTMLElement & { acceptDropped?: typeof acceptDropped }).acceptDropped = acceptDropped;

  /** Before anything is open the stage itself is the invitation, as it is on Vectorize. */
  const emptyDrop = () =>
    h(
      "div.drop.solid",
      null,
      h("span.glyph", null, icon("file", 32)),
      h(
        "div",
        { style: { display: "flex", flexDirection: "column", gap: "6px" } },
        h("span.headline", null, "Drop an SVG here"),
        h("span.faint", { style: { fontSize: "13px" } }, "or choose one to rewrite it in the fewest segments"),
      ),
      h("button.btn.primary", { onclick: openSvg }, "Open an SVG"),
    );

  const render = () => {
    const m = store.state.minify;
    const r = m.result;
    renderTools();
    layout();

    fill(
      before,
      h("span.eyebrow.panelabel", null, `Before${r ? ` · ${bytes(r.bytesBefore)}` : ""}`),
      m.before ? art(m.before) : emptyDrop(),
    );
    fill(
      after,
      h("span.eyebrow.panelabel", null, `After${r ? ` · ${bytes(r.bytesAfter)}` : ""}`),
      r ? art(r.svg) : m.before ? null : h("span.faint.panehint", null, "The rewritten drawing appears here, on the same ground, so nothing can hide."),
    );

    if (m.before !== measuredFor) {
      const file = m.before;
      measuredFor = file;
      autoTone = null;
      applyBackdrop();
      if (file) {
        void toneOf(file).then((tone) => {
          if (measuredFor !== file) return;
          autoTone = tone;
          applyBackdrop();
        });
      }
    }

    if (!m.before) {
      fill(
        rail,
        h(
          "div.railscroll",
          { style: { justifyContent: "center", alignItems: "center", textAlign: "center", gap: "16px" } },
          h("span.serif", { style: { fontSize: "20px" } }, "Minify an SVG"),
          h(
            "span.faint",
            { style: { fontSize: "12.5px", lineHeight: "1.6", maxWidth: "30ch" } },
            "From Illustrator, Figma, a stock site — anything. It gets rewritten with the fewest segments that draw the same picture, and the result appears as you open it.",
          ),
        ),
      );
      return;
    }

    const saved = r ? 1 - r.bytesAfter / Math.max(1, r.bytesBefore) : 0;

    fill(
      rail,
      h(
        "div.railscroll",
        { style: { gap: "20px", padding: "22px 20px" } },
        m.error
          ? h(
              "div.card",
              null,
              h("span.eyebrow", null, "This file would not parse"),
              h("span.faint", { style: { fontSize: "12.5px", lineHeight: "1.6" } }, m.error),
            )
          : h(
              "div.minifyresult",
              null,
              h("span.eyebrow", null, r ? `Result · ${seconds((r.ms ?? 0) / 1000)}` : "Working"),
              h(
                "div",
                { style: { display: "flex", alignItems: "baseline", gap: "10px" } },
                h("span.figure", null, r ? percent(saved) : "—"),
                h("span.dim", { style: { fontSize: "14px", paddingBottom: "6px" } }, "smaller"),
              ),
              r
                ? h("span.faint.num", { style: { fontSize: "13px" } }, `${bytes(r.bytesBefore)} → ${bytes(r.bytesAfter)}`)
                : null,
            ),
        r
          ? h(
              "div.stats",
              null,
              ...(
                [
                  ["numbers before", count(r.numbersBefore)],
                  ["numbers after", count(r.numbersAfter)],
                  ["paths", `${r.pathsBefore} → ${r.pathsAfter}`],
                  ["segments", `${count(r.segmentsBefore)} → ${count(r.segmentsAfter)}`],
                  // The promise, measured the same way the tracer measures its own.
                  ["difference", r.differenceDe00 === null ? "—" : `${de00(r.differenceDe00)} dE00`],
                  ["kept as drawn", `${r.guarded} run${r.guarded === 1 ? "" : "s"}`],
                ] as [string, string][]
              ).map(([k, v]) => h("div", null, h("span.k", null, k), h("span.v", null, v))),
            )
          : null,
        toleranceControl(store, run),
        h(
          "div.card",
          null,
          h(
            "div",
            { style: { display: "flex", alignItems: "center", justifyContent: "space-between" } },
            h("span.dim", { style: { fontSize: "12.5px" } }, "Corner threshold"),
            (() => {
              const readout = h("span.num.faint", { style: { fontSize: "12px", width: "34px" } }, `${m.settings.cornerDegrees}°`);
              return h(
                "div",
                { style: { display: "flex", alignItems: "center", gap: "8px" } },
                h("input.slider", {
                  type: "range",
                  min: "5",
                  max: "90",
                  step: "1",
                  style: { width: "120px" },
                  value: String(m.settings.cornerDegrees),
                  "aria-label": "Corner threshold in degrees",
                  // Drag moves the readout; the minifier runs once, on release.
                  oninput: (e: Event) => {
                    readout.textContent = `${(e.target as HTMLInputElement).value}°`;
                  },
                  onchange: (e: Event) => {
                    m.settings.cornerDegrees = Number((e.target as HTMLInputElement).value);
                    store.touch("minify");
                    void run();
                  },
                }),
                readout,
              );
            })(),
          ),
          h(
            "div",
            { style: { display: "flex", alignItems: "center", justifyContent: "space-between" } },
            h("span.dim", { style: { fontSize: "12.5px" } }, "Document cleanup"),
            h("button.switch", {
              role: "switch",
              "aria-checked": String(m.settings.documentCleanup),
              "aria-label": "Document cleanup",
              onclick: () => {
                m.settings.documentCleanup = !m.settings.documentCleanup;
                store.touch("minify");
                void run();
              },
            }),
          ),
          h(
            "span.muted",
            { style: { fontSize: "11.5px", lineHeight: "1.5" } },
            "Removes ids, groups, editor metadata and trailing zeros. Same geometry.",
          ),
        ),
        r && r.removed.length
          ? h(
              "div",
              { style: { display: "flex", flexDirection: "column", gap: "7px" } },
              h("span.eyebrow", null, "Removed"),
              ...r.removed.map((x) =>
                h(
                  "div",
                  { style: { display: "flex", alignItems: "baseline", gap: "10px", fontSize: "12px" } },
                  h("span.faint", { style: { flex: "1" } }, x.what),
                  h("span.dim.num", null, x.amount),
                ),
              ),
            )
          : null,
      ),
      h(
        "div.railfoot",
        null,
        m.before
          ? h(
              "button.btn.ghost",
              {
                style: { justifyContent: "flex-start", gap: "8px" },
                title:
                  "Minifying keeps every path the file has. Re-tracing draws the file again from its render: stacked or translucent paths, broken outlines and thousands of nodes come back as a few clean shapes, and the report measures the result against the original.",
                onclick: () =>
                  window.dispatchEvent(new CustomEvent("inkvec:retrace", { detail: { svg: m.before, name: m.name ?? "drawing.svg" } })),
              },
              icon("wand", 14),
              "Rebuild it clean by re-tracing",
            )
          : null,
        h(
          "div.row",
          null,
          h(
            "button.btn.primary",
            {
              style: { flex: "1" },
              disabled: !r,
              onclick: async () => {
                if (!r) return;
                const stem = (m.name ?? "drawing").replace(/\.svg$/i, "");
                const path = await save({
                  defaultPath: `${stem}.min.svg`,
                  filters: [{ name: "SVG", extensions: ["svg"] }],
                });
                if (!path) return;
                await api.saveBytes(path, [...new TextEncoder().encode(r.svg)]);
                toast(`Saved to ${path}`, {
                  kind: "good",
                  action: { label: "Show in folder", run: () => void revealItemInDir(path) },
                });
              },
            },
            "Save SVG",
          ),
          h(
            "button.btn",
            {
              disabled: !r,
              onclick: async () => {
                if (!r) return;
                await writeText(r.svg);
                toast("SVG copied. Paste straight into Figma or Illustrator.");
              },
            },
            "Copy",
          ),
          h("button.btn", { onclick: openSvg }, "Open…"),
        ),
      ),
    );
  };

  store.on(["minify"], render);
  render();
  return el;
}

/**
 * The tolerance control: a sentence with two numbers in it.
 *
 * "Nothing moves more than 0.1 px when the drawing is 1024 px wide" is a complete
 * statement of what the minifier guarantees, and it is a better control than a slider
 * labelled "tolerance" could ever be.
 */
function toleranceControl(store: Store, run: () => void): HTMLElement {
  const m = store.state.minify;
  const field = (value: number, accent: boolean, apply: (n: number) => void) =>
    h("input", {
      class: `inlinefield${accent ? " accent" : ""}`,
      type: "text",
      inputmode: "decimal",
      value: String(value),
      onchange: (e: Event) => {
        const n = Number((e.target as HTMLInputElement).value);
        if (Number.isFinite(n) && n > 0) {
          apply(n);
          store.touch("minify");
          run();
        }
      },
    }) as HTMLInputElement;

  const tolerance = field(m.settings.tolerancePx, true, (n) => (m.settings.tolerancePx = n));
  // 0–1 px on a square curve: the useful range is the first tenth of it.
  const toleranceAt = (el: HTMLInputElement) => {
    const p = Number(el.value) / 1000;
    return Number((p * p).toFixed(3));
  };

  return h(
    "div.card",
    null,
    h(
      "span.sentence",
      null,
      "Nothing moves more than ",
      tolerance,
      " px when the drawing is ",
      field(m.settings.judgePx, false, (n) => (m.settings.judgePx = n)),
      " px wide.",
    ),
    h("input.slider", {
      type: "range",
      min: "0",
      max: "1000",
      step: "1",
      value: String(Math.round(Math.sqrt(Math.min(1, m.settings.tolerancePx)) * 1000)),
      "aria-label": "Tolerance",
      // Drag rewrites the sentence; the minifier runs once, on release.
      oninput: (e: Event) => {
        tolerance.value = String(toleranceAt(e.target as HTMLInputElement));
      },
      onchange: (e: Event) => {
        m.settings.tolerancePx = toleranceAt(e.target as HTMLInputElement);
        store.touch("minify");
        run();
      },
    }),
    h(
      "div.stops",
      null,
      h("span", null, "exact"),
      h("span", null, "gentle"),
      h("span", null, "aggressive"),
    ),
  );
}

/**
 * One pane's drawing, parsed rather than injected.
 *
 * `.centred` because this tab has no pan and zoom: the Vectorize viewer pins its artwork
 * to the pane's origin so the overlay can share one transform with it, and without the
 * modifier these two drawings would inherit that and sit in the corner.
 */
export function art(svg: string): HTMLElement {
  const wrap = h("div.art.centred");
  const el = drawable(svg);
  if (!el) return wrap;
  // Sized by the pane, on both axes, rather than by a fixed number of pixels: the SVG
  // fills the wrapper's box and letterboxes itself by its own `viewBox`, so a wide logo
  // and a tall one both fit whatever shape the window is.
  el.setAttribute("width", "100%");
  el.setAttribute("height", "100%");
  wrap.append(el);
  return wrap;
}

function viewBoxOf(el: SVGSVGElement): [number, number, number, number] | null {
  const v = (el.getAttribute("viewBox") ?? "").trim().split(/[\s,]+/).map(Number);
  return v.length === 4 && v.every(Number.isFinite) && v[2] > 0 && v[3] > 0
    ? [v[0], v[1], v[2], v[3]]
    : null;
}

/**
 * The document as an element that will show up, or null if it is not an SVG at all.
 *
 * Most SVGs say where their coordinates live with a `viewBox`, and then any size can be put
 * on them. Some — a hand-written logo, an older export — give only `width` and `height`,
 * and forcing a size onto one of those without a `viewBox` crops it to the top-left
 * corner of its own canvas: the pane comes up blank. So the missing `viewBox` is
 * supplied, from the stated size if there is one and from the drawing's measured extent
 * if there is not.
 */
function drawable(svg: string): SVGSVGElement | null {
  const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = doc.documentElement;
  if (!root || root.nodeName !== "svg" || doc.querySelector("parsererror")) return null;
  const el = document.importNode(root, true) as unknown as SVGSVGElement;
  if (viewBoxOf(el)) return el;

  const width = el.getAttribute("width") ?? "";
  const height = el.getAttribute("height") ?? "";
  const w = parseFloat(width);
  const hh = parseFloat(height);
  if (w > 0 && hh > 0 && !width.includes("%") && !height.includes("%")) {
    el.setAttribute("viewBox", `0 0 ${w} ${hh}`);
    return el;
  }

  // Nothing stated: measure it. That needs the element to be in the document.
  const probe = document.createElement("div");
  probe.style.cssText = "position:absolute;visibility:hidden;width:0;height:0;overflow:hidden";
  el.setAttribute("width", "1");
  el.setAttribute("height", "1");
  probe.append(el);
  document.body.append(probe);
  try {
    const b = el.getBBox();
    if (b.width > 0 && b.height > 0) {
      const pad = Math.max(b.width, b.height) * 0.02;
      el.setAttribute("viewBox", `${b.x - pad} ${b.y - pad} ${b.width + 2 * pad} ${b.height + 2 * pad}`);
    } else {
      el.setAttribute("viewBox", "0 0 100 100");
    }
  } catch {
    el.setAttribute("viewBox", "0 0 100 100");
  } finally {
    probe.remove();
  }
  return el;
}

/**
 * Which backdrop the drawing can be seen on: `"light"` for dark artwork, `"dark"` for
 * light artwork, and `null` when the default is fine (or the picture is empty).
 *
 * Read from the rendered pixels rather than from the markup, because colour in an SVG
 * comes from attributes, style sheets, classes and inheritance, and the picture is the
 * one place all of that has already been resolved.
 */
export async function toneOf(svg: string): Promise<"light" | "dark" | null> {
  const el = drawable(svg);
  if (!el) return null;
  const size = 64;
  el.setAttribute("width", String(size));
  el.setAttribute("height", String(size));
  const url = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(new XMLSerializer().serializeToString(el))}`;
  try {
    const img = await new Promise<HTMLImageElement>((resolve, reject) => {
      const i = new Image();
      i.onload = () => resolve(i);
      i.onerror = reject;
      i.src = url;
    });
    const canvas = document.createElement("canvas");
    canvas.width = size;
    canvas.height = size;
    const g = canvas.getContext("2d", { willReadFrequently: true });
    if (!g) return null;
    g.drawImage(img, 0, 0, size, size);
    const px = g.getImageData(0, 0, size, size).data;
    let weight = 0;
    let light = 0;
    for (let i = 0; i < px.length; i += 4) {
      const a = px[i + 3] / 255;
      if (a < 0.05) continue;
      weight += a;
      light += (a * (0.2126 * px[i] + 0.7152 * px[i + 1] + 0.0722 * px[i + 2])) / 255;
    }
    if (weight < 20) return null;
    const mean = light / weight;
    return mean < 0.3 ? "light" : mean > 0.8 ? "dark" : null;
  } catch {
    return null;
  }
}
