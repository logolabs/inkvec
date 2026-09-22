/**
 * Put ONNX Runtime's DLLs where the bundler can pick them up, for a dynamically linked build.
 *
 * Only for Windows builds that link ONNX Runtime dynamically (see "Building on an older
 * MSVC" in the README). It is run from the overlay config's `beforeBuildCommand`, so the
 * files exist before cargo does.
 *
 * They are copied into `src-tauri/ort-runtime/` rather than bundled from where they came
 * from because `ort` already links them into the target directory, and the bundler's own
 * copy of a resource into that directory then tries to copy a file onto the link that
 * points at it — which Windows reports as a sharing violation, and the build stops. A
 * distinct source has nothing to collide with. Any link left over from a previous build
 * is removed for the same reason.
 *
 *     ORT_LIB_PATH=<onnxruntime>/lib node scripts/stage-ort.mjs
 */

import { copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const STUDIO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(STUDIO, "src-tauri", "ort-runtime");
const TARGET = join(STUDIO, "src-tauri", "target", "release");
const DLLS = ["onnxruntime.dll", "onnxruntime_providers_shared.dll"];

const lib = process.env.ORT_LIB_PATH;
if (!lib) {
  throw new Error("ORT_LIB_PATH is not set: point it at the `lib` folder of an onnxruntime-win-x64 release");
}

mkdirSync(OUT, { recursive: true });
for (const name of DLLS) {
  const from = join(lib, name);
  if (!existsSync(from)) throw new Error(`${from} is not there`);
  copyFileSync(from, join(OUT, name));
  rmSync(join(TARGET, name), { force: true });
  console.log(`ort: staged ${name}`);
}
