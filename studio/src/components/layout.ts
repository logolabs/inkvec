/**
 * Which layout the window is in, for the few choices the stylesheet cannot make alone.
 *
 * The compact layout (phones, and windows too small for three columns) is defined in
 * `styles/app.css` under the same media query: one scrolling column below 760 px wide,
 * thinner chrome below 500 px tall. Script reads it only where a starting state depends on
 * it, such as the engine log starting folded so it does not cover the drawing.
 *
 * Not from the literature: the two thresholds. 760 px is where the web build already says a
 * phone starts (`lib/web/chrome.ts`), and 500 px tall is below every laptop screen in the
 * r2-product layout runs (720 px and up) and above every landscape phone (390-430 px).
 */

/** The compact layout's media query; keep it equal to the one in `styles/app.css`. */
export const COMPACT = "(max-width: 760px), (max-height: 500px)";

/** Whether the window is in the compact layout now. False where there is no `matchMedia`. */
export function isCompact(): boolean {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" && window.matchMedia(COMPACT).matches;
}
