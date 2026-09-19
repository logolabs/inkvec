// Shared by the tests: where things are, and small utilities.

import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const PKG = resolve(dirname(fileURLToPath(import.meta.url)), "..");
export const ROOT = resolve(PKG, "../..");
export const SAMPLES = join(ROOT, "web", "samples");
export const CONTRACT = join(ROOT, "bindings", "contract");

export const samples = () =>
  readdirSync(SAMPLES)
    .filter((f) => f.endsWith(".png"))
    .sort()
    .map((f) => ({ name: f, bytes: readFileSync(join(SAMPLES, f)) }));

export const sample = (name) => readFileSync(join(SAMPLES, name));

export const sha256 = (text) => createHash("sha256").update(text, "utf8").digest("hex");

export function cargoVersion() {
  const toml = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[)/m.exec(toml);
  return /^version\s*=\s*"([^"]+)"/m.exec(section[1])[1];
}

/** The bindings module the package itself loads, for calling the raw exports. */
export async function rawGlue(arm) {
  return import(new URL(`../wasm/${arm}/inkvec_wasm.js`, import.meta.url).href);
}

/**
 * A white disk on a transparent ground: the case `cutout` exists for. Anti-aliased edge
 * (4x4 supersampled coverage in alpha), colour white everywhere.
 */
export function whiteDisk(size = 64) {
  const data = new Uint8ClampedArray(size * size * 4);
  const c = size / 2;
  const r = size * 0.3;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let inside = 0;
      for (let sy = 0; sy < 4; sy++) {
        for (let sx = 0; sx < 4; sx++) {
          const dx = x + (sx + 0.5) / 4 - c;
          const dy = y + (sy + 0.5) / 4 - c;
          if (dx * dx + dy * dy <= r * r) inside++;
        }
      }
      data.set([255, 255, 255, Math.round((inside / 16) * 255)], (y * size + x) * 4);
    }
  }
  return { data, width: size, height: size };
}

/** Every `fill="..."` colour in a document, lower-cased. */
export const fills = (svg) => [...svg.matchAll(/fill="([^"]+)"/g)].map((m) => m[1].toLowerCase());
