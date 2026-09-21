/**
 * The split comparison viewer.
 *
 * This is the product's argument, made visually, and it gets the most attention. Both
 * panes share one transform; the overlays live in one SVG that shares it too, so a
 * 3,000-node logo costs a single `transform` update per frame rather than 3,000 style
 * writes. Nothing here re-renders on store changes except when the drawing itself
 * changes — the pan/zoom path never touches the DOM structure.
 */

import { fill, h, s } from "../lib/dom";
import { anchorStyle, nodesOf, pathData, viewBoxOf } from "../lib/path";
import type { State, Store } from "../lib/state";

/** The zoom stops the toolbar offers, plus the range scroll can reach. */
export const ZOOM_MIN = 0.25;
export const ZOOM_MAX = 64;

export interface Viewer {
  el: HTMLElement;
  /** Rebuild the artwork. Call when the drawing or the source changes. */
  redraw(): void;
  /** Re-apply pan/zoom only. Cheap; safe to call every frame. */
  transform(): void;
  /** Fit the drawing to the pane. */
  fit(): void;
  /** Centre on a point in traced-raster pixels, at `zoom`. */
  goTo(x: number, y: number, zoom: number): void;
  /** Change the zoom while holding whatever is in the middle of the pane. */
  zoomTo(zoom: number): void;
}

