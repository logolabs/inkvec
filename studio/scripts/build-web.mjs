#!/usr/bin/env node
// Build the Hugging Face Space: the presentation page at the root, and Inkvec Studio Lite (the
// Studio's interface and its shared core, as WebAssembly) under studio/. Any host that sends
// the two cross-origin isolation headers serves it the same way.
//
//   node scripts/build-web.mjs              # both WebAssembly builds, then the site
//   node scripts/build-web.mjs --skip-wasm  # reuse the WebAssembly already in web/public
//
// Output: studio/dist-web/
//   index.html, showcase.json, favicon.svg, fonts/   the presentation page (web/index.html),
//                                                    its gallery data (tools/showcase_data.py)
//   README.md                                        the Space's card (web/README.md here)
//   studio/                                          Inkvec Studio Lite: the bundled interface,
//     pkg/, pkg-threads/                             the core, one core / a rayon pool
//     denoise.js, samples/, guide/, notices          what the Studio fetches at run time
// `scripts/deploy-space.py` uploads it; nothing here touches the network except cargo and
// wasm-pack's own downloads.
//
// Prerequisites, as for the engine's own browser package (tools/build_wasm.sh):
//   rustup target add wasm32-unknown-unknown
//   rustup toolchain install nightly --component rust-src
//   cargo install wasm-pack
//
// The two arms build the same crate for the same triple, so they need separate target
// directories: `$CARGO_TARGET_DIR/wasm-st` and `/wasm-mt` (default `studio/target-web`).

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const STUDIO = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = resolve(STUDIO, "..");
const CRATE = join(STUDIO, "wasm");
const PUBLIC = join(STUDIO, "web", "public");
const OUT = join(STUDIO, "dist-web");
const APP = join(OUT, "studio");
const TARGET = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(STUDIO, "target-web");
const skipWasm = process.argv.includes("--skip-wasm");

