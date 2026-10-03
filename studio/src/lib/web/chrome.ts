/**
 * What the browser build adds around the shared interface: files arriving by drag, drop or
 * paste (the desktop's webview reports dropped paths instead), a one-line tip on phones,
 * what the presentation page hands over, the loading screen, and the Hugging Face frame.
 */

import { h, icon } from "../dom";
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
  pastedImages(take);
}

/**
 * Ctrl+V anywhere outside a text field: an image copied from another app (a screenshot, a
 * browser's "Copy image") arrives as a file on the clipboard. Used by the desktop app too.
 */
export function pastedImages(take: (files: File[]) => void): void {
  window.addEventListener("paste", (e) => {
    const target = e.target as HTMLElement | null;
    if (target?.closest?.("input, textarea, [contenteditable]")) return;
    let files = [...(e.clipboardData?.files ?? [])];
    // Some webviews hand a copied bitmap over only as an item, not in `files`.
    if (!files.length) {
      files = [...(e.clipboardData?.items ?? [])]
        .filter((i) => i.kind === "file")
        .map((i) => i.getAsFile())
        .filter((f): f is File => f !== null);
    }
    if (!files.length) return;
    e.preventDefault();
    take(files);
  });
}

/** Narrower than this, with a touch screen, is a phone (the interface's compact layout). */
const PHONE_WIDTH = 760;

/** Where a dismissed phone tip is remembered, for this tab's session only. */
const PHONE_TIP_KEY = "inkvec-phone-tip-dismissed";

/**
 * On a phone, one line at the foot of the screen: the app works here (it has a phone
 * layout, one scrolling column upright and thin chrome on its side), and a larger screen
 * shows the viewer, the controls and the palette side by side. It is a note, not a dialog:
 * nothing waits for it, and its close button (32 px, over the 24 px of WCAG 2.2 SC 2.5.8)
 * removes it for the rest of the session. It used to be a full-screen card saying the app
 * did not fit a phone, which stopped being true when the phone layout arrived.
 *
 * Two things keep it from getting in the way (measured with axe-core and Playwright on the
 * phone sizes, 2026-10-03). It is an `<aside>`, a complementary landmark, because a note
 * outside every landmark is content a screen reader's landmark navigation cannot reach
 * (axe `region`, WCAG 2.2 SC 1.3.1). And while it is shown, its height is published as
 * `--phonetip-h`, which the compact layout adds under the sticky Export foot and at the
 * end of the scrolling page (`styles/app.css`): pinned to the same screen edge, the tip
 * covered Export, and on a 360 x 740 phone a tap on Export landed on the tip.
 */
export function mountWebChrome(app: HTMLElement): void {
  const small = window.matchMedia(`(max-width: ${PHONE_WIDTH}px)`).matches;
  const touch = window.matchMedia("(pointer: coarse)").matches;
  if (!(small && touch)) return;
  try {
    if (sessionStorage.getItem(PHONE_TIP_KEY)) return;
  } catch {
    // No session storage (blocked site data): the tip shows, and closes for this page only.
  }
  const root = document.documentElement;
  let watch: ResizeObserver | null = null;
  const close = () => {
    watch?.disconnect();
    root.style.removeProperty("--phonetip-h");
    tip.remove();
  };
  const tip = h(
    "aside.phonetip",
    {
      "aria-label": "Tip",
      style: {
        position: "fixed",
        left: "0",
        right: "0",
        bottom: "0",
        zIndex: "40",
        display: "flex",
        alignItems: "center",
        gap: "8px",
        padding: "6px 6px 6px 14px",
        background: "var(--card)",
        borderTop: "1px solid var(--rule)",
        color: "var(--dim)",
        fontSize: "13px",
        lineHeight: "1.4",
      },
    },
    h("span", { style: { flex: "1" } }, "Works on a phone; a larger screen shows the drawing, the controls and the palette side by side."),
    h(
      "button.btn.ghost.compact",
      {
        "aria-label": "Close this tip",
        style: { minWidth: "32px", minHeight: "32px", justifyContent: "center" },
        onclick: () => {
          close();
          try {
            sessionStorage.setItem(PHONE_TIP_KEY, "1");
          } catch {
            // Not remembered: the next page load shows it once more.
          }
        },
      },
      icon("x", 14),
    ),
  );
  app.append(tip);
  // The tip wraps to two lines on a narrow phone and changes height when the phone turns.
  const publish = () => root.style.setProperty("--phonetip-h", `${tip.offsetHeight}px`);
  publish();
  if (typeof ResizeObserver !== "undefined") {
    watch = new ResizeObserver(publish);
    watch.observe(tip);
  }
}

