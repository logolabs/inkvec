/**
 * A very small DOM builder, and the icon set.
 *
 * There is no framework here on purpose. The brief's rendering budget is "CSS, DOM and
 * inline SVG only", the comparison viewer has to survive a few thousand anchor dots, and
 * a virtual DOM diffing thousands of `<circle>` elements sixty times a second is exactly
 * the cost that budget exists to avoid. Panels are rebuilt whole because they are small;
 * the viewer is driven imperatively because it is not.
 */

// Generated from `web/logo.svg` by `studio/tools/make_assets.py`, with its ink set to
// `currentColor`. Imported rather than transcribed: the mark changed twice in one day
// while this app was being built, and a copied path is a copy that goes stale quietly.
import markSvg from "../assets/mark.svg?raw";

type Child = Node | string | number | null | undefined | false | Child[];

/** Attributes `h` understands beyond plain string properties. */
export interface Attrs {
  class?: string;
  style?: string | Partial<CSSStyleDeclaration>;
  /** Event handlers, e.g. `onclick`. */
  [key: string]: unknown;
}

/** Build an element. `h("button.primary", { onclick }, "Export")`. */
export function h<K extends keyof HTMLElementTagNameMap>(
  spec: K | string,
  attrs: Attrs | null = null,
  ...children: Child[]
): HTMLElement {
  const [tag, ...classes] = spec.split(".");
  const el = document.createElement(tag || "div");
  if (classes.length) el.className = classes.join(" ");
  apply(el, attrs);
  append(el, children);
  return el;
}

/** The same, in the SVG namespace. */
export function s(spec: string, attrs: Attrs | null = null, ...children: Child[]): SVGElement {
  const [tag, ...classes] = spec.split(".");
  const el = document.createElementNS("http://www.w3.org/2000/svg", tag || "g");
  if (classes.length) el.setAttribute("class", classes.join(" "));
  for (const [k, v] of Object.entries(attrs ?? {})) {
    if (v === null || v === undefined || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2), v as EventListener);
    } else {
      el.setAttribute(k, String(v));
    }
  }
  append(el, children);
  return el;
}

function apply(el: HTMLElement, attrs: Attrs | null) {
  for (const [k, v] of Object.entries(attrs ?? {})) {
    if (v === null || v === undefined || v === false) continue;
    if (k === "class") {
      el.className = el.className ? `${el.className} ${v}` : String(v);
    } else if (k === "style" && typeof v === "object") {
      Object.assign(el.style, v);
    } else if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2), v as EventListener);
    } else if (k === "value" && el instanceof HTMLInputElement) {
      el.value = String(v);
    } else if (v === true) {
      el.setAttribute(k, "");
    } else {
      el.setAttribute(k, String(v));
    }
  }
}

