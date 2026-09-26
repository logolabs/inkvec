/**
 * Inkvec Studio Lite's denoiser download, in words and as a bar: one wording for the three
 * places that show it (the Denoiser control in the rail, the status strip, a toast), so they
 * never disagree. The desktop has no `denoiserFetch` and never reaches this.
 */

import { h } from "../lib/dom";
import type { DenoiserFetch } from "../lib/ipc";

const mb = (n: number) => `${Math.round(n / (1024 * 1024))}`;

/** 0 to 100, or null when the size is not known yet. */
export function fetchPercent(f: DenoiserFetch): number | null {
  if (f.phase === "stored" || f.phase === "preparing" || f.phase === "ready") return 100;
  if (f.phase !== "downloading" || !f.total) return null;
  return Math.min(100, Math.floor((f.got / f.total) * 100));
}

/**
 * What the download is doing, or null when there is nothing worth saying (stored and not
 * wanted yet, ready, or never started). `wanted`: the controls ask for the denoiser now.
 */
export function fetchLine(f: DenoiserFetch | null, wanted: boolean): string | null {
  if (!f) return wanted ? "Starting the denoiser…" : null;
  switch (f.phase) {
    case "downloading": {
      const pct = fetchPercent(f);
      return f.total
        ? `Downloading the denoiser, ${mb(f.got)} of ${mb(f.total)} MB, ${pct}%`
        : `Downloading the denoiser, ${mb(f.got)} MB`;
    }
    case "stored":
    case "preparing":
      return wanted ? "Preparing the denoiser…" : null;
    case "failed":
      return "The denoiser did not download";
    case "idle":
      return wanted ? "Starting the denoiser download…" : null;
    case "ready":
      return null;
  }
}

/** `line` as a sentence: a full stop, unless it already ends in an ellipsis. */
export function sentence(line: string): string {
  return /[….]$/.test(line) ? line : `${line}.`;
}

/** A thin bar for the same progress, or null when there is none to show. */
export function fetchBar(f: DenoiserFetch | null): HTMLElement | null {
  if (!f || (f.phase !== "downloading" && f.phase !== "preparing" && f.phase !== "stored")) return null;
  const pct = fetchPercent(f) ?? 0;
  return h(
    "div.progress.fetchbar",
    {
      role: "progressbar",
      "aria-label": "Denoiser download",
      "aria-valuemin": "0",
      "aria-valuemax": "100",
      "aria-valuenow": String(pct),
    },
    h("i", { style: { width: `${pct}%` } }),
  );
}