/** What the presentation page asked the Studio to open: a bundled sample, or a file. */
export type Launch = { sample: string } | { name: string; bytes: Uint8Array } | null;

/**
 * The presentation page (the Space's root) opens the Studio with `?sample=<file>` for one of
 * the bundled samples, and hands over a file dropped on it in one of two ways:
 *
 * - `?open=handoff`: the page shares this tab's storage (the Space's own address, or "open
 *   here" inside Hugging Face's frame), so it left the file in this origin's IndexedDB
 *   (`inkvec-handoff`). Taken once and deleted.
 * - `#open=<base64url>&name=<name>`: the page is inside Hugging Face's frame and opened this
 *   tab of its own. Neither storage (partitioned under huggingface.co) nor messages (this
 *   page's COOP cuts the tab off from its opener) cross from that frame, so the bytes come in
 *   the URL fragment, which is never sent to a server; see `handOff` in `web/index.html`.
 *
 * Either way the address is cleared at once, so a reload does not open the file again and
 * the bytes do not stay in this tab's history entry.
 */
export async function takeLaunch(): Promise<Launch> {
  const q = new URLSearchParams(window.location.search);
  const sample = q.get("sample");
  const handoff = q.get("open") === "handoff";
  const carried = carriedFile(window.location.hash);
  if (!sample && !handoff && !carried) return null;
  window.history.replaceState(null, "", window.location.pathname);
  if (carried) return carried;
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

/**
 * The file a URL fragment carries, `#open=<base64url bytes>&name=<URI-encoded name>`, or null
 * when there is none or it does not decode. base64url is RFC 4648 section 5 (`-` and `_` for
 * `+` and `/`, no padding), which needs no escaping in a fragment. Exported for the tests.
 */
export function carriedFile(hash: string): { name: string; bytes: Uint8Array } | null {
  if (!hash.startsWith("#")) return null;
  const f = new URLSearchParams(hash.slice(1));
  const data = f.get("open");
  if (!data) return null;
  try {
    const b64 = data.replace(/-/g, "+").replace(/_/g, "/");
    const bin = atob(b64 + "=".repeat((4 - (b64.length & 3)) & 3));
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes.length ? { name: f.get("name") || "image", bytes } : null;
  } catch {
    return null;
  }
}

/**
 * The largest file an address carries: base64 makes 1.5 MB into 2,000,000 characters, inside
 * Chromium's 2 MiB limit on a URL (url::kMaxURLChars); a longer address opens as about:blank.
 * The presentation page's `HANDOFF_MAX` (`web/index.html`) is the same number.
 */
export const HANDOFF_MAX = 1_500_000;

/**
 * The fragment that carries a file into a new tab of the Studio, `#open=<base64url
 * bytes>&name=<URI-encoded name>`, which `carriedFile` reads back; null for an empty file or
 * one larger than `HANDOFF_MAX`. The encoding is the presentation page's (`base64url` in
 * `web/index.html`): RFC 4648 section 5 without padding, built in 32 KiB slices so a large
 * file never becomes one huge argument list. "Open in its own tab" inside the Space's frame
 * (`components/wincontrols.ts`) uses it, for the reason `takeLaunch` gives: from that frame
 * only the address reaches an isolated tab.
 */
export function handOffHash(name: string, bytes: Uint8Array): string | null {
  if (!bytes.length || bytes.length > HANDOFF_MAX) return null;
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  const data = btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  return `#open=${data}&name=${encodeURIComponent(name)}`;
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

/** Paint the newest queued step onto the loading screen: once per animation frame at most. */
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

/**
 * The bytes are in; the module compiles. Its thread pool starts after the app is shown
 * (`startPool` in `engine.worker.ts`), so this screen does not wait for it.
 */
export function bootEngineStarting(): void {
  bootStep("Starting the engine", 0.88, "Compiling");
}

/** The app's own start-up steps (`startup_progress`, 0 to 1), the last tenth of the bar. */
export function bootProgress(text: string, progress: number): void {
  bootStep(text, 0.9 + 0.1 * Math.min(1, Math.max(0, progress)));
}

/** The interface is drawn and its data loaded: the loading screen fades out at once. */
export function bootReady(): void {
  if (bootGone) return;
  // The bar runs to its end quickly, rather than still filling while the screen fades.
  const fillEl = document.getElementById("boot-fill");
  if (fillEl) fillEl.style.transitionDuration = "0.12s";
  bootStep("Ready", 1);
  // Then the fade; the app is already drawn underneath.
  window.setTimeout(() => {
    bootGone = true;
    const boot = document.getElementById("boot");
    if (!boot) return;
    boot.classList.add("leaving");
    window.setTimeout(() => boot.remove(), 260);
  }, 130);
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
