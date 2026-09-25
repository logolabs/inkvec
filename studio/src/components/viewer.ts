/**
 * The split comparison viewer.
 *
 * This is the product's argument, made visually, and it gets the most attention. Both
 * panes share one transform; the overlays live in one SVG that shares it too, so a
 * 3,000-node logo costs a single `transform` update per frame rather than 3,000 style
 * writes. The anchors are the one overlay painted on a canvas, which follows the view by
 * repainting. Nothing here re-renders on store changes except when the drawing itself
 * changes — the pan/zoom path never touches the DOM structure.
 */

import { fill, h, s } from "../lib/dom";
import { api } from "../lib/ipc";
import { anchorStyle, nodesOf, shapesOf, viewBoxOf } from "../lib/path";
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
  // The anchors are the exception: they are painted on a canvas the size of the pane,
  // not drawn in the overlay. Tens of thousands of round marks re-stroked on every zoom
  // tick cost a hundred milliseconds of paint and more; filled on a canvas they cost a
  // few. The canvas is not zoomed, so it lives beside the artwork rather than inside it,
  // and it is repainted at the new pan and zoom in the frame that shows them.
  const anchorsCv = h("canvas.anchors") as HTMLCanvasElement;
  const vectorPane = h(
    "div.pane.vector",
    null,
    h("span.eyebrow.panelabel", null, "Vector · SVG"),
    vectorArt,
    anchorsCv,
  );
  const divider = h("div.wipehandle", { role: "separator", "aria-label": "Wipe" });
  const panes = h("div.panes", null, sourcePane, vectorPane, divider);
  const el = panes;

  /** The drawing's own coordinate space, from its viewBox. */
  let box = { x: 0, y: 0, w: 1, h: 1 };
  /** The traced document currently on screen, sized by `transform()`. */
  let drawing: SVGSVGElement | null = null;
  /** What is on screen now, so a result that repeats the drawing costs nothing. */
  let shownSvg: string | null | undefined;
  let shownSource: State["source"] | undefined;
  /** The result whose bands the certainty layer holds, or is fetching. */
  let shownBands: State["result"] | undefined;
  /** Bumped whenever a bands request in flight stops being wanted. */
  let bandsRequest = 0;
  /** The zoom the documents were last sized at. */
  let sizedZoom = Number.NaN;

  // The layers, bottom to top, made once and emptied when the drawing changes. Each is
  // filled the first time it is shown: a 40,000-node logo whose anchors nobody looks at
  // should not pay for them on every trace.
  //
  // Colours and stroke widths come from the stylesheet, not from presentation
  // attributes: `var()` is a CSS value function and is not part of the attribute
  // grammar, so `fill="var(--anchor)"` is silently dropped by the SVG parser. Only
  // geometry is set here.
  const layers = {
    certainty: s("g.certainty") as SVGGElement,
    wireframe: s("g.wireframe") as SVGGElement,
    handles: s("g.handles") as SVGGElement,
  };
  type Layer = keyof typeof layers;
  const built: Record<Layer, boolean> = { certainty: false, wireframe: false, handles: false };
  for (const g of Object.values(layers)) g.style.display = "none";
  anchorsCv.style.display = "none";
  overlay.append(layers.certainty, layers.wireframe, layers.handles);

  /** The drawing's shapes and nodes, read once per drawing and only when a layer needs them. */
  let shapes: string[] | null = null;
  let nodes: { lines: string; knobs: string; dots: Dot[] } | null = null;

  // ------------------------------------------------------------- drawing ---

  /**
   * Put the current source and drawing on screen.
   *
   * Only what changed is rebuilt: a new result that repeats the drawing, or a stage change
   * around it, returns at once, and a new drawing leaves the source image alone. The
   * overlays are emptied here and built again only when they are shown.
   */
  function redraw(): void {
    const st = store.state;
    const svg = st.svg;
    const sourceChanged = st.source !== shownSource;
    const svgChanged = svg !== shownSvg;
    if (sourceChanged || svgChanged) {
      box = (svg ? viewBoxOf(svg) : null) ?? {
        x: 0,
        y: 0,
        w: st.source?.width ?? 1,
        h: st.source?.height ?? 1,
      };
      sizedZoom = Number.NaN;
    }

    if (sourceChanged) {
      shownSource = st.source;
      fill(
        sourceArt,
        st.source ? h("img", { src: st.source.preview, alt: "", draggable: "false" }) : null,
      );
    }

    if (svgChanged) {
      shownSvg = svg;
      // Parsed rather than injected as markup: the document comes from our own tracer,
      // but parsing it means a stray script or external reference could not execute even
      // if one ever appeared. The overlays read their nodes from this document too, so
      // they come from the drawing on screen rather than from a second reading of the text.
      drawing = null;
      if (svg) {
        const root = new DOMParser().parseFromString(svg, "image/svg+xml").documentElement;
        if (root && root.nodeName === "svg" && !root.querySelector("parsererror")) {
          root.setAttribute("width", String(box.w));
          root.setAttribute("height", String(box.h));
          drawing = document.importNode(root, true) as unknown as SVGSVGElement;
        }
      }
      fill(vectorArt, drawing, overlay);
      overlay.setAttribute("viewBox", `${box.x} ${box.y} ${box.w} ${box.h}`);
      shapes = null;
      nodes = null;
      for (const layer of ["wireframe", "handles"] as const) {
        fill(layers[layer]);
        built[layer] = false;
      }
      drawAnchors(true);
    }

    // The bands belong to a result rather than to a drawing: snapping an ink repaints the
    // drawing and keeps them, and a new trace of the same drawing replaces them. An outcome
    // with no drawing (a flat image) keeps the last result but has nothing to band.
    if (st.result !== shownBands || (svgChanged && !svg)) dropBands();

    if (sourceChanged || svgChanged) {
      applyShow();
      transform();
    } else if (st.show.certainty) {
      applyShow();
    }
  }

  function dropBands(): void {
    shownBands = undefined;
    bandsRequest++;
    fill(layers.certainty);
    built.certainty = false;
  }

  function shapesOfDrawing(): string[] {
    shapes ??= drawing ? shapesOf(drawing) : [];
    return shapes;
  }

  /**
   * Every node of the drawing, and its handle lines and knobs as one path each.
   *
   * A path of thousands of subpaths is one element to style, lay out and restyle on zoom,
   * where an element per handle was forty thousand. The knobs are drawn by the stroke — a
   * vanishing segment with square caps on a diagonal is a diamond — so with
   * `vector-effect: non-scaling-stroke` they keep their size on screen at any zoom without
   * a single attribute write. The anchors are points, painted by `paintAnchors`.
   */
  function nodeGeometry(): { lines: string; knobs: string; dots: Dot[] } {
    if (nodes) return nodes;
    const dots: Dot[] = [];
    const lines: string[] = [];
    const knobs: string[] = [];
    for (const d of shapesOfDrawing()) {
      // One dot per place, not one per node. A closed shape ends where it began — a
      // circle is four arcs back to its start — and two dots stacked on one point read
      // as a heavier dot, which is a lie about where the nodes are. Only the dot is
      // shared: the nodes that meet there each keep their own handles.
      // Keyed to a thousandth of a pixel as a number: a string key per node was a third
      // of the cost of this whole walk.
      const seen = new Set<number>();
      for (const n of nodesOf(d)) {
        const key = Math.round(n.x * 1000) * 1e8 + Math.round(n.y * 1000);
        if (!seen.has(key)) {
          seen.add(key);
          dots.push({ x: n.x, y: n.y });
        }
        for (const c of [n.in, n.out]) {
          if (!c) continue;
          lines.push(`M${n.x} ${n.y}L${c.x} ${c.y}`);
          // A diamond, not a dot: red/green confusion is common among exactly the
          // engineers who turn this overlay on, so shape carries the meaning too. The
          // cap of a square-capped stroke is square to the segment, so a vanishing
          // segment at 45 degrees draws the square on its point.
          knobs.push(`M${c.x} ${c.y}l.001 .001`);
        }
      }
    }
    nodes = { lines: lines.join(""), knobs: knobs.join(""), dots };
    return nodes;
  }

  function build(layer: Exclude<Layer, "certainty">): void {
    built[layer] = true;
    if (!drawing) return;
    const g = layers[layer];
    if (layer === "wireframe") {
      const d = joinPaths(shapesOfDrawing());
      if (d) g.append(s("path", { d }));
    } else {
      const { lines, knobs } = nodeGeometry();
      if (lines) g.append(s("path.lines", { d: lines }), s("path.knobs", { d: knobs }));
    }
  }

  // ------------------------------------------------------------- anchors ---

  /** What the canvas holds, so a frame that changes nothing it shows repaints nothing. */
  let paintedKey = "";
  let anchorsQueued = false;

  /**
   * Repaint the anchors in the next frame, once however many changes ask for it. `force`
   * marks what is painted stale (a new drawing, a theme); otherwise a frame whose pan,
   * zoom and pane size match the last paint is skipped.
   */
  function drawAnchors(force = false): void {
    if (force) paintedKey = "";
    if (anchorsQueued) return;
    anchorsQueued = true;
    requestAnimationFrame(() => {
      anchorsQueued = false;
      paintAnchors();
    });
  }

  /**
   * One dot per anchor, in screen pixels, over the part of the drawing the pane shows.
   *
   * The same marks the overlay drew: a disc of the anchor colour on a ring of the stage
   * colour 30% of the radius wide, sized and faded by `anchorStyle`. The fade is the
   * canvas's own opacity, as it was the group's, so a dot over its ring is not faded twice.
   * Two fills of one path each, rings first, whatever the count.
   */
  function paintAnchors(): void {
    const st = store.state;
    const on = Boolean(drawing) && st.show.anchors;
    // A hidden pane (the other side of A/B) has no size; paint it when it is shown.
    const pw = Math.round(vectorPane.clientWidth);
    const ph = Math.round(vectorPane.clientHeight);
    if (on && (!pw || !ph)) return;
    const dpr = window.devicePixelRatio || 1;
    const key = on ? `${st.zoom} ${st.pan.x} ${st.pan.y} ${pw} ${ph} ${dpr}` : "off";
    if (key === paintedKey) return;
    paintedKey = key;
    const ctx = anchorsCv.getContext("2d");
    if (!ctx) return;
    if (!on) {
      // Hidden or gone: release the backing store rather than clear it.
      anchorsCv.width = 0;
      anchorsCv.height = 0;
      return;
    }
    const bw = Math.max(1, Math.round(pw * dpr));
    const bh = Math.max(1, Math.round(ph * dpr));
    if (anchorsCv.width !== bw || anchorsCv.height !== bh) {
      anchorsCv.width = bw;
      anchorsCv.height = bh;
    }
    anchorsCv.style.width = `${pw}px`;
    anchorsCv.style.height = `${ph}px`;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, pw, ph);
    const { dots } = nodeGeometry();
    if (!dots.length) return;
    const style = getComputedStyle(vectorPane);
    const stage = style.getPropertyValue("--stage").trim() || "#808080";
    const ink = style.getPropertyValue("--anchor").trim() || "#ff5c00";
    const { r, opacity } = anchorStyle(st.zoom);
    anchorsCv.style.opacity = String(opacity);
    // Screen position of a drawing point: the art is pinned to the pane's origin, moved
    // by the pan and sized by the zoom.
    const ox = st.pan.x - box.x * st.zoom;
    const oy = st.pan.y - box.y * st.zoom;
    const outer = r * 1.15;
    const inner = r * 0.85;
    // Only the dots on screen, so a close zoom costs what it shows.
    const xs: number[] = [];
    const ys: number[] = [];
    for (const p of dots) {
      const x = ox + p.x * st.zoom;
      const y = oy + p.y * st.zoom;
      if (x < -outer || y < -outer || x > pw + outer || y > ph + outer) continue;
      xs.push(x);
      ys.push(y);
    }
    for (const [rad, colour] of [
      [outer, stage],
      [inner, ink],
    ] as const) {
      ctx.beginPath();
      for (let i = 0; i < xs.length; i++) {
        ctx.moveTo(xs[i] + rad, ys[i]);
        ctx.arc(xs[i], ys[i], rad, 0, 2 * Math.PI);
      }
      ctx.fillStyle = colour;
      ctx.fill();
    }
  }

  /**
   * The engine's confidence bands, fetched when Certainty is first shown for a result.
   *
   * A result carries them inline (an older backend) or leaves them with the backend,
   * which serves them by generation. Either way they are read with a scan of the text, not
   * a parse: fifteen thousand bands are a few hundred paths (see `layerBands`) rather than
   * fifteen thousand elements.
   */
  function buildBands(): void {
    const st = store.state;
    const result = st.result;
    built.certainty = true;
    shownBands = result;
    if (!result || !st.svg) return;
    if (result.bands) {
      fillBands(result.bands);
      return;
    }
    const ticket = ++bandsRequest;
    // No bands for this trace: the toggle is disabled until the next result, and it is
    // released too, or it would sit pressed over a layer that is not there.
    const missing = () => {
      if (ticket !== bandsRequest) return;
      store.set({ bandsMissing: true, show: { ...store.state.show, certainty: false } });
    };
    api.traceBands(st.resultGeneration).then((text) => {
      if (ticket !== bandsRequest) return;
      if (text) fillBands(text);
      else missing();
    }, missing);
  }

  function fillBands(text: string): void {
    fill(layers.certainty);
    for (const { cls, d } of layerBands(readBands(text))) layers.certainty.append(s(`path.${cls}`, { d }));
  }

  /** Show/hide the overlays and the fill, building a layer the first time it shows. */
  function applyShow(): void {
    const { show } = store.state;
    const set = (layer: Layer, on: boolean) => {
      if (on && !built[layer]) {
        if (layer === "certainty") buildBands();
        else build(layer);
      }
      layers[layer].style.display = on ? "" : "none";
    };
    set("wireframe", show.wireframe);
    set("handles", show.handles);
    set("certainty", show.certainty);
    anchorsCv.style.display = show.anchors ? "" : "none";
    drawAnchors();
    // Turning the fill off has to leave something behind, or "wireframe only" is a
    // blank pane; it dims rather than disappears.
    if (drawing) drawing.style.opacity = show.fill ? "1" : "0.12";
  }

  // ----------------------------------------------------------- transform ---

  function transform(): void {
    const st = store.state;

    // The zoom is *not* in the transform.
    //
    // `transform: scale()` on a promoted layer is composited: WebKit rasterises the
    // layer once and scales that bitmap, so at 12x the traced SVG came out as a twenty
    // pixel gradient where it should be one hard edge — the drawing was a real SVG and
    // looked exactly like a blown-up PNG, which is the one thing this viewer exists to
    // disprove. Sizing the document in CSS pixels makes the engine re-render the vector
    // at the resolution it is shown at, and the edge is an edge at any stop.
    //
    // The transform still carries the pan, which is a translate and composites
    // correctly, so dragging stays a single cheap write per frame.
    const t = `translate(${st.pan.x}px, ${st.pan.y}px)`;
    for (const node of [sourceArt, vectorArt]) {
      node.style.transform = t;
    }

    // A pan is the translate above and nothing else. Only a new zoom resizes the
    // documents, and the overlay marks are sized in screen pixels by the stylesheet. The
    // anchors' canvas is the pane's size and does not move with the art: it is repainted
    // for the new view in the coming frame (a pan that only translated it would leave the
    // dots it slid in from off-screen unpainted).
    if (st.zoom !== sizedZoom) {
      sizedZoom = st.zoom;
      const w = box.w * st.zoom;
      const h = box.h * st.zoom;
      const img = sourceArt.querySelector("img");
      if (img) {
        img.style.width = `${w}px`;
        img.style.height = `${h}px`;
      }
      if (drawing) {
        drawing.setAttribute("width", String(w));
        drawing.setAttribute("height", String(h));
      }
      overlay.setAttribute("width", String(w));
      overlay.setAttribute("height", String(h));
    }
    if (st.show.anchors) drawAnchors();

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
    // The source is a raster and its pixels are the evidence, so past the point where
    // they are individually visible they are drawn as squares rather than smoothed.
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
      fitted: true,
    });
  }

  function goTo(x: number, y: number, zoom: number): void {
    const { w, h: ph } = paneSize();
    store.set({ zoom, pan: { x: w / 2 - x * zoom, y: ph / 2 - y * zoom }, fitted: false });
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
      fitted: false,
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
        store.set({ pan: { x: st.pan.x + (m.clientX - lastX), y: st.pan.y + (m.clientY - lastY) }, fitted: false });
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

  // The viewer is the only thing that redraws itself; the stage around it never asks.
  store.on(["svg", "source", "result"], redraw);
  store.on(["zoom", "pan", "view", "wipe", "flicked", "detail"], transform);
  store.on(["show"], applyShow);

  // Fitting needs the pane's size, which is only known once it is laid out.
  const observer = new ResizeObserver(() => {
    if (!store.state.svg && !store.state.source) return;
    applyLayout();
    // The anchors' canvas is the pane's size, so a new pane size is a new canvas.
    if (store.state.show.anchors) drawAnchors();
    // A view nobody has moved follows the window; one somebody zoomed or panned is theirs.
    if (store.state.fitted) fit();
  });
  observer.observe(panes);

  // The canvas holds colours and a pixel density rather than references to them, so a
  // theme change or a move to a screen of another density repaints it.
  new MutationObserver(() => {
    if (store.state.show.anchors) drawAnchors(true);
  }).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  const watchDensity = () => {
    window
      .matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
      .addEventListener(
        "change",
        () => {
          if (store.state.show.anchors) drawAnchors();
          watchDensity();
        },
        { once: true },
      );
  };
  watchDensity();

  return { el, redraw, transform, fit, goTo, zoomTo };
}

