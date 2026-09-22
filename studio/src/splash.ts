/**
 * The splash window's script.
 *
 * Three jobs: draw the mark (outline first, then filled), say which version this is, and
 * follow the app's real start-up. The status line and the bar are driven by events the
 * main window sends as it actually finishes each step, not by a timer pretending to be
 * progress. When the app is ready the backend tells this window, which fades out, and the
 * backend then swaps the windows.
 */

import { emit, listen } from "@tauri-apps/api/event";

import markSvg from "./assets/mark.svg?raw";

const SVG_NS = "http://www.w3.org/2000/svg";

interface Progress {
  text: string;
  /** 0 to 1. */
  progress: number;
}

const root = document.getElementById("splash") as HTMLElement;
const bar = document.getElementById("bar") as HTMLElement;
const status = document.getElementById("status") as HTMLElement;

/** The mark: the outline of every shape in it, then the shapes themselves. */
function drawMark(): void {
  const doc = new DOMParser().parseFromString(markSvg, "image/svg+xml").documentElement;
  const path = doc.querySelector("path");
  const host = document.getElementById("mark");
  if (!path || !host) return;

  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", doc.getAttribute("viewBox") ?? "0 0 1 1");
  for (const cls of ["outline", "body"]) {
    const p = document.createElementNS(SVG_NS, "path");
    p.setAttribute("d", path.getAttribute("d") ?? "");
    p.setAttribute("class", cls);
    // A path length of one makes the dash animation independent of the geometry's size.
    if (cls === "outline") p.setAttribute("pathLength", "1");
    svg.append(p);
  }
  host.append(svg);
}

function platformName(): string {
  const ua = navigator.userAgent;
  if (/Windows/i.test(ua)) return "Windows";
  if (/Mac/i.test(ua)) return "macOS";
  return "Linux";
}

function setProgress(p: Progress): void {
  status.textContent = p.text;
  bar.style.width = `${Math.round(Math.min(1, Math.max(0, p.progress)) * 100)}%`;
}

async function main(): Promise<void> {
  drawMark();
  document.getElementById("version")!.textContent = `Version ${__APP_VERSION__} · ${platformName()}`;

  // The wordmark waits for its face. Starting the animation on a fallback font and then
  // swapping would be the first thing anyone saw of the product.
  await Promise.race([
    Promise.all([
      document.fonts.load('500 68px "Playfair Studio"'),
      document.fonts.load('400 12px "Inter Studio"'),
    ]),
    new Promise((resolve) => setTimeout(resolve, 900)),
  ]).catch(() => undefined);
  setProgress({ text: "Starting the engine…", progress: 0.12 });
  document.body.classList.add("go");
  void reportWhenPlayed();

  try {
    await listen<Progress>("splash-status", (e) => setProgress(e.payload));
    await listen("splash-done", () => {
      root.classList.add("done", "leaving");
    });
  } catch {
    // Opened outside the app (a browser, a preview): the animation still plays.
  }
}

/**
 * Tell the backend once the animation has played to the end.
 *
 * The app is often ready before the mark has finished drawing itself, and taking the
 * screen then would cut the one thing this window is for. So the backend does not swap
 * the windows until this has been said.
 *
 * The end is the animation's own `animationend`, not a timer. The animation clock starts at
 * the first frame this window actually paints, which on a cold start is well after the page
 * began to run, so any number of milliseconds counted from here would be a guess that is
 * wrong exactly when the machine is slow. Which animation ends last is read from the
 * stylesheet, so changing the CSS cannot leave a stale duration behind.
 */
async function reportWhenPlayed(): Promise<void> {
  // The class that starts the animations has to have been styled first.
  await new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
  const last = lastToFinish();
  if (last) {
    await Promise.race([
      new Promise<void>((resolve) => {
        last.el.addEventListener("animationend", (e) => {
          if ((e as AnimationEvent).animationName === last.name) resolve();
        });
      }),
      // Only if the animation is somehow stopped for good; the app is not held for it.
      new Promise((resolve) => setTimeout(resolve, 10000)),
    ]);
  }
  // A beat on the finished picture, so it is seen rather than merely reached.
  await new Promise((resolve) => setTimeout(resolve, 450));
  try {
    await emit("splash-animation-done");
  } catch {
    // Outside the app there is nobody to tell.
  }
}

/** The element and animation that end last, among those that end at all. */
function lastToFinish(): { el: Element; name: string } | null {
  const seconds = (v: string) => (v.trim().endsWith("ms") ? parseFloat(v) / 1000 : parseFloat(v));
  const list = (v: string) => v.split(",").map((s) => s.trim());
  let best: { el: Element; name: string; end: number } | null = null;
  for (const el of document.querySelectorAll("*")) {
    const cs = getComputedStyle(el);
    const names = list(cs.animationName);
    const durations = list(cs.animationDuration);
    const delays = list(cs.animationDelay);
    const counts = list(cs.animationIterationCount);
    names.forEach((name, i) => {
      const count = counts[i % counts.length];
      if (name === "none" || count === "infinite") return;
      const end = seconds(delays[i % delays.length]) + seconds(durations[i % durations.length]) * Number(count);
      if (!best || end > best.end) best = { el, name, end };
    });
  }
  return best;
}

void main();