function append(el: Element, children: Child[]) {
  for (const child of children.flat(8) as Child[]) {
    if (child === null || child === undefined || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

/** Empty a node and put `children` in it. */
export function fill(el: Element, ...children: Child[]): void {
  el.replaceChildren();
  append(el, children);
}

/**
 * The icon set: Lucide's geometry, inlined.
 *
 * Lucide is ISC-licensed and is literally part of the engine's own regression corpus,
 * which is the reason the brief suggests it. Only the glyphs the app actually uses are
 * here — a whole icon package for eighteen shapes is a dependency that earns nothing.
 */
const PATHS: Record<string, string> = {
  image: "M3 3h18v18H3zM3 15l5-5 4 4 3-3 6 6M8.5 8.5h.01",
  type: "M4 7V4h16v3M9 20h6M12 4v16",
  droplet: "M12 2.7 6.7 8a7.5 7.5 0 1 0 10.6 0Z",
  wand: "m3 21 9-9M15 4V2M15 16v-2M8 9h2M20 9h2M17.8 11.8 19 13M15 9h.01M17.8 6.2 19 5M12.8 6.2 11.6 5",
  layers: "m12 2 9 5-9 5-9-5 9-5M3 12l9 5 9-5M3 17l9 5 9-5",
  palette:
    "M12 21a9 9 0 1 1 0-18c4.97 0 9 3.58 9 8 0 1.66-1.34 3-3 3h-2a2 2 0 0 0-1.6 3.2 2 2 0 0 1-1.6 3.2ZM7.5 10.5h.01M10.5 7.5h.01M14 7.5h.01M16.5 10.5h.01",
  check: "m20 6-11 11-5-5",
  checkCircle: "M22 11.1V12a10 10 0 1 1-5.9-9.1M22 4 12 14.01l-3-3",
  x: "M18 6 6 18M6 6l12 12",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z",
  info: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20ZM12 16v-4M12 8h.01",
  clock: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20ZM12 6v6l4 2",
  download: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M7 10l5 5 5-5M12 15V3",
  upload: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4M17 8l-5-5-5 5M12 3v12",
  globe: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20ZM2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10Z",
  copy: "M20 9H11a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-9a2 2 0 0 0-2-2ZM5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1",
  folder: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
  settings:
    "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6ZM19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6 1.65 1.65 0 0 0 10 3.09V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1Z",
  chevronDown: "m6 9 6 6 6-6",
  chevronRight: "m9 18 6-6-6-6",
  minus: "M5 12h14",
  pen: "M21.17 6.81a2.82 2.82 0 0 0-3.99-3.99L3.84 16.17a2 2 0 0 0-.5.83l-1.32 4.35a.5.5 0 0 0 .62.62l4.35-1.32a2 2 0 0 0 .83-.5Z",
  square: "M19 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2Z",
  crosshair: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20ZM22 12h-4M6 12H2M12 6V2M12 22v-4",
  zap: "M4 14h7l-3 8 12-12h-7l3-8Z",
  play: "m6 3 14 9-14 9V3Z",
  pause: "M14 4h4v16h-4zM6 4h4v16H6z",
  rotate: "M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5",
  share: "M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8M16 6l-4-4-4 4M12 2v13",
  file: "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7ZM14 2v6h6",
};

/** An inline SVG icon at `size` pixels. */
export function icon(name: keyof typeof PATHS | string, size = 16): SVGElement {
  const d = PATHS[name] ?? PATHS.info;
  return s(
    "svg",
    {
      width: size,
      height: size,
      viewBox: "0 0 24 24",
      fill: "none",
      stroke: "currentColor",
      "stroke-width": 2,
      "stroke-linecap": "round",
      "stroke-linejoin": "round",
      "aria-hidden": "true",
      focusable: "false",
    },
    s("path", { d }),
  );
}

/** Whether an icon name is one the set actually has. */
export function hasIcon(name: string): boolean {
  return name in PATHS;
}

/**
 * The Inkvec mark, inline and mono.
 *
 * The drop with a dot and a chevron carved out of it — one path, `fill-rule="evenodd"`
 * — which is the mark the project settled on for Inkvec itself. It is drawn in
 * `currentColor` rather than in the palette, so the bar and About each get it in
 * whatever colour their own text is using and the light theme needs no second file.
 *
 * The geometry is not written here. It is generated from `web/logo.svg`, which is the
 * one the site serves, and `make_assets.py --check` fails if the two drift apart.
 */
export function appMark(size = 18): SVGElement {
  const node = MARK.cloneNode(true) as SVGSVGElement;
  // The file carries `color="#c9754a"` so that rendered on its own — a README image, a
  // file preview — it is the copper mono mark the documentation site shows. In here it
  // should take the chrome's colour instead, so the root's own `color` comes off and
  // `currentColor` inherits.
  node.removeAttribute("color");
  // Taller than it is wide, so the *height* is the size and the width follows from the
  // viewBox. Asking for a square would letterbox it or squash it, and a squashed mark
  // is a different mark.
  const [, , w, hh] = (node.getAttribute("viewBox") ?? "0 0 1 1").split(/\s+/).map(Number);
  node.setAttribute("height", String(size));
  node.setAttribute("width", String(Math.round((size * w) / hh)));
  node.setAttribute("aria-hidden", "true");
  node.setAttribute("focusable", "false");
  return node;
}

/** Parsed once. Every `appMark` is a clone of it. */
const MARK: SVGSVGElement = (() => {
  const root = new DOMParser().parseFromString(markSvg, "image/svg+xml").documentElement;
  return document.importNode(root, true) as unknown as SVGSVGElement;
})();