export function createViewer(store: Store): Viewer {
  const sourceArt = h("div.art");
  const vectorArt = h("div.art");
  const overlay = s("svg.overlay") as SVGSVGElement;

  const sourcePane = h(
    "div.pane.source",
    null,
    h("span.eyebrow.panelabel", null, "Source"),
    sourceArt,
  );
  // The overlay lives *inside* the artwork's wrapper rather than beside it. Two
  // absolutely positioned siblings carrying the same transform drift apart the moment
  // anything about their containing blocks differs, and an anchors overlay that lands
  // beside the paths instead of on them is worse than no overlay at all. One wrapper,
  // one transform, and the overlay inherits it by construction.
  vectorArt.append(overlay);
  const vectorPane = h(
    "div.pane.vector",
    null,
    h("span.eyebrow.panelabel", null, "Vector · SVG"),
    vectorArt,
  );
  const divider = h("div.wipehandle", { role: "separator", "aria-label": "Wipe" });
  const panes = h("div.panes", null, sourcePane, vectorPane, divider);
  const el = panes;

  /** The drawing's own coordinate space, from its viewBox. */
  let box = { x: 0, y: 0, w: 1, h: 1 };

  // ------------------------------------------------------------- drawing ---

  function redraw(): void {
    const st = store.state;
    const svg = st.svg;
    box = (svg ? viewBoxOf(svg) : null) ?? {
      x: 0,
      y: 0,
      w: st.source?.width ?? 1,
      h: st.source?.height ?? 1,
    };

    fill(
      sourceArt,
      st.source ? h("img", { src: st.source.preview, alt: "", draggable: "false" }) : null,
    );
    if (st.source) {
      const img = sourceArt.firstElementChild as HTMLImageElement | null;
      if (img) {
        img.style.width = `${box.w}px`;
        img.style.height = `${box.h}px`;
      }
    }

    if (svg) {
      // Parsed rather than injected as markup: the document comes from our own
      // tracer, but parsing it means a stray script or external reference could not
      // execute even if one ever appeared.
      const doc = new DOMParser().parseFromString(svg, "image/svg+xml");
      const root = doc.documentElement;
      if (root && root.nodeName === "svg") {
        root.setAttribute("width", String(box.w));
        root.setAttribute("height", String(box.h));
        fill(vectorArt, document.importNode(root, true), overlay);
      } else {
        fill(vectorArt, overlay);
      }
    } else {
      fill(vectorArt, overlay);
    }

    buildOverlay();
    applyShow();
    transform();
  }

  /**
   * The wireframe, anchors and handles, as one SVG in the drawing's coordinates.
   *
   * Built once per trace. Their size and opacity are the only things that change with
   * zoom, and those are attributes on three groups rather than on every dot.
   */
  function buildOverlay(): void {
    const svg = store.state.svg;
    fill(overlay);
    if (!svg) return;

    overlay.setAttribute("viewBox", `${box.x} ${box.y} ${box.w} ${box.h}`);
    const ds = pathData(svg);

    // Colours and stroke widths come from the stylesheet, not from presentation
    // attributes: `var()` is a CSS value function and is not part of the attribute
    // grammar, so `fill="var(--anchor)"` is silently dropped by the SVG parser. Only
    // geometry is set here.
    const wire = s("g.wireframe", { "vector-effect": "non-scaling-stroke" });
    const handles = s("g.handles");
    const anchors = s("g.anchors");

    for (const d of ds) {
      wire.append(s("path", { d }));
      for (const n of nodesOf(d)) {
        anchors.append(s("circle", { cx: n.x, cy: n.y, r: 1 }));
        for (const c of [n.in, n.out]) {
          if (!c) continue;
          handles.append(s("line", { x1: n.x, y1: n.y, x2: c.x, y2: c.y }));
          // A diamond, not a dot: red/green confusion is common among exactly the
          // engineers who turn this overlay on, so shape carries the meaning too.
          handles.append(
            s("rect", {
              class: "knob",
              x: c.x - 1,
              y: c.y - 1,
              width: 2,
              height: 2,
              transform: `rotate(45 ${c.x} ${c.y})`,
            }),
          );
        }
      }
    }
    overlay.append(wire, handles, anchors);
  }

  /** Show/hide the three overlays and the fill, without rebuilding anything. */
  function applyShow(): void {
    const { show } = store.state;
    const set = (sel: string, on: boolean) => {
      const g = overlay.querySelector<SVGGElement>(sel);
      if (g) g.style.display = on ? "" : "none";
    };
    set(".wireframe", show.wireframe);
    set(".handles", show.handles);
    set(".anchors", show.anchors);
    // Turning the fill off has to leave something behind, or "wireframe only" is a
    // blank pane; it dims rather than disappears.
    const art = vectorArt.querySelector<SVGElement>("svg:not(.overlay)");
    if (art) art.style.opacity = show.fill ? "1" : "0.12";
  }

  // ----------------------------------------------------------- transform ---

  function transform(): void {
    const st = store.state;
    const t = `translate(${st.pan.x}px, ${st.pan.y}px) scale(${st.zoom})`;
    for (const node of [sourceArt, vectorArt]) {
      node.style.transform = t;
    }
    overlay.setAttribute("width", String(box.w));
    overlay.setAttribute("height", String(box.h));

    // Overlay geometry is in the drawing's units, so everything that should stay a
    // constant size on screen is divided by the zoom. The anchors thin out with zoom
    // rather than being capped, which is what keeps a 3,000-node logo readable at 1x
    // and precise at 12x; see `anchorStyle`.
    const { r, opacity } = anchorStyle(st.zoom);
    overlay.style.setProperty("--dot", String(r / st.zoom));
    overlay.style.setProperty("--dot-opacity", String(opacity));
    overlay.style.setProperty("--hair", `${0.8 / st.zoom}`);
    for (const c of overlay.querySelectorAll(".anchors > circle")) {
      c.setAttribute("r", String(r / st.zoom));
    }
    for (const k of overlay.querySelectorAll<SVGRectElement>(".handles > rect")) {
      const size = (2.4 / st.zoom).toFixed(3);
      const half = Number(size) / 2;
      const cx = Number(k.dataset.cx ?? k.getAttribute("x")) + 1;
      const cy = Number(k.dataset.cy ?? k.getAttribute("y")) + 1;
      k.dataset.cx ??= String(cx - 1);
      k.dataset.cy ??= String(cy - 1);
      k.setAttribute("x", String(cx - half));
      k.setAttribute("y", String(cy - half));
      k.setAttribute("width", size);
      k.setAttribute("height", size);
      k.setAttribute("transform", `rotate(45 ${cx} ${cy})`);
    }

    applyLayout();
  }

  /** Side-by-side, wipe or A/B — and the pixel grid, which is a mode of its own. */
  function applyLayout(): void {
    const st = store.state;
    panes.classList.toggle("stacked", st.view !== "side");
    divider.style.display = st.view === "wipe" ? "" : "none";

    if (st.view === "side") {
      sourcePane.classList.remove("hidden");
      vectorPane.classList.remove("hidden");
      sourcePane.style.clipPath = "";
      vectorPane.style.clipPath = "";
    } else if (st.view === "wipe") {
      sourcePane.classList.remove("hidden");
      vectorPane.classList.remove("hidden");
      const pct = `${(st.wipe * 100).toFixed(2)}%`;
      divider.style.left = pct;
      sourcePane.style.clipPath = `inset(0 ${(100 - st.wipe * 100).toFixed(2)}% 0 0)`;
      vectorPane.style.clipPath = `inset(0 0 0 ${pct})`;
    } else {
      // A/B: one pane at a time. Holding Space swaps and releasing swaps back — it is
      // a comparison, not a transition, so there is no easing on either edge.
      sourcePane.style.clipPath = "";
      vectorPane.style.clipPath = "";
      sourcePane.classList.toggle("hidden", !st.flicked);
      vectorPane.classList.toggle("hidden", st.flicked);
    }

    // The pixel grid, drawn only where it can mean something: one cell per source
    // pixel, which is only legible past about 6x.
    const showGrid = st.detail && st.zoom >= 6;
    let grid = vectorPane.querySelector<HTMLElement>(".pixelgrid");
    if (showGrid && !grid) {
      grid = h("div.pixelgrid");
      vectorPane.append(grid);
    }
    if (grid) {
      grid.style.display = showGrid ? "" : "none";
      grid.style.backgroundSize = `${st.zoom}px ${st.zoom}px`;
      grid.style.backgroundPosition = `${st.pan.x}px ${st.pan.y}px`;
    }
    const img = sourceArt.querySelector("img");
    if (img) img.style.imageRendering = st.zoom >= 4 ? "pixelated" : "auto";
  }

  // ------------------------------------------------------------ gestures ---

  function paneSize(): { w: number; h: number } {
    const r = vectorPane.getBoundingClientRect();
    return { w: r.width || 1, h: r.height || 1 };
  }

  function fit(): void {
    const { w, h: ph } = paneSize();
    const zoom = Math.min(w / box.w, ph / box.h) * 0.88;
    store.set({
      zoom,
      pan: { x: (w - box.w * zoom) / 2, y: (ph - box.h * zoom) / 2 },
    });
  }

  function goTo(x: number, y: number, zoom: number): void {
    const { w, h: ph } = paneSize();
    store.set({ zoom, pan: { x: w / 2 - x * zoom, y: ph / 2 - y * zoom } });
  }

  /**
   * Change the zoom while holding the middle of the pane.
   *
   * What the toolbar's 1x / 4x / 12x and the Detail toggle use. Setting the zoom without
   * moving the pan would scale about the drawing's origin, which throws the artwork off
   * screen at 12x — the stop would be correct and the view would be empty.
   */
  function zoomTo(next: number): void {
    const { w, h: ph } = paneSize();
    zoomAt(w / 2, ph / 2, Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, next)) / store.state.zoom);
  }

  /** Zoom about a point in pane coordinates, so the pixel under the cursor stays put. */
  function zoomAt(px: number, py: number, factor: number): void {
    const st = store.state;
    const next = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, st.zoom * factor));
    const k = next / st.zoom;
    store.set({
      zoom: next,
      pan: { x: px - (px - st.pan.x) * k, y: py - (py - st.pan.y) * k },
    });
  }

  for (const pane of [sourcePane, vectorPane]) {
    pane.addEventListener(
      "wheel",
      (e: WheelEvent) => {
        e.preventDefault();
        const r = pane.getBoundingClientRect();
        // A trackpad reports small deltas continuously and a wheel reports large ones
        // in steps; the exponent makes both feel like the same gesture.
        zoomAt(e.clientX - r.left, e.clientY - r.top, Math.exp(-e.deltaY * 0.0016));
      },
      { passive: false },
    );
    pane.addEventListener("dblclick", fit);
    pane.addEventListener("pointerdown", (e: PointerEvent) => {
      if (e.button !== 0) return;
      pane.setPointerCapture(e.pointerId);
      let lastX = e.clientX;
      let lastY = e.clientY;
      const move = (m: PointerEvent) => {
        const st = store.state;
        store.set({ pan: { x: st.pan.x + (m.clientX - lastX), y: st.pan.y + (m.clientY - lastY) } });
        lastX = m.clientX;
        lastY = m.clientY;
      };
      const up = () => {
        pane.removeEventListener("pointermove", move);
        pane.removeEventListener("pointerup", up);
        pane.removeEventListener("pointercancel", up);
      };
      pane.addEventListener("pointermove", move);
      pane.addEventListener("pointerup", up);
      pane.addEventListener("pointercancel", up);
    });
  }

  // The wipe divider follows the pointer 1:1, and releasing does not animate.
  divider.addEventListener("pointerdown", (e: PointerEvent) => {
    e.stopPropagation();
    divider.setPointerCapture(e.pointerId);
    const move = (m: PointerEvent) => {
      const r = panes.getBoundingClientRect();
      store.set({ wipe: Math.min(0.98, Math.max(0.02, (m.clientX - r.left) / r.width)) });
    };
    const up = () => {
      divider.removeEventListener("pointermove", move);
      divider.removeEventListener("pointerup", up);
    };
    divider.addEventListener("pointermove", move);
    divider.addEventListener("pointerup", up);
  });

  // ----------------------------------------------------------- wiring up ---

  store.on(["svg", "source"], redraw);
  store.on(["zoom", "pan", "view", "wipe", "flicked", "detail"], transform);
  store.on(["show"], () => {
    applyShow();
    transform();
  });

  // Fitting needs the pane's size, which is only known once it is laid out.
  const observer = new ResizeObserver(() => {
    if (!store.state.svg && !store.state.source) return;
    applyLayout();
  });
  observer.observe(panes);

  return { el, redraw, transform, fit, goTo, zoomTo };
}

/** Whether a state has something for the viewer to show. */
export function hasDrawing(st: State): boolean {
  return Boolean(st.source);
}