function run(cmd, args, env = {}, cwd = STUDIO) {
  console.log(`\n==> ${cmd} ${args.join(" ")}`);
  // Windows needs a shell to find npx.cmd; a shell needs the paths with spaces quoted.
  const win = process.platform === "win32";
  const argv = win ? args.map((a) => (/[\s"]/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a)) : args;
  const r = spawnSync(cmd, argv, { cwd, stdio: "inherit", env: { ...process.env, ...env }, shell: win });
  if (r.status !== 0) {
    console.error(`${cmd} failed (${r.status ?? r.signal})`);
    process.exit(1);
  }
}

// ---------------------------------------------------------------- WebAssembly ---

const ST = join(PUBLIC, "pkg");
const MT = join(PUBLIC, "pkg-threads");

if (!skipWasm) {
  // One core, stable toolchain, runs anywhere.
  run("wasm-pack", ["build", CRATE, "--target", "web", "--release", "--out-dir", ST], {
    CARGO_TARGET_DIR: join(TARGET, "wasm-st"),
  });
  // A rayon pool. See tools/build_wasm.sh for why each flag is there; the same four
  // failures apply to this crate. +simd128 is restated because RUSTFLAGS replaces the
  // repository's .cargo/config.toml rather than adding to it.
  const rustflags = [
    "-C target-feature=+atomics,+bulk-memory,+mutable-globals,+simd128",
    "-C link-arg=--shared-memory",
    "-C link-arg=--import-memory",
    "-C link-arg=--max-memory=4294967296",
    "-C link-arg=--export=__wasm_init_tls",
    "-C link-arg=--export=__tls_size",
    "-C link-arg=--export=__tls_align",
    "-C link-arg=--export=__tls_base",
  ].join(" ");
  run(
    "wasm-pack",
    ["build", CRATE, "--target", "web", "--release", "--out-dir", MT, "--", "-Z", "build-std=panic_abort,std", "--features", "threads"],
    { CARGO_TARGET_DIR: join(TARGET, "wasm-mt"), RUSTUP_TOOLCHAIN: "nightly", RUSTFLAGS: rustflags },
  );
}

for (const dir of [ST, MT]) {
  if (!existsSync(join(dir, "inkvec_studio_wasm_bg.wasm"))) {
    console.error(`missing ${dir}: build the WebAssembly first (drop --skip-wasm)`);
    process.exit(1);
  }
  // wasm-pack writes these for npm; the site wants neither.
  rmSync(join(dir, ".gitignore"), { force: true });
  rmSync(join(dir, "package.json"), { force: true });
}

// A cache token from the two modules' own bytes: a new build is a new URL, an unchanged one
// keeps its cache. It is on every URL the worker loads (see engine.worker.ts).
const hash = createHash("sha256");
for (const dir of [ST, MT]) hash.update(readFileSync(join(dir, "inkvec_studio_wasm_bg.wasm")));
const token = hash.digest("hex").slice(0, 12);

// wasm-bindgen-rayon's worker helper imports its package as a directory ('../../..'), which
// only a bundler resolves; name the file, with the same token as its parent.
const snippets = join(MT, "snippets");
let patched = 0;
for (const pkg of existsSync(snippets) ? readdirSync(snippets) : []) {
  const helper = join(snippets, pkg, "src", "workerHelpers.js");
  if (!existsSync(helper)) continue;
  const text = readFileSync(helper, "utf8");
  const next = text.replace(/await import\('\.\.\/\.\.\/\.\.(?:\/inkvec_studio_wasm\.js[^']*)?'\)/, `await import('../../../inkvec_studio_wasm.js?v=${token}')`);
  if (!next.includes(`inkvec_studio_wasm.js?v=${token}`)) {
    console.error(`the rayon worker helper could not be patched: ${helper}`);
    process.exit(1);
  }
  writeFileSync(helper, next);
  patched++;
}
if (!patched) {
  console.error("no rayon worker helper found in pkg-threads/snippets");
  process.exit(1);
}

// ------------------------------------------------------------ static files ---

// Everything the page fetches at run time besides the WebAssembly.
mkdirSync(join(PUBLIC, "samples"), { recursive: true });
cpSync(join(STUDIO, "public"), PUBLIC, { recursive: true });
cpSync(join(ROOT, "web", "denoise.js"), join(PUBLIC, "denoise.js"));
cpSync(join(ROOT, "web", "favicon.svg"), join(PUBLIC, "favicon.svg"));
for (const f of readdirSync(join(STUDIO, "src-tauri", "samples"))) {
  if (f.endsWith(".png")) cpSync(join(STUDIO, "src-tauri", "samples", f), join(PUBLIC, "samples", f));
}
cpSync(join(STUDIO, "STUDIO_THIRD_PARTY.md"), join(PUBLIC, "STUDIO_THIRD_PARTY.md"));
cpSync(join(ROOT, "docs", "THIRD_PARTY.md"), join(PUBLIC, "THIRD_PARTY.md"));
cpSync(join(ROOT, "LICENSE"), join(PUBLIC, "LICENSE"));
cpSync(join(ROOT, "NOTICE"), join(PUBLIC, "NOTICE"));

// ------------------------------------------------------------------ the site ---

rmSync(OUT, { recursive: true, force: true });
run("npx", ["tsc", "--noEmit"]);
// Into dist-web/studio (vite.config.ts); its URLs are relative, so the subfolder is free.
run("npx", ["vite", "build", "--mode", "web"], { INKVEC_WASM_TOKEN: token });

// The presentation page at the root, with its gallery data and the Studio's fonts, and the
// Space's card: its front matter asks Hugging Face for the isolation headers.
cpSync(join(ROOT, "web", "index.html"), join(OUT, "index.html"));
cpSync(join(ROOT, "web", "showcase.json"), join(OUT, "showcase.json"));
cpSync(join(ROOT, "web", "favicon.svg"), join(OUT, "favicon.svg"));
cpSync(join(STUDIO, "public", "fonts"), join(OUT, "fonts"), { recursive: true });
cpSync(join(STUDIO, "web", "README.md"), join(OUT, "README.md"));
if (!existsSync(join(APP, "index.html")) || !existsSync(join(APP, "guide"))) {
  console.error(`the Studio did not land in ${APP} with its guide`);
  process.exit(1);
}

const size = (p) => {
  let total = 0;
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const f = join(d, e.name);
      if (e.isDirectory()) walk(f);
      else total += readFileSync(f).length;
    }
  };
  if (statSync(p).isFile()) return statSync(p).size;
  walk(p);
  return total;
};
const mb = (n) => `${(n / 1024 / 1024).toFixed(2)} MB`;
console.log(`\nThe Space built into ${OUT}`);
console.log(`  token            ${token}`);
console.log(`  landing          ${mb(size(join(OUT, "index.html")) + size(join(OUT, "showcase.json")) + size(join(OUT, "fonts")))}`);
console.log(`  studio/pkg       ${mb(size(join(APP, "pkg")))}`);
console.log(`  studio/pkg-thr.  ${mb(size(join(APP, "pkg-threads")))}`);
console.log(`  studio/assets    ${mb(size(join(APP, "assets")))}`);
console.log(`  studio/guide     ${mb(size(join(APP, "guide")))}`);
console.log(`  site total       ${mb(size(OUT))}`);
