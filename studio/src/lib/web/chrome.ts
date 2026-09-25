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
// The desktop's splash window, shown as a card over the page (`#boot`, an iframe of
// splash.html). The rule is the desktop's: the app takes the screen only once it is ready
// AND the splash has played its animation to the end, which the splash reports from its own
// `animationend` rather than a timer; either can come first.

let appIsReady = false;
let animationDone = false;
let bootGone = false;

function bootFrame(): HTMLIFrameElement | null {
  return document.querySelector<HTMLIFrameElement>("#boot iframe");
}

function tell(message: Record<string, unknown>): void {
  bootFrame()?.contentWindow?.postMessage(message, window.location.origin);
}

/** One real step of start-up, for the splash's status line and bar. */
export function bootProgress(text: string, progress: number): void {
  tell({ type: "splash-status", text, progress });
}

function finishBoot(force: boolean): void {
  if (bootGone || !(force || (appIsReady && animationDone))) return;
  bootGone = true;
  const boot = document.getElementById("boot");
  tell({ type: "splash-done" });
  boot?.classList.add("leaving");
  window.setTimeout(() => boot?.remove(), 300);
}

/** The interface is drawn and its data loaded: hand over once the splash has played. */
export function bootReady(): void {
  appIsReady = true;
  finishBoot(false);
  // A splash that never reports (its page failed to load) does not hold the app.
  window.setTimeout(() => finishBoot(true), 6000);
}

window.addEventListener("message", (e: MessageEvent) => {
  if (e.origin !== window.location.origin || (e.data as { type?: string })?.type !== "splash-animation-done") return;
  animationDone = true;
  finishBoot(false);
});

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
