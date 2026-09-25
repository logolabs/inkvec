/**
 * "Auto chose": what the automatic trace can say about the image it just traced, and the
 * presets that would likely suit it better.
 *
 * Nothing here changes a trace. Auto's output is the automatic trace exactly as it always
 * was; these are read off its own report (the colours, the losses, the size) and a couple of
 * facts about the file (lossy, noisy, transparent), and offered as one-click alternatives
 * that stay suggestions until somebody takes one. Every rule is deliberately conservative:
 * a suggestion that is wrong on a clean logo teaches people to ignore the line.
 */

import { lab, parseHex } from "./colour";
import type { Capabilities, Ink, Loss, PresetId, Report, Settings, SourceFacts, SourceInfo } from "./ipc";

export interface AutoSuggestion {
  id: "photo" | "bw" | "lineart" | "icon";
  preset: PresetId;
  /** What was detected, as a plain claim: "Looks like a photo or a screenshot". */
  title: string;
  /** The evidence, as measured: "JPEG · noise 3.4 levels". */
  evidence: string;
  /** What taking it does: "Photo or scan, with the denoiser". */
  action: string;
}

/**
 * Pixel noise, in 8-bit levels, above which a file that is not a JPEG still reads as a
 * photo, a scan or a screenshot of one. A clean render sits at the estimator's floor of
 * half a level; dense line work reads higher than flat art, so this is set well clear of it.
 */
export const NOISY_LEVELS = 2.5;

/** A small image, on its longer side: where Icon's one-pixel speckle floor matters. */
const ICON_PX = 256;

export interface SuggestInput {
  source: SourceInfo | null;
  report: Report | null;
  palette: Ink[];
  losses: Loss[];
  facts: SourceFacts | null;
  settings: Settings;
  preset: string | null;
}

/** Whether the image reads as damaged: a lossy container, or measurably noisy pixels. */
export function looksDamaged(source: SourceInfo | null, facts: SourceFacts | null): boolean {
  return Boolean(source?.lossy) || (facts !== null && facts.noiseLevels >= NOISY_LEVELS);
}

/** The inks that cover a visible share of the canvas. */
function visibleInks(palette: Ink[]): Ink[] {
  return palette.filter((i) => i.share >= 0.005);
}

/** Whether two inks are a dark and a light neutral: what "Black & white" would keep. */
function blackAndWhitePair(inks: Ink[]): boolean {
  if (inks.length !== 2 || inks.some((i) => i.kind === "gradient")) return false;
  const labs = inks.map((i) => parseHex(i.hex)).map((rgb) => (rgb ? lab(rgb) : null));
  if (labs.some((l) => l === null)) return false;
  const [a, b] = labs as [number, number, number][];
  const neutral = (l: [number, number, number]) => Math.hypot(l[1], l[2]) < 12;
  const [dark, light] = a[0] < b[0] ? [a, b] : [b, a];
  return neutral(dark) && neutral(light) && dark[0] < 35 && light[0] > 65;
}

/** The suggestions for this trace, strongest first; at most two. */
export function autoSuggestions(input: SuggestInput): AutoSuggestion[] {
  const { source, report, palette, losses, facts, settings } = input;
  if (!source || !report) return [];
  const out: AutoSuggestion[] = [];

  if (looksDamaged(source, facts) && settings.cleanUpDamage === "off") {
    const evidence = [
      source.lossy ? source.container : null,
      facts ? `noise ${facts.noiseLevels.toFixed(1)} levels` : null,
    ].filter(Boolean);
    out.push({
      id: "photo",
      preset: "photo-or-scan",
      title: "Looks like a photo, a scan or a screenshot",
      evidence: evidence.join(" · "),
      action: "Photo or scan, with the denoiser",
    });
  }

  const inks = visibleInks(palette);
  if (!settings.blackAndWhite && blackAndWhitePair(inks)) {
    out.push({
      id: "bw",
      preset: "black-and-white",
      title: "Two colours, black and white",
      evidence: `${inks.length} inks cover the canvas`,
      action: "Black & white",
    });
  }

  if (!settings.lineArt && losses.some((l) => l.kind === "strokes") && !settings.blackAndWhite) {
    out.push({
      id: "lineart",
      preset: "line-art",
      title: "Drawn with strokes",
      evidence: "line work came back as filled outlines",
      action: "Line art, as strokes",
    });
  }

  const longer = Math.max(source.width, source.height);
  const gradients = palette.some((i) => i.kind === "gradient");
  const iconLike = settings.speckleFloor <= 1 && settings.maxColours <= 16;
  if (!iconLike && longer <= ICON_PX && !gradients && inks.length <= 16 && out.length === 0) {
    out.push({
      id: "icon",
      preset: "icon",
      title: "A small flat mark",
      evidence: `${longer} px, ${inks.length} colour${inks.length === 1 ? "" : "s"}`,
      action: "Icon, keeping one-pixel details",
    });
  }
  return out.slice(0, 2);
}

/** How many controls differ between two settings, counted over the Tune tab's rows. */
export function changedControls(caps: Capabilities | null, a: Settings, b: Settings): (keyof Settings)[] {
  return (caps?.controls ?? []).filter((c) => a[c.key] !== b[c.key]).map((c) => c.key);
}

/** The settings Auto used, in words: "the Logo preset, the default". */
export function describeAuto(caps: Capabilities | null, settings: Settings, preset: string | null): string {
  const named = caps?.presets.find((p) => p.id === preset);
  if (named) {
    const moved = changedControls(caps, settings, named.settings).length;
    const base = named.id === "logo" ? "the Logo preset, the default" : `the ${named.name} preset`;
    return moved ? `${base}, with ${moved} control${moved === 1 ? "" : "s"} changed last time` : base;
  }
  return "your settings from last time";
}
