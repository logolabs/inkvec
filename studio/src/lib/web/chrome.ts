/**
 * What the browser build adds around the shared interface: files arriving by drag, drop or
 * paste (the desktop's webview reports dropped paths instead), and a plain word for phones.
 */

import { h } from "../dom";
import type { Store } from "../state";

/**
 * Files dropped anywhere on the page, or pasted (an image copied from another app arrives
 * as a file on the clipboard). `take` gets them in the order they came.
 */
export function webDrops(store: Store, take: (files: File[]) => void): void {
  let depth = 0;
  const hasFiles = (e: DragEvent) => [...(e.dataTransfer?.types ?? [])].includes("Files");
  const leave = () => {
    depth = 0;
    store.set({ dragging: false });
    document.body.classList.remove("dragging");
  };
  window.addEventListener("dragenter", (e) => {
    if (!hasFiles(e)) return;
    depth++;
    store.set({ dragging: true });
    document.body.classList.add("dragging");
  });
  window.addEventListener("dragover", (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
  });
  window.addEventListener("dragleave", (e) => {
    if (!hasFiles(e)) return;
    depth = Math.max(0, depth - 1);
    if (depth === 0) leave();
  });
  window.addEventListener("drop", (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    leave();
    const files = [...(e.dataTransfer?.files ?? [])];
    if (files.length) take(files);
  });
  window.addEventListener("paste", (e) => {
    const target = e.target as HTMLElement | null;
    if (target?.closest?.("input, textarea, [contenteditable]")) return;
    const files = [...(e.clipboardData?.files ?? [])];
    if (!files.length) return;
    e.preventDefault();
    take(files);
  });
}

/** Narrower than this, with a touch screen, the three-column interface does not fit. */
const PHONE_WIDTH = 760;

/**
 * On a phone, say so rather than squeeze: the viewer, the rail and the palette need a
 * laptop's width. The note can be dismissed, and the app underneath works as it is.
 */
export function mountWebChrome(app: HTMLElement): void {
  const small = window.matchMedia(`(max-width: ${PHONE_WIDTH}px)`).matches;
  const touch = window.matchMedia("(pointer: coarse)").matches;
  if (!(small && touch)) return;
  const note = h(
    "div.phonenote",
    { role: "dialog", "aria-label": "Best on a larger screen" },
    h(
      "div.phonecard",
      null,
      h("span.serif", { style: { fontSize: "22px" } }, "Best on a larger screen"),
      h(
        "p",
        null,
        "Inkvec Studio Lite is a full editor: a viewer, a rail of controls and a palette side by side. On a phone they do not fit. Open this page on a laptop or desktop to use it; everything runs in your browser and nothing is uploaded.",
      ),
      h("button.btn", { onclick: () => note.remove() }, "Continue anyway"),
    ),
  );
  app.append(note);
}

/** What the presentation page asked the Studio to open: a bundled sample, or a file. */
export type Launch = { sample: string } | { name: string; bytes: Uint8Array } | null;

/**
 * The presentation page (the Space's root) opens the Studio with `?sample=<file>` for one of
 * the bundled samples, or `?open=handoff` for a file dropped on it, which it left in this
 * origin's IndexedDB (`inkvec-handoff`) rather than sending anywhere. The file is taken
 * once and deleted, and the query string is cleared so a reload does not open it again.
 */
export async function takeLaunch(): Promise<Launch> {
  const q = new URLSearchParams(window.location.search);
  const sample = q.get("sample");
  const handoff = q.get("open") === "handoff";
  if (!sample && !handoff) return null;
  window.history.replaceState(null, "", window.location.pathname);
  if (sample) return { sample };
  return new Promise((resolve) => {
    let req: IDBOpenDBRequest;
    try {
      req = indexedDB.open("inkvec-handoff", 1);
    } catch {
      resolve(null);
      return;
    }
    req.onupgradeneeded = () => req.result.createObjectStore("files");
    req.onerror = () => resolve(null);
    req.onsuccess = () => {
      const store = req.result.transaction("files", "readwrite").objectStore("files");
      const get = store.get("open");
      get.onsuccess = () => {
        store.delete("open");
        const v = get.result as { name?: string; bytes?: Uint8Array } | undefined;
        resolve(v?.bytes ? { name: v.name ?? "image", bytes: v.bytes } : null);
      };
      get.onerror = () => resolve(null);
    };
  });
}

// ---------------------------------------------------------------- the loading screen ---
//
// The browser build's own loading screen (`#boot`, from web/boot/, inlined into the page so it
// paints with the first frame). Not the desktop's splash card: the whole page, with real
// progress on one bar -- the engine's bytes as they arrive, its start, the app's own first
// steps -- and it leaves the moment the app is drawn, with a short fade. Nothing waits for an
// animation to finish, and nothing is added to make it look busier than it is.
//
// The bar is one element scaled by one CSS transition; every step below only ever moves it
// forward, and at most once a frame.