/**
 * Several `d` attributes as one, for a layer drawn as a single path.
 *
 * A leading relative `m` is absolute only as the first command of a path; after another
 * subpath it would be relative to where that one ended. A `M0 0` in front keeps it where
 * it was, and a lone moveto is never stroked.
 */
export function joinPaths(ds: string[]): string {
  let out = "";
  for (const raw of ds) {
    const d = raw.trim();
    if (!d) continue;
    out += d[0] === "M" ? d : `M0 0${d}`;
  }
  return out;
}

/** How sure an edge is, by the mean sigma of its band: under 0.1 px, under 0.3 px, above. */
export type BandClass = "sure" | "soft" | "unsure";

/**
 * The bands of an `--uncertainty` document, in document order, each with its class.
 *
 * A clean edge's band is a tenth of a pixel wide, true and invisible; each band is
 * coloured by how sure the edge is and outlined a screen pixel wide, so the sure ones read
 * as green lines and the unsure ones as red swellings. Read with a scan of the text: the
 * document is the engine's own, a flat list of paths, and parsing it into a DOM only to
 * read two attributes back cost more than everything else a new result does.
 */
export function readBands(text: string): Band[] {
  const out: Band[] = [];
  const tag = /<path\b([^>]*)>/g;
  for (let m = tag.exec(text); m; m = tag.exec(text)) {
    const attrs = m[1];
    const raw = /(?:^|\s)d="([^"]*)"/.exec(attrs)?.[1]?.trim();
    if (!raw) continue;
    const box = bandBox(raw);
    if (!box) continue;
    const sigma = Number(/\sdata-sigma-mean="([^"]*)"/.exec(attrs)?.[1] ?? 0);
    out.push({
      // Bands are joined into shared paths, where a leading relative moveto would move.
      d: raw[0] === "M" ? raw : `M0 0${raw}`,
      cls: sigma <= 0.1 ? "sure" : sigma <= 0.3 ? "soft" : "unsure",
      x0: box.x0,
      y0: box.y0,
      x1: box.x1,
      y1: box.y1,
    });
  }
  return out;
}

