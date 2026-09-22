/**
 * Build the `inkvec` command and put it where Tauri expects a sidecar.
 *
 * The app offers "Add inkvec to PATH" in Settings, and the only version of that feature
 * worth having ships the command built from this same tree by this same release — so a
 * trace started from the terminal and a trace started from the window are the same engine
 * at the same version. That means the binary has to exist before the bundler runs, which
 * is what this does, from `beforeBuildCommand`.
 *
 * Tauri names an external binary `<name>-<target triple>` and strips the triple when it
 * installs it beside the app, so that is the name written here.
 *
 *     node scripts/sidecar.mjs [--target <triple>]
 *
 * The target comes from `--target`, else `SIDECAR_TARGET`, else this host. Tauri does not
 * pass its own `--target` down to `beforeBuildCommand`, so a cross build has to say which
 * triple it means; the release workflow sets the environment variable from its matrix.
 */

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const STUDIO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = resolve(STUDIO, "..");
const OUT = join(STUDIO, "src-tauri", "binaries");

/** The triple to build for. */
function triple() {
  const flag = process.argv.indexOf("--target");
  if (flag !== -1 && process.argv[flag + 1]) return process.argv[flag + 1];
  if (process.env.SIDECAR_TARGET) return process.env.SIDECAR_TARGET;
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  const host = /^host:\s*(.+)$/m.exec(out);
  if (!host) throw new Error("could not read the host target from `rustc -vV`");
  return host[1].trim();
}

function main() {
  const target = triple();
  const exe = target.includes("windows") ? ".exe" : "";

  // The command the app carries has the restorer wherever the app does, so `inkvec
  // --restore` on the PATH and "Clean up damage" in the window are the same thing. The
  // app's own feature flags do not reach this script, so the rule is written here too:
  // every target but the one ONNX Runtime has no package for. SIDECAR_FEATURES overrides
  // it, and an empty value means none.
  const NO_ONNX_RUNTIME = ["x86_64-apple-darwin"];
  const features =
    process.env.SIDECAR_FEATURES !== undefined
      ? process.env.SIDECAR_FEATURES.trim()
      : NO_ONNX_RUNTIME.includes(target)
        ? ""
        : "restore-model";
  const featureArgs = features ? ["--features", features] : [];
  // Built into the engine's own target directory, so a developer who has already built
  // the CLI does not build it twice.
  console.log(`sidecar: building inkvec for ${target}${features ? ` with ${features}` : ""}`);
  execFileSync(
    "cargo",
    ["build", "--release", "-p", "inkvec-cli", "--target", target, ...featureArgs],
    { cwd: REPO, stdio: "inherit" },
  );

  const built = join(REPO, "target", target, "release", `inkvec${exe}`);
  if (!existsSync(built)) {
    throw new Error(`cargo reported success but ${built} is not there`);
  }
  mkdirSync(OUT, { recursive: true });
  const dest = join(OUT, `inkvec-${target}${exe}`);
  copyFileSync(built, dest);
  console.log(`sidecar: ${dest}`);
}

main();