let shown = 0;
let queued: { text: string; fraction: number; detail: string } | null = null;
let frame = 0;
let bootGone = false;
let lastStep = performance.now();

const mb = (n: number) => (n / (1024 * 1024)).toFixed(1);

function paintBoot(): void {
  frame = 0;
  const step = queued;
  queued = null;
  if (!step || bootGone) return;
  shown = step.fraction;
  const bar = document.getElementById("boot-bar");
  const fillEl = document.getElementById("boot-fill");
  const status = document.getElementById("boot-status");
  const pct = document.getElementById("boot-pct");
  const detail = document.getElementById("boot-detail");
  const percent = Math.round(shown * 100);
  if (fillEl) fillEl.style.transform = `scaleX(${shown})`;
  bar?.setAttribute("aria-valuenow", String(percent));
  if (status) status.textContent = step.text;
  if (pct) pct.textContent = `${percent}%`;
  if (detail) detail.textContent = step.detail;
}

/** One real step of start-up: what is happening, how far along (0 to 1), and a detail line. */
export function bootStep(text: string, fraction: number, detail = ""): void {
  if (bootGone) return;
  lastStep = performance.now();
  queued = { text, fraction: Math.max(shown, queued?.fraction ?? 0, Math.min(1, fraction)), detail };
  if (!frame) frame = requestAnimationFrame(paintBoot);
}

/** The engine's WebAssembly arriving: most of the wait on a cold visit, so most of the bar. */
export function bootEngineBytes(got: number, total: number): void {
  const share = total > 0 ? Math.min(1, got / total) : 0;
  bootStep("Downloading the engine", 0.04 + 0.8 * share, total > 0 ? `${mb(got)} of ${mb(total)} MB` : `${mb(got)} MB`);
}

/** The bytes are in; the module compiles and its thread pool starts. */
export function bootEngineStarting(): void {
  bootStep("Starting the engine", 0.88, "Compiling, and starting a thread per core");
}

/** The app's own start-up steps (`startup_progress`, 0 to 1), the last tenth of the bar. */
export function bootProgress(text: string, progress: number): void {
  bootStep(text, 0.9 + 0.1 * Math.min(1, Math.max(0, progress)));
}

/** The interface is drawn and its data loaded: the loading screen fades out at once. */
export function bootReady(): void {
  if (bootGone) return;
  bootStep("Ready", 1);
  // The frame that shows the full bar, then the fade; the app is already underneath.
  requestAnimationFrame(() => {
    bootGone = true;
    const boot = document.getElementById("boot");
    if (!boot) return;
    boot.classList.add("leaving");
    window.setTimeout(() => boot.remove(), 260);
  });
}

// The desktop imports this module too (its drop and launch helpers are shared), and has its
// splash window instead: none of this runs there.
if (__INKVEC_WEB__) {
  // Tells the head script's failsafe (web/boot/boot.js) that the app's own script is running.
  (window as { __inkvecBoot?: boolean }).__inkvecBoot = true;
  bootStep("Loading the Studio", 0.03);

  // A start-up that stops moving says so rather than showing a bar that never moves. It is
  // still waiting (the app takes over the moment it is ready); only the words change.
  const watchdog = window.setInterval(() => {
    if (bootGone) {
      window.clearInterval(watchdog);
      return;
    }
    if (performance.now() - lastStep > 20_000) {
      const detail = document.getElementById("boot-detail");
      if (detail) {
        detail.textContent =
          "Still waiting for the network. On a slow connection the first visit can take a minute; later ones start from the browser's cache.";
      }
    }
  }, 2_000);
}

// ---------------------------------------------------------------- inside Hugging Face ---

/**
 * Whether the page is inside someone else's frame: Hugging Face shows a Space in an iframe
 * under its own header. `framed` then puts a gap between that header and the app bar; full
 * screen takes it away again (`fullscreen`).
 */
export function markFramed(): void {
  let framed: boolean;
  try {
    framed = window.self !== window.top;
  } catch {
    framed = true;
  }
  const origins = (window.location as Location & { ancestorOrigins?: DOMStringList }).ancestorOrigins;
  if (origins) {
    // An opaque ancestor (about:blank, a sandbox) reads "null", which is not a URL.
    for (let i = 0; i < origins.length; i++) if (/(^|\.)huggingface\.co(:\d+)?$/.test(origins[i].replace(/^[a-z]+:\/\//, ""))) framed = true;
  }
  const root = document.documentElement;
  root.classList.toggle("framed", framed);
  const sync = () => root.classList.toggle("fullscreen", Boolean(document.fullscreenElement));
  document.addEventListener("fullscreenchange", sync);
  sync();
}