/** An anchor, in the drawing's units. */
export interface Dot {
  x: number;
  y: number;
}

/** A band's bounding box, in the drawing's units. */
export interface Box {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

/** One band: its outline, its class and its box. */
export interface Band extends Box {
  d: string;
  cls: BandClass;
}

const TENTHS = [1, 1e-1, 1e-2, 1e-3, 1e-4, 1e-5, 1e-6, 1e-7, 1e-8, 1e-9, 1e-10, 1e-11, 1e-12, 1e-13, 1e-14, 1e-15];

/**
 * A band's bounding box, read straight off its outline.
 *
 * The outline is the engine's own (`uncertainty.rs`): absolute `M x y L x y ... Z`, so
 * every number is a coordinate and they alternate x, y. Read digit by digit rather than
 * with a regex and `Number()`: fifteen thousand bands are half a million numbers, and
 * converting each through a string was three times the cost of this loop.
 */
export function bandBox(d: string): Box | null {
  let x0 = Infinity;
  let y0 = Infinity;
  let x1 = -Infinity;
  let y1 = -Infinity;
  let isY = false;
  const n = d.length;
  let i = 0;
  while (i < n) {
    let c = d.charCodeAt(i);
    const neg = c === 45; // -
    const signed = neg || c === 43;
    if (signed) c = d.charCodeAt(++i);
    if (!((c >= 48 && c <= 57) || c === 46)) {
      if (!signed) i++;
      continue;
    }
    let v = 0;
    while (c >= 48 && c <= 57) {
      v = v * 10 + (c - 48);
      c = d.charCodeAt(++i);
    }
    if (c === 46) {
      let f = 0;
      let k = 0;
      c = d.charCodeAt(++i);
      while (c >= 48 && c <= 57) {
        if (k < 15) {
          f = f * 10 + (c - 48);
          k++;
        }
        c = d.charCodeAt(++i);
      }
      v += f * TENTHS[k];
    }
    if (c === 101 || c === 69) {
      c = d.charCodeAt(++i);
      const eneg = c === 45;
      if (eneg || c === 43) c = d.charCodeAt(++i);
      let e = 0;
      while (c >= 48 && c <= 57) {
        e = e * 10 + (c - 48);
        c = d.charCodeAt(++i);
      }
      v *= 10 ** (eneg ? -e : e);
    }
    if (neg) v = -v;
    if (isY) {
      if (v < y0) y0 = v;
      if (v > y1) y1 = v;
    } else {
      if (v < x0) x0 = v;
      if (v > x1) x1 = v;
    }
    isY = !isY;
  }
  return x0 <= x1 && y0 <= y1 ? { x0, y0, x1, y1 } : null;
}

/**
 * How far apart, in the drawing's units, two bands' boxes must be to be drawn without
 * regard to each other. Their fills only meet where the bands do; the margin covers
 * their screen-pixel outlines and the antialiasing at their edges.
 */
const BAND_GAP = 1;
/** The grid the overlap search buckets boxes into; about the size of a typical band. */
const BAND_CELL = 16;
/** The side of the square tiles each layer is cut into, in the drawing's units. */
const BAND_TILE = 256;

/**
 * The bands as paths that paint exactly as one element per band did.
 *
 * One element per band was fifteen thousand elements, and a zoom restyled every one. One
 * path per class fills where two bands overlap once rather than twice, puts every red band
 * over every orange one, and merges even-odd interiors: a fit view came out visibly
 * different. So the bands are dealt into layers first. A band joins an existing layer
 * only where nothing it overlaps could tell: the layer is of its class, holds no band that
 * overlaps it (their translucent fills stack, as two elements did), and comes after every
 * layer holding an earlier band of another class that overlaps it (that one is painted
 * under it, as in the document). Otherwise it starts a new layer at the top. "Overlaps" is
 * the boxes, a little padded, found through a coarse grid; conservative, never a miss.
 * Layers of one class commute, so only the order across classes is kept.
 *
 * Then each layer is cut into tiles by band centre, so a close zoom paints the paths it
 * shows and skips the rest: a layer that spans the drawing is painted whole at any zoom.
 * On a 15,000-band logo: about 340 layers, 2,200 paths.
 */
export function layerBands(bands: Band[], tile = BAND_TILE): { cls: BandClass; d: string }[] {
  const grid = new Map<number, number[]>();
  const layerOf = new Int32Array(bands.length);
  const layers: BandClass[] = [];
  // The layers of each class, bottom to top.
  const byClass: Record<BandClass, number[]> = { sure: [], soft: [], unsure: [] };
  // stamp[l] === i + 1 when layer l holds a band of band i's class that overlaps it.
  const stamp: number[] = [];
  const g = BAND_GAP / 2;
  for (let i = 0; i < bands.length; i++) {
    const b = bands[i];
    const x0 = b.x0 - g;
    const y0 = b.y0 - g;
    const x1 = b.x1 + g;
    const y1 = b.y1 + g;
    const cx0 = Math.floor(x0 / BAND_CELL);
    const cx1 = Math.floor(x1 / BAND_CELL);
    const cy0 = Math.floor(y0 / BAND_CELL);
    const cy1 = Math.floor(y1 / BAND_CELL);
    // The highest layer this band must be painted over.
    let floor = -1;
    for (let cy = cy0; cy <= cy1; cy++) {
      for (let cx = cx0; cx <= cx1; cx++) {
        const near = grid.get(cx * 65536 + cy);
        if (!near) continue;
        for (const j of near) {
          const o = bands[j];
          if (o.x0 - g < x1 && o.x1 + g > x0 && o.y0 - g < y1 && o.y1 + g > y0) {
            if (o.cls === b.cls) stamp[layerOf[j]] = i + 1;
            else if (layerOf[j] > floor) floor = layerOf[j];
          }
        }
      }
    }
    let layer = -1;
    for (const l of byClass[b.cls]) {
      if (l > floor && stamp[l] !== i + 1) {
        layer = l;
        break;
      }
    }
    if (layer < 0) {
      layer = layers.length;
      layers.push(b.cls);
      byClass[b.cls].push(layer);
    }
    layerOf[i] = layer;
    for (let cy = cy0; cy <= cy1; cy++) {
      for (let cx = cx0; cx <= cx1; cx++) {
        const key = cx * 65536 + cy;
        const near = grid.get(key);
        if (near) near.push(i);
        else grid.set(key, [i]);
      }
    }
  }
  // Each layer cut into tiles by band centre, so a close zoom skips what it does not show.
  // Tiles of one layer never overlap, so they are painted layer by layer, in any order.
  const paths = new Map<number, { cls: BandClass; d: string }>();
  for (let i = 0; i < bands.length; i++) {
    const b = bands[i];
    const tx = Math.floor((b.x0 + b.x1) / 2 / tile) + 2048;
    const ty = Math.floor((b.y0 + b.y1) / 2 / tile) + 2048;
    const key = (layerOf[i] * 4096 + ty) * 4096 + tx;
    const path = paths.get(key);
    if (path) path.d += b.d;
    else paths.set(key, { cls: layers[layerOf[i]], d: b.d });
  }
  return [...paths.entries()].sort((a, b) => a[0] - b[0]).map((e) => e[1]);
}

/** Whether a state has something for the viewer to show. */
export function hasDrawing(st: State): boolean {
  return Boolean(st.source);
}
